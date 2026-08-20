mod fake;
mod openai_compatible;
mod replay;

use std::{fmt, time::Duration};

use ys_agent_core::{CoreError, CoreResult, ModelCapabilities};

pub use fake::FakeModelProvider;
pub use openai_compatible::{OpenAiCompatibleProvider, ProviderCallTelemetry};
pub use replay::ReplayModelProvider;

#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, Debug)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub model: String,
    pub supports_tool_calls: bool,
    pub supports_tool_call_ids: bool,
    pub supports_multi_turn_tool_results: bool,
    pub context_window_tokens: u64,
    pub max_tool_schema_bytes: u64,
    pub request_timeout: Duration,
}

impl OpenAiCompatibleConfig {
    pub fn validate(&self) -> CoreResult<()> {
        if self.base_url.trim() != self.base_url {
            return Err(CoreError::validation(
                "invalid_provider_url",
                "provider base URL must not have surrounding whitespace",
            ));
        }
        let url = reqwest::Url::parse(&self.base_url).map_err(|_| {
            CoreError::validation("invalid_provider_url", "provider base URL is invalid")
        })?;

        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(CoreError::validation(
                "invalid_provider_url",
                "provider base URL must be an absolute HTTP or HTTPS URL",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(CoreError::validation(
                "provider_url_contains_secret",
                "provider credentials must not be embedded in the base URL",
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(CoreError::validation(
                "invalid_provider_url",
                "provider base URL must not contain a query string or fragment",
            ));
        }
        if self.api_key.expose().trim().is_empty() {
            return Err(CoreError::validation(
                "invalid_provider_api_key",
                "provider API key must not be empty",
            ));
        }
        if self.model.trim().is_empty() || self.model.trim() != self.model {
            return Err(CoreError::validation(
                "invalid_provider_model",
                "provider model must not be empty or have surrounding whitespace",
            ));
        }
        if self.context_window_tokens == 0 || self.context_window_tokens > u64::from(u32::MAX) {
            return Err(CoreError::validation(
                "invalid_context_window",
                "context window must fit the Core token counter and be greater than zero",
            ));
        }
        if self.max_tool_schema_bytes == 0 {
            return Err(CoreError::validation(
                "invalid_tool_schema_limit",
                "tool schema byte limit must be greater than zero",
            ));
        }
        if self.request_timeout.is_zero() {
            return Err(CoreError::validation(
                "invalid_provider_timeout",
                "provider timeout must be greater than zero",
            ));
        }

        let missing = [
            (!self.supports_tool_calls, "tool calling"),
            (!self.supports_tool_call_ids, "tool call IDs"),
            (
                !self.supports_multi_turn_tool_results,
                "multi-turn tool results",
            ),
        ]
        .into_iter()
        .filter_map(|(is_missing, name)| is_missing.then_some(name))
        .collect::<Vec<_>>();

        if !missing.is_empty() {
            return Err(CoreError::UnsupportedCapability(format!(
                "provider profile is missing {}",
                missing.join(", ")
            )));
        }

        Ok(())
    }
}

pub(super) fn required_capabilities(context_window_tokens: u64) -> ModelCapabilities {
    ModelCapabilities {
        tool_calling: true,
        structured_outputs: true,
        max_context_tokens: context_window_tokens as u32,
        parallel_tool_calls: false,
        streaming: false,
    }
}
