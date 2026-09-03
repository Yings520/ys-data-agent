//! Allowlist-constrained model discovery for Provider management.
//!
//! Production construction exposes no URL or provider-registry input. The selected core
//! `ProviderId` is mapped to one of eight fixed `liter-llm` providers, and every returned model is
//! normalized back under that same product prefix before it crosses the adapter boundary.

use std::{collections::BTreeSet, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use liter_llm::{ClientBuilder, LlmClient, error::LiterLlmError, types::ModelsListResponse};
use ys_agent_core::{
    CredentialLease, DiscoverModelsRequest, DiscoveredModel, ModelDiscovery, ProviderErrorCode,
    ProviderField, ProviderId, ProviderManagementError, ProviderModelId, ProviderRemediation,
    ProviderResult,
};

use super::liter::provider_base_url;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const OPENCODE_GO_MODELS: [&str; 10] = [
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "kimi-k2.7-code",
    "kimi-k3",
    "kimi-k2.6",
    "glm-5.2",
    "glm-5.1",
    "grok-4.5",
    "mimo-v2.5-pro",
    "mimo-v2.5",
];

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
        if let Some(models) = fixed_plan_models(request.provider) {
            return Ok(models
                .iter()
                .map(|model| discovered_model(request.provider, model))
                .collect());
        }
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
        let base_url =
            provider_base_url(provider).map_err(|_| TransportFailure::InvalidResponse)?;
        let client = credential
            .with_secret(|secret| {
                secret.with_exposed(|api_key| {
                    ClientBuilder::new()
                        .api_key(api_key.to_owned())
                        .provider(provider_hint)
                        .base_url(base_url)
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
        ProviderId::OpenAi => Ok("openai/"),
        ProviderId::Kimi => Ok("moonshot/"),
        ProviderId::Qwen => Ok("dashscope/"),
        ProviderId::Gemini => Ok("gemini/"),
        ProviderId::Glm => Ok("zai/"),
        ProviderId::ChatGptSubscription => Err(error(
            ProviderErrorCode::ProtocolIncompatible,
            Some(ProviderField::Provider),
            ProviderRemediation::ReturnToEdit,
        )),
        ProviderId::ClaudeSubscription
        | ProviderId::AlibabaCoding
        | ProviderId::BigModelCoding
        | ProviderId::ZaiCoding
        | ProviderId::MiniMaxCoding
        | ProviderId::KimiCoding => Ok("anthropic/"),
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
    if fixed_plan_models(request.provider).is_none() {
        fixed_provider_hint(request.provider)?;
    }
    if request.profile_revision == 0 {
        return Err(error(
            ProviderErrorCode::DiscoveryFailed,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ));
    }
    if request.credential_generation.profile_id() != request.profile_id
        || request.credential_generation.kind() != request.provider.required_credential_kind()
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
                .chain(ProviderId::LEGACY)
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

    let models = if provider == ProviderId::OpenCodeGo {
        OPENCODE_GO_MODELS
            .into_iter()
            .map(|model| format!("{prefix}{model}"))
            .filter(|model| models.contains(model))
            .collect::<Vec<_>>()
    } else {
        models.into_iter().collect::<Vec<_>>()
    };

    let models = models
        .into_iter()
        .filter_map(|model| match online_model_evidence(provider, &model) {
            OnlineModelEvidence::Chat { context_limit } => Some(DiscoveredModel {
                model,
                context_limit: Some(context_limit),
            }),
            OnlineModelEvidence::Unknown => Some(DiscoveredModel {
                model,
                context_limit: None,
            }),
            OnlineModelEvidence::Unsupported => None,
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        return Err(error(
            ProviderErrorCode::DiscoveryFailed,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ));
    }

    Ok(models)
}

fn fixed_plan_models(provider: ProviderId) -> Option<&'static [&'static str]> {
    match provider {
        ProviderId::ChatGptSubscription => Some(&["codex-mini-latest"]),
        ProviderId::ClaudeSubscription => Some(&[
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
        ]),
        ProviderId::AlibabaCoding => Some(&[
            "qwen3-coder-plus",
            "qwen3.5-plus",
            "glm-5",
            "kimi-k2.5",
            "MiniMax-M2.5",
        ]),
        ProviderId::BigModelCoding | ProviderId::ZaiCoding => Some(&["glm-5.1", "glm-5"]),
        ProviderId::MiniMaxCoding => Some(&["MiniMax-M2.7"]),
        ProviderId::KimiCoding => Some(&["kimi-for-coding"]),
        _ => None,
    }
}

fn discovered_model(provider: ProviderId, model: &str) -> DiscoveredModel {
    let model = format!("{}{model}", provider.model_prefix());
    let context_limit = known_context_limit(&model);
    DiscoveredModel {
        model,
        context_limit,
    }
}

fn known_context_limit(model: &str) -> Option<u32> {
    let bare_model = model.rsplit('/').next().unwrap_or(model);
    governed_context_limit(bare_model).or_else(|| catalog_context_limit(model))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnlineModelEvidence {
    Chat { context_limit: u32 },
    Unknown,
    Unsupported,
}

fn online_model_evidence(provider: ProviderId, model: &str) -> OnlineModelEvidence {
    if provider == ProviderId::OpenCodeGo {
        // The product catalog explicitly curates the Go plan's Chat-compatible subset.
        return known_context_limit(model)
            .map_or(OnlineModelEvidence::Unsupported, |context_limit| {
                OnlineModelEvidence::Chat { context_limit }
            });
    }
    let bare_model = model.rsplit('/').next().unwrap_or(model);
    if let Some(limit) = governed_context_limit(bare_model) {
        return OnlineModelEvidence::Chat {
            context_limit: limit,
        };
    }
    let Some(info) = liter_llm::cost::model_info(model) else {
        return OnlineModelEvidence::Unknown;
    };
    if info.mode.as_deref() != Some("chat") || info.supports_function_calling != Some(true) {
        return OnlineModelEvidence::Unsupported;
    }
    info.max_tokens
        .and_then(|limit| u32::try_from(limit).ok())
        .map_or(OnlineModelEvidence::Unknown, |context_limit| {
            OnlineModelEvidence::Chat { context_limit }
        })
}

fn catalog_context_limit(model: &str) -> Option<u32> {
    liter_llm::cost::model_info(model)
        .and_then(|info| info.max_tokens)
        .and_then(|limit| u32::try_from(limit).ok())
}

fn governed_context_limit(model: &str) -> Option<u32> {
    match model {
        "gpt-5.5" | "gpt-5.4" => Some(1_050_000),
        "gpt-5.4-mini" | "gpt-5.4-nano" => Some(400_000),
        "o3" | "o3-pro" | "o4-mini" => Some(200_000),
        "deepseek-v4-pro" | "deepseek-v4-flash" => Some(1_048_576),
        "kimi-k2.6" | "kimi-k2.5" | "kimi-k2-thinking" | "kimi-for-coding" => Some(256_000),
        "qwen3.6-plus" | "qwen3.6-flash" | "qwen3.5-plus" | "qwen3-coder-plus" => Some(1_000_000),
        "qwen3-coder-next" => Some(262_144),
        "glm-5.1" | "glm-5-turbo" => Some(202_752),
        "glm-5" | "glm-4.7" => Some(200_000),
        "glm-4.5-air" | "glm-4.5-flash" => Some(128_000),
        "MiniMax-M2.5" | "MiniMax-M2.7" => Some(204_800),
        "claude-opus-4-7" | "claude-opus-4-6" | "claude-sonnet-4-6" => Some(1_000_000),
        "claude-haiku-4-5" => Some(200_000),
        "gemini-3.1-pro-preview"
        | "gemini-3.1-flash-lite-preview"
        | "gemini-2.5-pro"
        | "gemini-2.5-flash" => Some(1_048_576),
        "codex-mini-latest" => Some(192_000),
        _ => None,
    }
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
