use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{body_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use ys_agent_core::{
    CredentialGeneration, CredentialKind, CredentialVault, OAuthConnectionService,
    OAuthConnectionStatus, OperationId, ProfileId, RemoteRevocationOutcome,
};

use super::{
    BrowserLauncher, CHATGPT_AUTH_ORIGIN, CHATGPT_CLIENT_ID, CHATGPT_DEVICE_CALLBACK,
    CHATGPT_DEVICE_POLL_ENDPOINT, CHATGPT_DEVICE_USER_CODE_ENDPOINT, CHATGPT_REVOKE_ENDPOINT,
    CHATGPT_TOKEN_ENDPOINT, CHATGPT_VERIFICATION_URI, ChatGptOAuthManager, OAuthEndpoints,
    base64_url_encode, decode_token_bundle, pkce_challenge,
};
use crate::credential::memory::InMemoryCredentialVault;

const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

fn oauth_generation(profile_id: ProfileId, number: u64) -> CredentialGeneration {
    CredentialGeneration::new(profile_id, number, CredentialKind::OAuthConnection)
        .expect("valid OAuth generation")
}

#[derive(Default)]
struct RecordingBrowser {
    opened: Mutex<Vec<String>>,
}

impl BrowserLauncher for RecordingBrowser {
    fn open(&self, uri: &str) {
        self.opened
            .lock()
            .expect("browser recorder lock")
            .push(uri.to_owned());
    }
}

#[derive(Default)]
struct BlockingBrowser {
    released: Mutex<bool>,
    release_signal: Condvar,
}

impl BlockingBrowser {
    fn release(&self) {
        *self.released.lock().expect("browser release lock") = true;
        self.release_signal.notify_all();
    }
}

impl BrowserLauncher for BlockingBrowser {
    fn open(&self, _uri: &str) {
        let mut released = self.released.lock().expect("browser release lock");
        while !*released {
            released = self
                .release_signal
                .wait(released)
                .expect("browser release wait");
        }
    }
}

fn fixture_manager(
    server: &MockServer,
) -> (
    ChatGptOAuthManager,
    Arc<InMemoryCredentialVault>,
    Arc<RecordingBrowser>,
) {
    let vault = Arc::new(InMemoryCredentialVault::new());
    let browser = Arc::new(RecordingBrowser::default());
    let manager = ChatGptOAuthManager::with_endpoints_for_test(
        vault.clone(),
        OAuthEndpoints::for_test(&server.uri()),
        browser.clone(),
    )
    .expect("fixture manager");
    (manager, vault, browser)
}

fn id_token(account_id: &str) -> String {
    let header = base64_url_encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64_url_encode(
        serde_json::to_string(&json!({
            "iss": CHATGPT_AUTH_ORIGIN,
            "aud": CHATGPT_CLIENT_ID,
            "sub": "subject-1",
            "exp": 4_102_444_800_u64,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id
            }
        }))
        .expect("JWT claims serialize")
        .as_bytes(),
    );
    format!("{header}.{payload}.fixture-signature")
}

async fn mount_device_start(server: &MockServer, expected: u64) {
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/usercode"))
        .and(body_json(json!({ "client_id": CHATGPT_CLIENT_ID })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_auth_id": "device-auth-id",
            "user_code": "ABCD-1234",
            "interval": "1"
        })))
        .expect(expected)
        .mount(server)
        .await;
}

async fn mount_device_poll(server: &MockServer, delay: Option<Duration>) {
    let mut response = ResponseTemplate::new(200).set_body_json(json!({
        "authorization_code": "authorization-code",
        "code_challenge": pkce_challenge(VERIFIER),
        "code_verifier": VERIFIER
    }));
    if let Some(delay) = delay {
        response = response.set_delay(delay);
    }
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/token"))
        .and(body_json(json!({
            "device_auth_id": "device-auth-id",
            "user_code": "ABCD-1234"
        })))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_exchange(server: &MockServer, access: &str, refresh: &str) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=authorization-code"))
        .and(body_string_contains(format!(
            "client_id={CHATGPT_CLIENT_ID}"
        )))
        .and(body_string_contains(
            "code_verifier=abcdefghijklmnopqrstuvwxyz",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token_type": "bearer",
            "access_token": access,
            "refresh_token": refresh,
            "id_token": id_token("account-1"),
            "expires_in": 3600
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn connect(manager: &ChatGptOAuthManager, server: &MockServer, profile_id: ProfileId) {
    mount_device_start(server, 1).await;
    mount_device_poll(server, None).await;
    mount_exchange(server, "access-one", "refresh-one").await;
    let operation_id = OperationId::new();
    manager
        .start(profile_id, operation_id)
        .await
        .expect("start device authorization");
    let view = manager
        .complete(operation_id, oauth_generation(profile_id, 1))
        .await
        .expect("complete device authorization");
    assert_eq!(view.status, OAuthConnectionStatus::Connected);
}

#[test]
fn production_endpoints_and_client_identity_are_fixed() {
    assert_eq!(CHATGPT_AUTH_ORIGIN, "https://auth.openai.com");
    assert_eq!(CHATGPT_CLIENT_ID, "app_EMoamEEZ73f0CkXaXp7hrann");
    assert_eq!(
        CHATGPT_DEVICE_USER_CODE_ENDPOINT,
        "https://auth.openai.com/api/accounts/deviceauth/usercode"
    );
    assert_eq!(
        CHATGPT_DEVICE_POLL_ENDPOINT,
        "https://auth.openai.com/api/accounts/deviceauth/token"
    );
    assert_eq!(
        CHATGPT_VERIFICATION_URI,
        "https://auth.openai.com/codex/device"
    );
    assert_eq!(
        CHATGPT_TOKEN_ENDPOINT,
        "https://auth.openai.com/oauth/token"
    );
    assert_eq!(
        CHATGPT_REVOKE_ENDPOINT,
        "https://auth.openai.com/oauth/revoke"
    );
    assert_eq!(
        CHATGPT_DEVICE_CALLBACK,
        "https://auth.openai.com/deviceauth/callback"
    );
}

#[tokio::test]
async fn device_flow_opens_fixed_verification_and_writes_one_masked_bundle() {
    let server = MockServer::start().await;
    let (manager, vault, browser) = fixture_manager(&server);
    let profile_id = ProfileId::new();
    mount_device_start(&server, 1).await;
    mount_device_poll(&server, None).await;
    mount_exchange(&server, "access-canary", "refresh-canary").await;

    let operation_id = OperationId::new();
    let authorization = manager
        .start(profile_id, operation_id)
        .await
        .expect("start device authorization");
    assert_eq!(
        authorization.verification_uri,
        format!("{}/codex/device", server.uri())
    );
    assert_eq!(authorization.user_code, "ABCD-1234");
    assert_eq!(authorization.expires_in_seconds, 900);

    let connected = manager
        .complete(operation_id, oauth_generation(profile_id, 1))
        .await
        .expect("complete device authorization");
    assert_eq!(connected.status, OAuthConnectionStatus::Connected);
    assert_eq!(connected.remediation, None);
    let generation = manager
        .credential_generation(profile_id)
        .expect("connected generation");
    assert_eq!(generation.number(), 1);

    let lease = vault
        .read_generation(ys_agent_core::ProviderCredentialReference {
            profile_id,
            generation,
        })
        .await
        .expect("OAuth bundle is in Vault");
    let bundle = decode_token_bundle(&lease).expect("decode protected bundle");
    assert_eq!(bundle.access_token.expose(), "access-canary");
    assert_eq!(bundle.refresh_token.expose(), "refresh-canary");
    assert_eq!(bundle.account_id.expose(), "account-1");
    drop(bundle);

    let restarted = ChatGptOAuthManager::with_endpoints_for_test(
        vault.clone(),
        OAuthEndpoints::for_test(&server.uri()),
        Arc::new(RecordingBrowser::default()),
    )
    .expect("restarted fixture manager");
    assert_eq!(
        restarted
            .restore_connection(profile_id, generation)
            .await
            .expect("restore exact Vault generation")
            .status,
        OAuthConnectionStatus::Connected
    );
    vault
        .delete_generation(ys_agent_core::ProviderCredentialReference {
            profile_id,
            generation,
        })
        .await
        .expect("remove the local OAuth bundle");
    assert_eq!(
        restarted
            .restore_connection(profile_id, generation)
            .await
            .expect("missing bundle is represented as a safe status")
            .status,
        OAuthConnectionStatus::Revoked
    );

    let rendered = format!("{connected:?} {manager:?}");
    assert!(!rendered.contains("access-canary"));
    assert!(!rendered.contains("refresh-canary"));
    assert_eq!(
        browser
            .opened
            .lock()
            .expect("browser recorder lock")
            .as_slice(),
        &[format!("{}/codex/device", server.uri())]
    );
}

#[tokio::test]
async fn device_code_returns_before_a_blocked_browser_launcher_finishes() {
    let server = MockServer::start().await;
    let vault = Arc::new(InMemoryCredentialVault::new());
    let browser = Arc::new(BlockingBrowser::default());
    let manager = ChatGptOAuthManager::with_endpoints_for_test(
        vault,
        OAuthEndpoints::for_test(&server.uri()),
        browser.clone(),
    )
    .expect("fixture manager");
    mount_device_start(&server, 1).await;

    let authorization = tokio::time::timeout(
        Duration::from_millis(500),
        manager.start(ProfileId::new(), OperationId::new()),
    )
    .await
    .expect("the TUI must receive the code without waiting for browser startup")
    .expect("start device authorization");
    browser.release();

    assert_eq!(authorization.user_code, "ABCD-1234");
    assert_eq!(
        authorization.verification_uri,
        format!("{}/codex/device", server.uri())
    );
}

#[tokio::test]
async fn refresh_rotates_generation_and_invalid_refresh_fails_closed_without_echo() {
    let server = MockServer::start().await;
    let (manager, vault, _) = fixture_manager(&server);
    let profile_id = ProfileId::new();
    connect(&manager, &server, profile_id).await;

    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_json(json!({
            "client_id": CHATGPT_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": "refresh-one"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-two",
            "refresh_token": "refresh-two",
            "id_token": id_token("account-1"),
            "expires_in": 7200
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    manager
        .refresh(
            profile_id,
            OperationId::new(),
            oauth_generation(profile_id, 2),
        )
        .await
        .expect("refresh rotates token bundle");
    let rotated = manager
        .credential_generation(profile_id)
        .expect("rotated generation");
    assert_eq!(rotated.number(), 2);
    let lease = vault
        .read_generation(ys_agent_core::ProviderCredentialReference {
            profile_id,
            generation: rotated,
        })
        .await
        .expect("rotated bundle is readable");
    let bundle = decode_token_bundle(&lease).expect("decode rotated bundle");
    assert_eq!(bundle.access_token.expose(), "access-two");
    assert_eq!(bundle.refresh_token.expose(), "refresh-two");

    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "code": "refresh_token_invalidated",
                "message": "server echoed refresh-two and response-canary"
            }
        })))
        .with_priority(10)
        .expect(1)
        .mount(&server)
        .await;
    let error = manager
        .refresh(
            profile_id,
            OperationId::new(),
            oauth_generation(profile_id, 3),
        )
        .await
        .expect_err("invalidated refresh token fails closed");
    assert_eq!(error.code(), "provider.oauth.not_connected");
    assert!(!format!("{error:?}").contains("response-canary"));
    assert_eq!(
        manager.view(profile_id).await.expect("masked view").status,
        OAuthConnectionStatus::Revoked
    );
}

#[tokio::test]
async fn late_device_completion_cannot_overwrite_reauthorization() {
    let server = MockServer::start().await;
    let (manager, _, _) = fixture_manager(&server);
    let profile_id = ProfileId::new();
    mount_device_start(&server, 2).await;
    mount_device_poll(&server, Some(Duration::from_millis(100))).await;

    let first = OperationId::new();
    manager
        .start(profile_id, first)
        .await
        .expect("start first operation");
    let late_manager = manager.clone();
    let late = tokio::spawn(async move {
        late_manager
            .complete(first, oauth_generation(profile_id, 1))
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = OperationId::new();
    manager
        .reauthorize(profile_id, second)
        .await
        .expect("new operation supersedes first");

    let error = late
        .await
        .expect("late task joins")
        .expect_err("late completion is stale");
    assert_eq!(error.code(), "provider.operation.stale");
    assert_eq!(
        manager.view(profile_id).await.expect("masked view").status,
        OAuthConnectionStatus::Pending
    );
    assert_eq!(manager.credential_generation(profile_id), None);
}

#[tokio::test]
async fn logout_deletes_local_bundle_before_reporting_remote_revoke_residual_risk() {
    let server = MockServer::start().await;
    let (manager, vault, _) = fixture_manager(&server);
    let profile_id = ProfileId::new();
    connect(&manager, &server, profile_id).await;
    let generation = manager
        .credential_generation(profile_id)
        .expect("connected generation");

    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .and(body_json(json!({
            "token": "refresh-one",
            "token_type_hint": "refresh_token",
            "client_id": CHATGPT_CLIENT_ID
        })))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("remote echoed refresh-one and revoke-response-canary"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let outcome = manager
        .logout(profile_id, OperationId::new())
        .await
        .expect("local logout succeeds");
    assert!(matches!(
        outcome,
        RemoteRevocationOutcome::ResidualRisk { .. }
    ));
    assert!(!format!("{outcome:?}").contains("revoke-response-canary"));
    assert_eq!(
        manager.view(profile_id).await.expect("masked view").status,
        OAuthConnectionStatus::Revoked
    );
    assert_eq!(
        vault
            .credential_status(ys_agent_core::ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await
            .expect("Vault status"),
        ys_agent_core::CredentialViewStatus::Missing
    );
}
