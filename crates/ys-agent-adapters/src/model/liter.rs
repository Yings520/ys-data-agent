//! Governed `liter-llm` client factory.
//!
//! This is the only production module that constructs a `DefaultClient`. It accepts a bound core
//! Provider selection and scoped lease, and has no custom URL, provider-registry, environment, or
//! fallback input.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use liter_llm::{
    LlmClient, ResponseClient,
    auth::StaticTokenProvider,
    client::{ClientConfig, ClientConfigBuilder, DefaultClient},
    error::LiterLlmError,
};
use secrecy::SecretString;
use ys_agent_core::{
    CoreError, CoreResult, CredentialLease, ModelCapabilities, ModelProvider, ModelRequest,
    ModelResponse, ParameterApplicability, ProviderClientBinding, ProviderClientFactory,
    ProviderErrorCode, ProviderField, ProviderId, ProviderManagementError, ProviderParameterKey,
    ProviderRemediation, ProviderResult,
};

use super::{
    liter_chat::LiterChatCodec, liter_responses::ChatGptResponsesCodec, required_capabilities,
};

const MAX_BOUND_RETRIES: u32 = 2;
const CANDIDATE_CONTEXT_LIMIT: u64 = 128_000;
const CLAUDE_SUBSCRIPTION_BETAS: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,prompt-caching-scope-2026-01-05,web-fetch-2025-09-10";

/// The only production client factory for the nine governed Provider paths.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiterProviderFactory;

/// A private, non-serializable plan that lets the factory fully validate the binding before a
/// third-party client exists. It is also the seam used by offline contract tests.
enum ClientPlan {
    Chat {
        model_hint: String,
        model: String,
        temperature: Option<f32>,
        config: ClientConfig,
        codec: LiterChatCodec,
    },
    Responses {
        model_hint: String,
        model: String,
        temperature: Option<f32>,
        config: ClientConfig,
        codec: ChatGptResponsesCodec,
    },
}

impl LiterProviderFactory {
    pub const fn new() -> Self {
        Self
    }

    fn build_plan(
        &self,
        binding: ProviderClientBinding,
        credential: CredentialLease,
    ) -> ProviderResult<ClientPlan> {
        validate_binding(&binding)?;
        let provider = binding.provider;
        let model = binding.model.as_str().to_owned();
        let timeout = Duration::from_secs(u64::from(binding.parameters.timeout_seconds()));
        let retry_count = binding.parameters.retry_count();

        if provider == ProviderId::ChatGptSubscription {
            let codec = ChatGptResponsesCodec::new(ParameterApplicability::Conditional);
            let mut config = codec.client_config(&credential)?;
            config.timeout = timeout;
            config.max_retries = retry_count;
            return Ok(ClientPlan::Responses {
                model_hint: model.clone(),
                model,
                temperature: binding.parameters.temperature(),
                config,
                codec,
            });
        }

        let codec = LiterChatCodec::new(provider, ParameterApplicability::Conditional)?;
        let config = credential.with_secret(|secret| {
            secret.with_exposed(|api_key| {
                if api_key.trim().is_empty() || api_key.trim() != api_key {
                    return Err(authentication_invalid());
                }
                let mut builder = ClientConfigBuilder::new(api_key)
                    .load_env(false)
                    .timeout(timeout)
                    .max_retries(retry_count);
                if provider == ProviderId::ClaudeSubscription {
                    builder = ClientConfigBuilder::new("")
                        .load_env(false)
                        .timeout(timeout)
                        .max_retries(retry_count)
                        .credential_provider(Arc::new(StaticTokenProvider::new(
                            SecretString::from(api_key.to_owned()),
                        )))
                        .header("anthropic-beta", CLAUDE_SUBSCRIPTION_BETAS)
                        .map_err(map_client_construction_error)?
                        .header("user-agent", "claude-cli/2.1.75 (external, cli)")
                        .map_err(map_client_construction_error)?
                        .header("x-app", "cli")
                        .map_err(map_client_construction_error)?
                        .header("anthropic-dangerous-direct-browser-access", "true")
                        .map_err(map_client_construction_error)?;
                }
                builder = builder.base_url(provider_base_url(provider)?);
                Ok(builder.build())
            })
        })?;
        Ok(ClientPlan::Chat {
            model_hint: model.clone(),
            model,
            temperature: binding.parameters.temperature(),
            config,
            codec,
        })
    }
}

pub(super) fn provider_base_url(provider: ProviderId) -> ProviderResult<&'static str> {
    match provider {
        ProviderId::OpenAi => Ok("https://api.openai.com/v1"),
        ProviderId::DeepSeek => Ok("https://api.deepseek.com"),
        ProviderId::Anthropic | ProviderId::ClaudeSubscription => {
            Ok("https://api.anthropic.com/v1")
        }
        ProviderId::Kimi => Ok("https://api.moonshot.cn/v1"),
        ProviderId::Qwen => Ok("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        // `liter-llm` uses Google's OpenAI-compatible transport for the stable Chat codec.
        ProviderId::Gemini => Ok("https://generativelanguage.googleapis.com/v1beta/openai"),
        ProviderId::MiniMax => Ok("https://api.minimaxi.com/v1"),
        ProviderId::Glm => Ok("https://open.bigmodel.cn/api/paas/v4"),
        ProviderId::OpenRouter => Ok("https://openrouter.ai/api/v1"),
        ProviderId::OpenCodeGo => Ok("https://opencode.ai/zen/go/v1"),
        ProviderId::AlibabaCoding => {
            Ok("https://coding-intl.dashscope.aliyuncs.com/apps/anthropic/v1")
        }
        ProviderId::BigModelCoding => Ok("https://open.bigmodel.cn/api/anthropic/v1"),
        ProviderId::ZaiCoding => Ok("https://api.z.ai/api/anthropic/v1"),
        ProviderId::MiniMaxCoding => Ok("https://api.minimaxi.com/anthropic/v1"),
        ProviderId::KimiCoding => Ok("https://api.kimi.com/coding/v1"),
        ProviderId::ChatGptSubscription
        | ProviderId::OpenCodeZen
        | ProviderId::Xai
        | ProviderId::Zai => Err(ProviderManagementError::new(
            ProviderErrorCode::ProtocolIncompatible,
            Some(ProviderField::Provider),
            ProviderRemediation::ReturnToEdit,
        )),
    }
}

#[async_trait]
impl ProviderClientFactory for LiterProviderFactory {
    async fn build(
        &self,
        binding: ProviderClientBinding,
        credential: CredentialLease,
    ) -> ProviderResult<Arc<dyn ModelProvider>> {
        let plan = self.build_plan(binding, credential)?;
        Ok(Arc::new(LiterModelProvider::from_plan(plan)?))
    }
}

/// A single bound Provider/model client. It cannot re-route a request because it stores exactly
/// one `DefaultClient`, exact model ID, and exactly one closed wire codec.
#[derive(Clone)]
pub struct LiterModelProvider {
    model: String,
    temperature: Option<f32>,
    client: BoundClient,
}

#[derive(Clone)]
enum BoundClient {
    Chat {
        client: DefaultClient,
        codec: LiterChatCodec,
    },
    Responses {
        client: DefaultClient,
        codec: ChatGptResponsesCodec,
    },
}

impl LiterModelProvider {
    fn from_plan(plan: ClientPlan) -> ProviderResult<Self> {
        match plan {
            ClientPlan::Chat {
                model_hint,
                model,
                temperature,
                config,
                codec,
            } => {
                let client = DefaultClient::new(config, Some(&model_hint))
                    .map_err(map_client_construction_error)?;
                Ok(Self {
                    model,
                    temperature,
                    client: BoundClient::Chat { client, codec },
                })
            }
            ClientPlan::Responses {
                model_hint,
                model,
                temperature,
                config,
                codec,
            } => {
                let client = DefaultClient::new(config, Some(&model_hint))
                    .map_err(map_client_construction_error)?;
                Ok(Self {
                    model,
                    temperature,
                    client: BoundClient::Responses { client, codec },
                })
            }
        }
    }

    fn validate_request(&self, request: &ModelRequest) -> CoreResult<()> {
        if request.model != self.model {
            return Err(CoreError::validation(
                "provider_model_mismatch",
                "request model does not match the bound Provider model",
            ));
        }
        if request.temperature != self.temperature {
            return Err(CoreError::validation(
                "provider_parameter_mismatch",
                "request temperature does not match the bound Provider parameters",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl ModelProvider for LiterModelProvider {
    fn capabilities(&self) -> ModelCapabilities {
        // A client is a probe candidate, not support evidence. Compatibility validation owns
        // promotion to Ready and must replace this conservative transport capability with its
        // model-level evidence before an active Profile is selected.
        required_capabilities(CANDIDATE_CONTEXT_LIMIT)
    }

    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse> {
        self.validate_request(&request)?;
        match &self.client {
            BoundClient::Chat { client, codec } => {
                let wire_request = codec.encode_request(&request).map_err(provider_to_core)?;
                let wire_response = client.chat(wire_request).await.map_err(liter_to_core)?;
                codec
                    .decode_response(&request, wire_response)
                    .map_err(provider_to_core)
            }
            BoundClient::Responses { client, codec } => {
                let wire_request = codec.encode_request(&request).map_err(provider_to_core)?;
                let wire_response = client
                    .create_response(wire_request)
                    .await
                    .map_err(liter_to_core)?;
                codec
                    .decode_response(&request, wire_response)
                    .map_err(provider_to_core)
            }
        }
    }
}

fn validate_binding(binding: &ProviderClientBinding) -> ProviderResult<()> {
    if binding.model.provider() != binding.provider {
        return Err(ProviderManagementError::new(
            ProviderErrorCode::InvalidModelPrefix,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ));
    }
    if binding.profile_revision == 0 {
        return Err(ProviderManagementError::new(
            ProviderErrorCode::ModelIncompatible,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ));
    }
    if binding.credential_generation.profile_id() != binding.profile_id
        || binding.credential_generation.kind() != binding.provider.required_credential_kind()
    {
        return Err(authentication_invalid());
    }
    if binding.parameters.temperature().is_some() {
        return Err(parameter_incompatible(ProviderParameterKey::Temperature));
    }
    if binding.parameters.max_tokens().is_some() {
        return Err(parameter_incompatible(ProviderParameterKey::MaxTokens));
    }
    if binding.parameters.timeout_seconds() == 0 {
        return Err(parameter_incompatible(ProviderParameterKey::Timeout));
    }
    if binding.parameters.retry_count() > MAX_BOUND_RETRIES {
        return Err(parameter_incompatible(ProviderParameterKey::Retry));
    }
    if let Some(key) = binding.parameters.provider_specific().keys().next() {
        return Err(parameter_incompatible(
            ProviderParameterKey::ProviderSpecific(key.clone()),
        ));
    }
    Ok(())
}

fn map_client_construction_error(error: LiterLlmError) -> ProviderManagementError {
    ProviderErrorNormalizer::from_liter(error)
}

fn liter_to_core(error: LiterLlmError) -> CoreError {
    ProviderErrorNormalizer::into_core(ProviderErrorNormalizer::from_liter(error))
}

fn provider_to_core(error: ProviderManagementError) -> CoreError {
    ProviderErrorNormalizer::into_core(error)
}

/// Converts third-party failures into the closed core surface before they can be returned,
/// rendered, or observed. It deliberately consumes every external payload without formatting or
/// retaining it; retry decisions remain solely in the fixed client configuration.
#[derive(Debug, Default, Clone, Copy)]
struct ProviderErrorNormalizer;

impl ProviderErrorNormalizer {
    fn from_liter(error: LiterLlmError) -> ProviderManagementError {
        let (code, field, remediation) = match error {
            LiterLlmError::Authentication { .. } => (
                ProviderErrorCode::AuthenticationInvalid,
                Some(ProviderField::Credential),
                ProviderRemediation::ReturnToEdit,
            ),
            LiterLlmError::NotFound { .. } => (
                ProviderErrorCode::ModelNotFound,
                Some(ProviderField::Model),
                ProviderRemediation::ReturnToEdit,
            ),
            LiterLlmError::RateLimited { .. } => (
                ProviderErrorCode::RateLimited,
                Some(ProviderField::Model),
                ProviderRemediation::Retry,
            ),
            LiterLlmError::Timeout => (
                ProviderErrorCode::Timeout,
                Some(ProviderField::Model),
                ProviderRemediation::Retry,
            ),
            LiterLlmError::Network(_) => (
                ProviderErrorCode::Network,
                Some(ProviderField::Model),
                ProviderRemediation::Retry,
            ),
            LiterLlmError::ServerError { .. } | LiterLlmError::ServiceUnavailable { .. } => (
                ProviderErrorCode::Server,
                Some(ProviderField::Model),
                ProviderRemediation::Retry,
            ),
            LiterLlmError::BadRequest { .. }
            | LiterLlmError::ContextWindowExceeded { .. }
            | LiterLlmError::ContentPolicy { .. }
            | LiterLlmError::EndpointNotSupported { .. }
            | LiterLlmError::BudgetExceeded { .. } => (
                ProviderErrorCode::ModelIncompatible,
                Some(ProviderField::Model),
                ProviderRemediation::ValidateProfile,
            ),
            LiterLlmError::InternalError { .. } => (
                ProviderErrorCode::Internal,
                Some(ProviderField::Validation),
                ProviderRemediation::ContactSupport,
            ),
            LiterLlmError::Streaming { .. }
            | LiterLlmError::InvalidHeader { .. }
            | LiterLlmError::Serialization(_)
            | LiterLlmError::HookRejected { .. }
            | LiterLlmError::OutboundForbidden { .. }
            | LiterLlmError::IdempotencyConflict { .. }
            | LiterLlmError::IdempotencyInFlight { .. }
            | _ => (
                ProviderErrorCode::ProtocolInvalidResponse,
                Some(ProviderField::Validation),
                ProviderRemediation::ValidateProfile,
            ),
        };
        ProviderManagementError::new(code, field, remediation)
    }

    #[allow(
        dead_code,
        reason = "the Provider-management service routes explicit cancellation through this stable mapping"
    )]
    fn cancelled() -> ProviderManagementError {
        ProviderManagementError::new(
            ProviderErrorCode::OperationCancelled,
            None,
            ProviderRemediation::ReturnToEdit,
        )
    }

    fn into_core(error: ProviderManagementError) -> CoreError {
        CoreError::validation(error.code(), "Provider transport rejected the request")
    }
}

fn authentication_invalid() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::AuthenticationInvalid,
        Some(ProviderField::Credential),
        ProviderRemediation::ReturnToEdit,
    )
}

fn parameter_incompatible(key: ProviderParameterKey) -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ModelIncompatible,
        Some(ProviderField::Parameter(key)),
        ProviderRemediation::ReturnToEdit,
    )
}

#[cfg(test)]
#[path = "liter_tests.rs"]
mod liter_tests;

#[cfg(test)]
#[path = "liter_http_tests.rs"]
mod liter_http_tests;
