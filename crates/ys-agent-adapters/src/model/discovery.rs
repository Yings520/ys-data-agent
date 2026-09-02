//! Allowlist-constrained model discovery for Provider management.
//!
//! Production construction exposes no URL or provider-registry input. The selected core
//! `ProviderId` is mapped to one of eight fixed `liter-llm` providers, and every returned model is
//! normalized back under that same product prefix before it crosses the adapter boundary.

use std::{collections::BTreeSet, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use liter_llm::{ClientBuilder, LlmClient, error::LiterLlmError, types::ModelsListResponse};
use ys_agent_core::{
    CredentialKind, CredentialLease, DiscoverModelsRequest, DiscoveredModel, ModelDiscovery,
    ProviderErrorCode, ProviderField, ProviderId, ProviderManagementError, ProviderModelId,
    ProviderRemediation, ProviderResult,
};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Production model discovery. Its only configurable dependency is private and test-only; callers
/// cannot provide a base URL or extend the Provider allowlist.
#[derive(Clone)]
pub struct LiterModelDiscovery {
    transport: Arc<dyn DiscoveryTransport>,
}

impl Default for LiterModelDiscovery {
    fn default() -> Self {
        Self {
            transport: Arc::new(LiterDiscoveryTransport),
        }
    }
}

impl fmt::Debug for LiterModelDiscovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiterModelDiscovery")
            .field("transport", &"fixed-liter-provider")
            .finish()
    }
}

impl LiterModelDiscovery {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn with_transport(transport: Arc<dyn DiscoveryTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl ModelDiscovery for LiterModelDiscovery {
    async fn discover(
        &self,
        request: DiscoverModelsRequest,
        credential: CredentialLease,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        validate_request(&request)?;
        let response = self
            .transport
            .list_models(request.provider, credential)
            .await
            .map_err(map_transport_failure)?;
        normalize_models(request.provider, response)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportFailure {
    Authentication,
    RateLimited,
    Timeout,
    Network,
    Server,
    InvalidResponse,
}

#[async_trait]
trait DiscoveryTransport: Send + Sync {
    async fn list_models(
        &self,
        provider: ProviderId,
        credential: CredentialLease,
    ) -> Result<ModelsListResponse, TransportFailure>;
}

#[derive(Debug, Clone, Copy)]
struct LiterDiscoveryTransport;

#[async_trait]
impl DiscoveryTransport for LiterDiscoveryTransport {
    async fn list_models(
        &self,
        provider: ProviderId,
        credential: CredentialLease,
    ) -> Result<ModelsListResponse, TransportFailure> {
        let provider_hint =
            fixed_provider_hint(provider).map_err(|_| TransportFailure::InvalidResponse)?;
        let client = credential
            .with_secret(|secret| {
                secret.with_exposed(|api_key| {
                    ClientBuilder::new()
                        .api_key(api_key.to_owned())
                        .provider(provider_hint)
                        .load_env(false)
                        .timeout(DISCOVERY_TIMEOUT)
                        .max_retries(0)
                        .build()
                })
            })
            .map_err(classify_liter_error)?;

        client.list_models().await.map_err(classify_liter_error)
    }
}

fn fixed_provider_hint(provider: ProviderId) -> ProviderResult<&'static str> {
    match provider {
        ProviderId::ChatGptSubscription => Err(error(
            ProviderErrorCode::ProtocolIncompatible,
            Some(ProviderField::Provider),
            ProviderRemediation::ReturnToEdit,
        )),
        ProviderId::OpenCodeGo => Ok("opencode-go/"),
        ProviderId::OpenCodeZen => Ok("opencode/"),
        ProviderId::DeepSeek => Ok("deepseek/"),
        ProviderId::Xai => Ok("xai/"),
        ProviderId::Zai => Ok("zai/"),
        ProviderId::OpenRouter => Ok("openrouter/"),
        ProviderId::MiniMax => Ok("minimax/"),
        ProviderId::Anthropic => Ok("anthropic/"),
    }
}

fn validate_request(request: &DiscoverModelsRequest) -> ProviderResult<()> {
    fixed_provider_hint(request.provider)?;
    if request.profile_revision == 0 {
        return Err(error(
            ProviderErrorCode::DiscoveryFailed,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ));
    }
    if request.credential_generation.profile_id() != request.profile_id
        || request.credential_generation.kind() != CredentialKind::ApiKey
        || request.provider.required_credential_kind() != CredentialKind::ApiKey
    {
        return Err(error(
            ProviderErrorCode::AuthenticationInvalid,
            Some(ProviderField::Credential),
            ProviderRemediation::ReturnToEdit,
        ));
    }
    Ok(())
}

fn normalize_models(
    provider: ProviderId,
    response: ModelsListResponse,
) -> ProviderResult<Vec<DiscoveredModel>> {
    let prefix = provider.model_prefix();
    let mut models = BTreeSet::new();

    for candidate in response.data {
        let id = candidate.id;
        if id.is_empty() || id.trim() != id || id.len() > 512 || id.chars().any(char::is_whitespace)
        {
            continue;
        }
        let normalized = if id.starts_with(prefix) {
            id
        } else if !provider_allows_namespaced_model_ids(provider)
            && ProviderId::ALL
                .into_iter()
                .filter(|other| *other != provider)
                .any(|other| id.starts_with(other.model_prefix()))
        {
            continue;
        } else {
            format!("{prefix}{id}")
        };
        if ProviderModelId::new(provider, normalized.clone()).is_ok() {
            models.insert(normalized);
        }
    }

    if models.is_empty() {
        return Err(error(
            ProviderErrorCode::DiscoveryFailed,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ));
    }

    Ok(models
        .into_iter()
        .map(|model| DiscoveredModel {
            model,
            // The list-models protocol has no trustworthy portable context-limit field.
            // Compatibility probing owns that evidence instead of guessing here.
            context_limit: None,
        })
        .collect())
}

const fn provider_allows_namespaced_model_ids(provider: ProviderId) -> bool {
    matches!(
        provider,
        ProviderId::OpenCodeGo | ProviderId::OpenCodeZen | ProviderId::OpenRouter
    )
}

fn classify_liter_error(error: LiterLlmError) -> TransportFailure {
    match error {
        LiterLlmError::Authentication { .. } => TransportFailure::Authentication,
        LiterLlmError::RateLimited { .. } => TransportFailure::RateLimited,
        LiterLlmError::Timeout => TransportFailure::Timeout,
        LiterLlmError::Network(_) => TransportFailure::Network,
        LiterLlmError::ServerError { .. } | LiterLlmError::ServiceUnavailable { .. } => {
            TransportFailure::Server
        }
        _ => TransportFailure::InvalidResponse,
    }
}

fn map_transport_failure(failure: TransportFailure) -> ProviderManagementError {
    let (code, field, remediation) = match failure {
        TransportFailure::Authentication => (
            ProviderErrorCode::AuthenticationInvalid,
            ProviderField::Credential,
            ProviderRemediation::ReturnToEdit,
        ),
        TransportFailure::RateLimited => (
            ProviderErrorCode::RateLimited,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        TransportFailure::Timeout => (
            ProviderErrorCode::Timeout,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        TransportFailure::Network => (
            ProviderErrorCode::Network,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        TransportFailure::Server => (
            ProviderErrorCode::Server,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        TransportFailure::InvalidResponse => (
            ProviderErrorCode::DiscoveryFailed,
            ProviderField::Model,
            ProviderRemediation::ReturnToEdit,
        ),
    };
    error(code, Some(field), remediation)
}

const fn error(
    code: ProviderErrorCode,
    field: Option<ProviderField>,
    remediation: ProviderRemediation,
) -> ProviderManagementError {
    ProviderManagementError::new(code, field, remediation)
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod discovery_tests;
