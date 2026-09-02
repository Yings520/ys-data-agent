//! Fixed-origin ChatGPT Subscription OAuth transport.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use oauth2::{AccessToken, AuthorizationCode, PkceCodeVerifier, RefreshToken};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use ys_agent_core::{
    CredentialGeneration, CredentialKind, CredentialLease, CredentialVault,
    DeviceAuthorizationView, OAuthConnectionService, OAuthConnectionStatus, OAuthConnectionView,
    OperationId, ProfileId, ProtectedCredentialWrite, ProviderCredentialReference,
    ProviderErrorCode, ProviderField, ProviderManagementError, ProviderRemediation, ProviderResult,
    RemoteRevocationOutcome, SecretValue,
};
use zeroize::{Zeroize, Zeroizing};

pub const CHATGPT_AUTH_ORIGIN: &str = "https://auth.openai.com";
pub const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CHATGPT_DEVICE_USER_CODE_ENDPOINT: &str =
    "https://auth.openai.com/api/accounts/deviceauth/usercode";
pub const CHATGPT_DEVICE_POLL_ENDPOINT: &str =
    "https://auth.openai.com/api/accounts/deviceauth/token";
pub const CHATGPT_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
pub const CHATGPT_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
pub const CHATGPT_REVOKE_ENDPOINT: &str = "https://auth.openai.com/oauth/revoke";
pub const CHATGPT_DEVICE_CALLBACK: &str = "https://auth.openai.com/deviceauth/callback";

const DEVICE_CODE_LIFETIME: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_OAUTH_RESPONSE_BYTES: usize = 64 * 1024;
const TOKEN_BUNDLE_VERSION: u32 = 1;

#[derive(Clone)]
pub struct ChatGptOAuthManager {
    http: Client,
    vault: Arc<dyn CredentialVault>,
    endpoints: OAuthEndpoints,
    browser: Arc<dyn BrowserLauncher>,
    state: Arc<Mutex<ManagerState>>,
}

impl ChatGptOAuthManager {
    pub fn new(vault: Arc<dyn CredentialVault>) -> ProviderResult<Self> {
        Self::build(
            vault,
            OAuthEndpoints::production(),
            Arc::new(SystemBrowserLauncher),
            true,
        )
    }

    fn build(
        vault: Arc<dyn CredentialVault>,
        endpoints: OAuthEndpoints,
        browser: Arc<dyn BrowserLauncher>,
        https_only: bool,
    ) -> ProviderResult<Self> {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .no_proxy()
            .build()
            .map_err(|_| internal_error())?;
        Ok(Self {
            http,
            vault,
            endpoints,
            browser,
            state: Arc::new(Mutex::new(ManagerState::default())),
        })
    }

    #[cfg(test)]
    fn with_endpoints_for_test(
        vault: Arc<dyn CredentialVault>,
        endpoints: OAuthEndpoints,
        browser: Arc<dyn BrowserLauncher>,
    ) -> ProviderResult<Self> {
        Self::build(vault, endpoints, browser, false)
    }

    pub fn credential_generation(&self, profile_id: ProfileId) -> Option<CredentialGeneration> {
        self.lock_state()
            .profiles
            .get(&profile_id)
            .and_then(|connection| connection.generation)
    }

    /// Rehydrate non-sensitive OAuth state after the owning repository has loaded its exact
    /// immutable credential generation. The token bundle remains in the Vault.
    pub async fn restore_connection(
        &self,
        profile_id: ProfileId,
        generation: CredentialGeneration,
    ) -> ProviderResult<OAuthConnectionView> {
        validate_oauth_generation(profile_id, generation)?;
        let lease = self
            .vault
            .read_generation(ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await?;
        let bundle = decode_token_bundle(&lease)?;
        let status = if bundle.expires_at_epoch_seconds <= now_epoch_seconds()? {
            OAuthConnectionStatus::Expired
        } else {
            OAuthConnectionStatus::Connected
        };
        let expires_at_epoch_seconds = bundle.expires_at_epoch_seconds;
        drop(bundle);

        self.lock_state().profiles.insert(
            profile_id,
            ProfileConnection {
                status,
                generation: Some(generation),
                expires_at_epoch_seconds: Some(expires_at_epoch_seconds),
                active_operation: None,
            },
        );
        Ok(connection_view(profile_id, status))
    }

    fn lock_state(&self) -> MutexGuard<'_, ManagerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn begin_authorization(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        {
            let state = self.lock_state();
            if state.pending.contains_key(&operation_id)
                || state
                    .profiles
                    .values()
                    .any(|connection| connection.active_operation == Some(operation_id))
            {
                return Err(stale_operation());
            }
        }

        let response = self
            .http
            .post(&self.endpoints.device_user_code)
            .json(&DeviceUserCodeRequest {
                client_id: CHATGPT_CLIENT_ID,
            })
            .send()
            .await
            .map_err(map_transport_error)?;
        if !response.status().is_success() {
            return Err(map_http_status(response.status()));
        }
        let mut response: DeviceUserCodeResponse = decode_response(response).await?;
        if response.device_auth_id.is_empty() || response.user_code.is_empty() {
            return Err(protocol_error());
        }
        let interval_seconds = response.interval.seconds().clamp(1, 30);
        let pending = Arc::new(PendingDeviceAuthorization {
            profile_id,
            device_auth_id: SensitiveText::new(response.device_auth_id.take()),
            user_code: SensitiveText::new(response.user_code.take()),
            interval: Duration::from_secs(interval_seconds),
            expires_at: Instant::now() + DEVICE_CODE_LIFETIME,
        });
        let verification_uri = self.endpoints.verification.clone();
        let user_code = pending.user_code.expose().to_owned();

        {
            let mut state = self.lock_state();
            if let Some(previous) = state
                .profiles
                .get(&profile_id)
                .and_then(|connection| connection.active_operation)
            {
                state.pending.remove(&previous);
            }
            state.pending.insert(operation_id, pending);
            let previous_generation = state
                .profiles
                .get(&profile_id)
                .and_then(|connection| connection.generation);
            state.profiles.insert(
                profile_id,
                ProfileConnection {
                    status: OAuthConnectionStatus::Pending,
                    generation: previous_generation,
                    expires_at_epoch_seconds: None,
                    active_operation: Some(operation_id),
                },
            );
        }

        let browser = self.browser.clone();
        let browser_uri = verification_uri.clone();
        let _ = tokio::task::spawn_blocking(move || browser.open(&browser_uri)).await;

        Ok(DeviceAuthorizationView {
            verification_uri,
            user_code,
            expires_in_seconds: DEVICE_CODE_LIFETIME.as_secs() as u32,
        })
    }

    async fn complete_authorization(
        &self,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        let pending = {
            let state = self.lock_state();
            let pending = state
                .pending
                .get(&operation_id)
                .cloned()
                .ok_or_else(stale_operation)?;
            ensure_current_operation(&state, pending.profile_id, operation_id)?;
            pending
        };

        let result = self
            .complete_authorization_inner(operation_id, pending.clone())
            .await;
        if result.is_err()
            && result
                .as_ref()
                .err()
                .is_some_and(|error| error.code() != "provider.operation.stale")
        {
            self.finish_failure(
                pending.profile_id,
                operation_id,
                OAuthConnectionStatus::Failed,
            );
        }
        result
    }

    async fn complete_authorization_inner(
        &self,
        operation_id: OperationId,
        pending: Arc<PendingDeviceAuthorization>,
    ) -> ProviderResult<OAuthConnectionView> {
        let mut code_response = self.poll_for_authorization(&pending).await?;
        self.ensure_current(pending.profile_id, operation_id)?;

        if code_response.authorization_code.is_empty()
            || code_response.code_challenge.is_empty()
            || !valid_pkce_verifier(code_response.code_verifier.expose())
        {
            return Err(protocol_error());
        }
        let verifier = PkceCodeVerifier::new(code_response.code_verifier.take());
        if pkce_challenge(verifier.secret()) != code_response.code_challenge.expose() {
            return Err(protocol_error());
        }
        let authorization_code = AuthorizationCode::new(code_response.authorization_code.take());
        let bundle = self
            .exchange_authorization_code(authorization_code, verifier)
            .await?;
        self.ensure_current(pending.profile_id, operation_id)?;

        let generation_number = self.next_generation(pending.profile_id)?;
        let generation = CredentialGeneration::new(
            pending.profile_id,
            generation_number,
            CredentialKind::OAuthConnection,
        )
        .map_err(|_| internal_error())?;
        let expires_at_epoch_seconds = bundle.expires_at_epoch_seconds;
        self.write_bundle(pending.profile_id, generation, bundle)
            .await?;

        if let Err(error) = self.ensure_current(pending.profile_id, operation_id) {
            let _ = self
                .vault
                .delete_generation(ProviderCredentialReference {
                    profile_id: pending.profile_id,
                    generation,
                })
                .await;
            return Err(error);
        }

        let mut state = self.lock_state();
        ensure_current_operation(&state, pending.profile_id, operation_id)?;
        state.pending.remove(&operation_id);
        state.profiles.insert(
            pending.profile_id,
            ProfileConnection {
                status: OAuthConnectionStatus::Connected,
                generation: Some(generation),
                expires_at_epoch_seconds: Some(expires_at_epoch_seconds),
                active_operation: None,
            },
        );
        Ok(connection_view(
            pending.profile_id,
            OAuthConnectionStatus::Connected,
        ))
    }

    async fn poll_for_authorization(
        &self,
        pending: &PendingDeviceAuthorization,
    ) -> ProviderResult<DeviceCodeSuccessResponse> {
        loop {
            if Instant::now() >= pending.expires_at {
                return Err(timeout_error());
            }
            let response = self
                .http
                .post(&self.endpoints.device_poll)
                .json(&DevicePollRequest {
                    device_auth_id: pending.device_auth_id.expose(),
                    user_code: pending.user_code.expose(),
                })
                .send()
                .await
                .map_err(map_transport_error)?;
            match response.status() {
                status if status.is_success() => return decode_response(response).await,
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
                    tokio::time::sleep(pending.interval).await;
                }
                status => return Err(map_http_status(status)),
            }
        }
    }

    async fn exchange_authorization_code(
        &self,
        authorization_code: AuthorizationCode,
        verifier: PkceCodeVerifier,
    ) -> ProviderResult<TokenBundle> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", authorization_code.secret()),
                ("redirect_uri", self.endpoints.device_callback.as_str()),
                ("client_id", CHATGPT_CLIENT_ID),
                ("code_verifier", verifier.secret()),
            ])
            .send()
            .await;
        let mut authorization_code = authorization_code.into_secret();
        authorization_code.zeroize();
        let mut verifier = verifier.into_secret();
        verifier.zeroize();
        let response = response.map_err(map_transport_error)?;
        if !response.status().is_success() {
            return Err(map_http_status(response.status()));
        }
        let response: TokenResponse = decode_response(response).await?;
        token_bundle_from_initial_response(response)
    }

    fn next_generation(&self, profile_id: ProfileId) -> ProviderResult<u64> {
        self.lock_state()
            .profiles
            .get(&profile_id)
            .and_then(|connection| connection.generation)
            .map_or(Ok(1), |generation| {
                generation
                    .number()
                    .checked_add(1)
                    .ok_or_else(internal_error)
            })
    }

    async fn write_bundle(
        &self,
        profile_id: ProfileId,
        generation: CredentialGeneration,
        bundle: TokenBundle,
    ) -> ProviderResult<()> {
        let mut serialized =
            Zeroizing::new(serde_json::to_string(&bundle).map_err(|_| internal_error())?);
        drop(bundle);
        self.vault
            .write_generation(ProtectedCredentialWrite {
                reference: ProviderCredentialReference {
                    profile_id,
                    generation,
                },
                secret: SecretValue::from_utf8(std::mem::take(&mut *serialized)),
            })
            .await
    }

    fn ensure_current(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<()> {
        ensure_current_operation(&self.lock_state(), profile_id, operation_id)
    }

    fn finish_failure(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
        status: OAuthConnectionStatus,
    ) {
        let mut state = self.lock_state();
        let is_current = state
            .profiles
            .get(&profile_id)
            .is_some_and(|connection| connection.active_operation == Some(operation_id));
        if is_current {
            state.pending.remove(&operation_id);
            if let Some(connection) = state.profiles.get_mut(&profile_id) {
                connection.status = status;
                connection.active_operation = None;
                connection.expires_at_epoch_seconds = None;
            }
        }
    }

    async fn refresh_connection(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        let generation = {
            let mut state = self.lock_state();
            let connection = state
                .profiles
                .get_mut(&profile_id)
                .ok_or_else(oauth_not_connected)?;
            if !matches!(
                connection.status,
                OAuthConnectionStatus::Connected | OAuthConnectionStatus::Expired
            ) {
                return Err(oauth_not_connected());
            }
            connection.active_operation = Some(operation_id);
            connection.generation.ok_or_else(oauth_not_connected)?
        };

        let result = self
            .refresh_connection_inner(profile_id, operation_id, generation)
            .await;
        if result.is_err()
            && result
                .as_ref()
                .err()
                .is_some_and(|error| error.code() != "provider.operation.stale")
        {
            let status = self.lock_state().profiles.get(&profile_id).map_or(
                OAuthConnectionStatus::Failed,
                |connection| {
                    if matches!(
                        connection.status,
                        OAuthConnectionStatus::Expired | OAuthConnectionStatus::Revoked
                    ) {
                        connection.status
                    } else {
                        OAuthConnectionStatus::Failed
                    }
                },
            );
            self.finish_failure(profile_id, operation_id, status);
        }
        result
    }

    async fn refresh_connection_inner(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
        generation: CredentialGeneration,
    ) -> ProviderResult<OAuthConnectionView> {
        let lease = self
            .vault
            .read_generation(ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await?;
        let mut bundle = decode_token_bundle(&lease)?;
        let refresh_token = RefreshToken::new(bundle.refresh_token.expose().to_owned());
        if refresh_token.secret().is_empty() {
            return Err(oauth_not_connected());
        }
        self.ensure_current(profile_id, operation_id)?;

        let response = self
            .http
            .post(&self.endpoints.token)
            .json(&RefreshRequest {
                client_id: CHATGPT_CLIENT_ID,
                grant_type: "refresh_token",
                refresh_token: refresh_token.secret(),
            })
            .send()
            .await;
        let mut refresh_token = refresh_token.into_secret();
        refresh_token.zeroize();
        let response = response.map_err(map_transport_error)?;
        self.ensure_current(profile_id, operation_id)?;
        if !response.status().is_success() {
            let status = classify_refresh_failure(response).await;
            self.finish_failure(profile_id, operation_id, status);
            return Err(oauth_not_connected());
        }
        let response: TokenResponse = decode_response(response).await?;
        update_bundle_from_refresh(&mut bundle, response)?;
        let next_number = generation
            .number()
            .checked_add(1)
            .ok_or_else(internal_error)?;
        let next_generation =
            CredentialGeneration::new(profile_id, next_number, CredentialKind::OAuthConnection)
                .map_err(|_| internal_error())?;
        let expires_at_epoch_seconds = bundle.expires_at_epoch_seconds;
        self.ensure_current(profile_id, operation_id)?;
        self.write_bundle(profile_id, next_generation, bundle)
            .await?;

        if let Err(error) = self.ensure_current(profile_id, operation_id) {
            let _ = self
                .vault
                .delete_generation(ProviderCredentialReference {
                    profile_id,
                    generation: next_generation,
                })
                .await;
            return Err(error);
        }
        self.lock_state().profiles.insert(
            profile_id,
            ProfileConnection {
                status: OAuthConnectionStatus::Connected,
                generation: Some(next_generation),
                expires_at_epoch_seconds: Some(expires_at_epoch_seconds),
                active_operation: None,
            },
        );
        Ok(connection_view(
            profile_id,
            OAuthConnectionStatus::Connected,
        ))
    }

    async fn logout_connection(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<RemoteRevocationOutcome> {
        let generation = {
            let mut state = self.lock_state();
            let Some(connection) = state.profiles.get_mut(&profile_id) else {
                return Ok(RemoteRevocationOutcome::Revoked);
            };
            let Some(generation) = connection.generation else {
                connection.status = OAuthConnectionStatus::Revoked;
                return Ok(RemoteRevocationOutcome::Revoked);
            };
            connection.active_operation = Some(operation_id);
            generation
        };
        let lease = self
            .vault
            .read_generation(ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await?;
        let bundle = decode_token_bundle(&lease)?;
        self.ensure_current(profile_id, operation_id)?;
        self.vault
            .delete_generation(ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await?;
        {
            let mut state = self.lock_state();
            ensure_current_operation(&state, profile_id, operation_id)?;
            state.profiles.insert(
                profile_id,
                ProfileConnection {
                    status: OAuthConnectionStatus::Revoked,
                    generation: None,
                    expires_at_epoch_seconds: None,
                    active_operation: None,
                },
            );
        }

        let response = self
            .http
            .post(&self.endpoints.revoke)
            .json(&RevokeRequest {
                token: bundle.refresh_token.expose(),
                token_type_hint: "refresh_token",
                client_id: CHATGPT_CLIENT_ID,
            })
            .send()
            .await;
        drop(bundle);
        match response {
            Ok(response) if response.status().is_success() => Ok(RemoteRevocationOutcome::Revoked),
            _ => Ok(RemoteRevocationOutcome::ResidualRisk {
                remediation: ProviderRemediation::ContactSupport,
            }),
        }
    }
}

impl fmt::Debug for ChatGptOAuthManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock_state();
        formatter
            .debug_struct("ChatGptOAuthManager")
            .field("profile_count", &state.profiles.len())
            .field("pending_count", &state.pending.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl OAuthConnectionService for ChatGptOAuthManager {
    async fn view(&self, profile_id: ProfileId) -> ProviderResult<OAuthConnectionView> {
        let now = now_epoch_seconds()?;
        let mut state = self.lock_state();
        let Some(connection) = state.profiles.get_mut(&profile_id) else {
            return Ok(connection_view(profile_id, OAuthConnectionStatus::Failed));
        };
        if connection.status == OAuthConnectionStatus::Connected
            && connection
                .expires_at_epoch_seconds
                .is_some_and(|expires_at| expires_at <= now)
        {
            connection.status = OAuthConnectionStatus::Expired;
        }
        Ok(connection_view(profile_id, connection.status))
    }

    async fn start(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        self.begin_authorization(profile_id, operation_id).await
    }

    async fn complete(&self, operation_id: OperationId) -> ProviderResult<OAuthConnectionView> {
        self.complete_authorization(operation_id).await
    }

    async fn refresh(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        self.refresh_connection(profile_id, operation_id).await
    }

    async fn reauthorize(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        self.begin_authorization(profile_id, operation_id).await
    }

    async fn logout(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<RemoteRevocationOutcome> {
        self.logout_connection(profile_id, operation_id).await
    }
}

trait BrowserLauncher: Send + Sync {
    fn open(&self, uri: &str);
}

struct SystemBrowserLauncher;

impl BrowserLauncher for SystemBrowserLauncher {
    fn open(&self, uri: &str) {
        let _ = webbrowser::open(uri);
    }
}

#[derive(Clone)]
struct OAuthEndpoints {
    device_user_code: String,
    device_poll: String,
    verification: String,
    token: String,
    revoke: String,
    device_callback: String,
}

impl OAuthEndpoints {
    fn production() -> Self {
        Self {
            device_user_code: CHATGPT_DEVICE_USER_CODE_ENDPOINT.to_owned(),
            device_poll: CHATGPT_DEVICE_POLL_ENDPOINT.to_owned(),
            verification: CHATGPT_VERIFICATION_URI.to_owned(),
            token: CHATGPT_TOKEN_ENDPOINT.to_owned(),
            revoke: CHATGPT_REVOKE_ENDPOINT.to_owned(),
            device_callback: CHATGPT_DEVICE_CALLBACK.to_owned(),
        }
    }

    #[cfg(test)]
    fn for_test(origin: &str) -> Self {
        let origin = origin.trim_end_matches('/');
        Self {
            device_user_code: format!("{origin}/api/accounts/deviceauth/usercode"),
            device_poll: format!("{origin}/api/accounts/deviceauth/token"),
            verification: format!("{origin}/codex/device"),
            token: format!("{origin}/oauth/token"),
            revoke: format!("{origin}/oauth/revoke"),
            device_callback: format!("{origin}/deviceauth/callback"),
        }
    }
}

#[derive(Default)]
struct ManagerState {
    profiles: HashMap<ProfileId, ProfileConnection>,
    pending: HashMap<OperationId, Arc<PendingDeviceAuthorization>>,
}

struct ProfileConnection {
    status: OAuthConnectionStatus,
    generation: Option<CredentialGeneration>,
    expires_at_epoch_seconds: Option<u64>,
    active_operation: Option<OperationId>,
}

struct PendingDeviceAuthorization {
    profile_id: ProfileId,
    device_auth_id: SensitiveText,
    user_code: SensitiveText,
    interval: Duration,
    expires_at: Instant,
}

#[derive(Serialize)]
struct DeviceUserCodeRequest<'a> {
    client_id: &'a str,
}

#[derive(Deserialize)]
struct DeviceUserCodeResponse {
    device_auth_id: SensitiveText,
    #[serde(alias = "usercode")]
    user_code: SensitiveText,
    #[serde(default)]
    interval: PollInterval,
}

#[derive(Default, Deserialize)]
#[serde(untagged)]
enum PollInterval {
    Number(u64),
    Text(String),
    #[default]
    Missing,
}

impl PollInterval {
    fn seconds(&self) -> u64 {
        match self {
            Self::Number(value) => *value,
            Self::Text(value) => value.trim().parse().unwrap_or(5),
            Self::Missing => 5,
        }
    }
}

#[derive(Serialize)]
struct DevicePollRequest<'a> {
    device_auth_id: &'a str,
    user_code: &'a str,
}

#[derive(Deserialize)]
struct DeviceCodeSuccessResponse {
    authorization_code: SensitiveText,
    code_challenge: SensitiveText,
    code_verifier: SensitiveText,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token_type: Option<String>,
    access_token: Option<SensitiveText>,
    refresh_token: Option<SensitiveText>,
    id_token: Option<SensitiveText>,
    expires_in: Option<u64>,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: &'a str,
}

#[derive(Serialize)]
struct RevokeRequest<'a> {
    token: &'a str,
    token_type_hint: &'a str,
    client_id: &'a str,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenBundle {
    schema_version: u32,
    access_token: SensitiveText,
    refresh_token: SensitiveText,
    id_token: SensitiveText,
    account_id: SensitiveText,
    subject: Option<SensitiveText>,
    expires_at_epoch_seconds: u64,
}

struct SensitiveText(String);

impl SensitiveText {
    fn new(value: String) -> Self {
        Self(value)
    }

    fn expose(&self) -> &str {
        &self.0
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn take(&mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl Drop for SensitiveText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Serialize for SensitiveText {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SensitiveText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}

#[derive(Deserialize)]
struct IdTokenClaims {
    iss: String,
    aud: Audience,
    sub: Option<SensitiveText>,
    exp: u64,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: IdTokenAuthClaims,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, expected: &str) -> bool {
        match self {
            Self::One(value) => value == expected,
            Self::Many(values) => values.iter().any(|value| value == expected),
        }
    }
}

#[derive(Deserialize)]
struct IdTokenAuthClaims {
    chatgpt_account_id: SensitiveText,
}

struct ParsedIdentity {
    account_id: SensitiveText,
    subject: Option<SensitiveText>,
    expires_at_epoch_seconds: u64,
}

fn token_bundle_from_initial_response(mut response: TokenResponse) -> ProviderResult<TokenBundle> {
    validate_token_type(response.token_type.as_deref())?;
    let mut access = response.access_token.take().ok_or_else(protocol_error)?;
    let mut refresh = response.refresh_token.take().ok_or_else(protocol_error)?;
    let mut id_token = response.id_token.take().ok_or_else(protocol_error)?;
    if access.is_empty() || refresh.is_empty() || id_token.is_empty() {
        return Err(protocol_error());
    }
    let access = AccessToken::new(access.take());
    let refresh = RefreshToken::new(refresh.take());
    let identity = parse_id_token(id_token.expose())?;
    let now = now_epoch_seconds()?;
    let expires_at_epoch_seconds = response
        .expires_in
        .and_then(|seconds| now.checked_add(seconds))
        .unwrap_or(identity.expires_at_epoch_seconds);
    if expires_at_epoch_seconds <= now {
        return Err(oauth_not_connected());
    }
    Ok(TokenBundle {
        schema_version: TOKEN_BUNDLE_VERSION,
        access_token: SensitiveText::new(access.into_secret()),
        refresh_token: SensitiveText::new(refresh.into_secret()),
        id_token: SensitiveText::new(id_token.take()),
        account_id: identity.account_id,
        subject: identity.subject,
        expires_at_epoch_seconds,
    })
}

fn update_bundle_from_refresh(
    bundle: &mut TokenBundle,
    mut response: TokenResponse,
) -> ProviderResult<()> {
    validate_token_type(response.token_type.as_deref())?;
    let mut access_token = response.access_token.take().ok_or_else(protocol_error)?;
    if access_token.is_empty() {
        return Err(protocol_error());
    }
    if let Some(mut id_token) = response.id_token.take() {
        let identity = parse_id_token(id_token.expose())?;
        if identity.account_id.expose() != bundle.account_id.expose() {
            return Err(oauth_not_connected());
        }
        bundle.id_token = SensitiveText::new(id_token.take());
        bundle.subject = identity.subject;
        bundle.expires_at_epoch_seconds = identity.expires_at_epoch_seconds;
    }
    bundle.access_token = SensitiveText::new(access_token.take());
    if let Some(mut refresh_token) = response.refresh_token.take() {
        if refresh_token.is_empty() {
            return Err(protocol_error());
        }
        bundle.refresh_token = SensitiveText::new(refresh_token.take());
    }
    if let Some(expires_in) = response.expires_in {
        bundle.expires_at_epoch_seconds = now_epoch_seconds()?
            .checked_add(expires_in)
            .ok_or_else(protocol_error)?;
    }
    if bundle.access_token.is_empty()
        || bundle.refresh_token.is_empty()
        || bundle.expires_at_epoch_seconds <= now_epoch_seconds()?
    {
        return Err(oauth_not_connected());
    }
    Ok(())
}

fn validate_token_type(token_type: Option<&str>) -> ProviderResult<()> {
    if token_type.is_some_and(|value| !value.eq_ignore_ascii_case("bearer")) {
        Err(protocol_error())
    } else {
        Ok(())
    }
}

fn parse_id_token(id_token: &str) -> ProviderResult<ParsedIdentity> {
    let mut segments = id_token.split('.');
    let _header = segments.next().ok_or_else(protocol_error)?;
    let payload = segments.next().ok_or_else(protocol_error)?;
    let _signature = segments.next().ok_or_else(protocol_error)?;
    if segments.next().is_some() {
        return Err(protocol_error());
    }
    let decoded = base64_url_decode(payload)?;
    let claims: IdTokenClaims = serde_json::from_slice(&decoded).map_err(|_| protocol_error())?;
    if claims.iss != CHATGPT_AUTH_ORIGIN
        || !claims.aud.contains(CHATGPT_CLIENT_ID)
        || claims.auth.chatgpt_account_id.is_empty()
    {
        return Err(protocol_error());
    }
    Ok(ParsedIdentity {
        account_id: claims.auth.chatgpt_account_id,
        subject: claims.sub,
        expires_at_epoch_seconds: claims.exp,
    })
}

fn decode_token_bundle(lease: &CredentialLease) -> ProviderResult<TokenBundle> {
    let bundle = lease.with_secret(|secret| {
        secret.with_exposed(|serialized| {
            serde_json::from_str::<TokenBundle>(serialized).map_err(|_| protocol_error())
        })
    })?;
    if bundle.schema_version != TOKEN_BUNDLE_VERSION
        || bundle.access_token.is_empty()
        || bundle.refresh_token.is_empty()
        || bundle.id_token.is_empty()
        || bundle.account_id.is_empty()
    {
        return Err(protocol_error());
    }
    let identity = parse_id_token(bundle.id_token.expose())?;
    if identity.account_id.expose() != bundle.account_id.expose() {
        return Err(protocol_error());
    }
    drop(identity);
    Ok(bundle)
}

/// Exposes a connected ChatGPT OAuth bundle only while an adapter constructs a fixed Responses
/// client. The token and account identifier cannot be returned to application code or views.
#[allow(
    dead_code,
    reason = "The dependency-ordered Liter factory consumes this bridge in task 3.7."
)]
pub(crate) fn with_connected_chatgpt_responses_auth<T>(
    lease: &CredentialLease,
    use_auth: impl FnOnce(&str, &str) -> ProviderResult<T>,
) -> ProviderResult<T> {
    let bundle = decode_token_bundle(lease).map_err(|_| oauth_not_connected())?;
    if bundle.expires_at_epoch_seconds <= now_epoch_seconds().map_err(|_| oauth_not_connected())?
        || bundle.access_token.expose().trim() != bundle.access_token.expose()
        || bundle.account_id.expose().trim().is_empty()
        || bundle.account_id.expose().trim() != bundle.account_id.expose()
    {
        return Err(oauth_not_connected());
    }
    use_auth(bundle.access_token.expose(), bundle.account_id.expose())
}

async fn decode_response<T>(response: reqwest::Response) -> ProviderResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = response.bytes().await.map_err(map_transport_error)?;
    if bytes.len() > MAX_OAUTH_RESPONSE_BYTES {
        return Err(protocol_error());
    }
    let raw = Zeroizing::new(bytes.to_vec());
    serde_json::from_slice(&raw).map_err(|_| protocol_error())
}

async fn classify_refresh_failure(response: reqwest::Response) -> OAuthConnectionStatus {
    #[derive(Deserialize)]
    struct ErrorEnvelope {
        error: Option<ErrorValue>,
        code: Option<SensitiveText>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ErrorValue {
        Text(SensitiveText),
        Object { code: Option<SensitiveText> },
    }

    let parsed = decode_response::<ErrorEnvelope>(response).await.ok();
    let code = parsed.as_ref().and_then(|envelope| {
        envelope
            .code
            .as_ref()
            .map(SensitiveText::expose)
            .or(match &envelope.error {
                Some(ErrorValue::Text(code)) => Some(code.expose()),
                Some(ErrorValue::Object { code }) => code.as_ref().map(SensitiveText::expose),
                None => None,
            })
    });
    match code.map(str::to_ascii_lowercase).as_deref() {
        Some("refresh_token_expired") => OAuthConnectionStatus::Expired,
        Some("refresh_token_invalidated" | "refresh_token_reused" | "invalid_grant") => {
            OAuthConnectionStatus::Revoked
        }
        _ => OAuthConnectionStatus::Failed,
    }
}

fn validate_oauth_generation(
    profile_id: ProfileId,
    generation: CredentialGeneration,
) -> ProviderResult<()> {
    if generation.profile_id() != profile_id || generation.kind() != CredentialKind::OAuthConnection
    {
        Err(protocol_error())
    } else {
        Ok(())
    }
}

fn ensure_current_operation(
    state: &ManagerState,
    profile_id: ProfileId,
    operation_id: OperationId,
) -> ProviderResult<()> {
    if state
        .profiles
        .get(&profile_id)
        .is_some_and(|connection| connection.active_operation == Some(operation_id))
    {
        Ok(())
    } else {
        Err(stale_operation())
    }
}

fn connection_view(profile_id: ProfileId, status: OAuthConnectionStatus) -> OAuthConnectionView {
    let remediation = match status {
        OAuthConnectionStatus::Pending | OAuthConnectionStatus::Connected => None,
        OAuthConnectionStatus::Expired
        | OAuthConnectionStatus::Revoked
        | OAuthConnectionStatus::Failed => Some(ProviderRemediation::Reauthorize),
    };
    OAuthConnectionView {
        profile_id,
        status,
        remediation,
    }
}

fn now_epoch_seconds() -> ProviderResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| internal_error())
}

fn pkce_challenge(verifier: &str) -> String {
    base64_url_encode(&Sha256::digest(verifier.as_bytes()))
}

fn valid_pkce_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

fn base64_url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(third & 0x3f) as usize] as char);
        }
    }
    output
}

fn base64_url_decode(input: &str) -> ProviderResult<Zeroizing<Vec<u8>>> {
    let mut output = Zeroizing::new(Vec::with_capacity(input.len() * 3 / 4));
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err(protocol_error()),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1_u32 << bits).saturating_sub(1);
        }
    }
    if bits >= 6 || (buffer != 0 && bits > 0) {
        return Err(protocol_error());
    }
    Ok(output)
}

fn map_transport_error(error: reqwest::Error) -> ProviderManagementError {
    if error.is_timeout() {
        timeout_error()
    } else {
        ProviderManagementError::new(
            ProviderErrorCode::Network,
            Some(ProviderField::OAuth),
            ProviderRemediation::Retry,
        )
    }
}

fn map_http_status(status: StatusCode) -> ProviderManagementError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        ProviderManagementError::new(
            ProviderErrorCode::Server,
            Some(ProviderField::OAuth),
            ProviderRemediation::Retry,
        )
    } else {
        oauth_not_connected()
    }
}

fn oauth_not_connected() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::OAuthNotConnected,
        Some(ProviderField::OAuth),
        ProviderRemediation::Reauthorize,
    )
}

fn stale_operation() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::OperationStale,
        Some(ProviderField::OAuth),
        ProviderRemediation::WaitForCurrentOperation,
    )
}

fn timeout_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Timeout,
        Some(ProviderField::OAuth),
        ProviderRemediation::Retry,
    )
}

fn protocol_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ProtocolInvalidResponse,
        Some(ProviderField::OAuth),
        ProviderRemediation::Reauthorize,
    )
}

fn internal_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        Some(ProviderField::OAuth),
        ProviderRemediation::ContactSupport,
    )
}

#[cfg(test)]
#[path = "chatgpt_tests.rs"]
mod tests;
