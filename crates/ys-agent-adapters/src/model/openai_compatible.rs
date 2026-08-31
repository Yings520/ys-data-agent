use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use ys_agent_core::{
    AgentAction, CoreError, CoreResult, ModelCapabilities, ModelMessage, ModelProvider,
    ModelRequest, ModelResponse, ModelRole, ModelUsage, ToolCall, ToolCallId, ToolSpec,
};

use super::{OpenAiCompatibleConfig, required_capabilities};

const PROVIDER_NAME: &str = "openai_compatible";
const MAX_TRANSPORT_RETRIES: u32 = 2;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    http: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCallTelemetry {
    pub provider: &'static str,
    pub model: String,
    pub latency_ms: u64,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub attempts: u32,
    pub outcome: &'static str,
}

impl ProviderCallTelemetry {
    fn record(&self) {
        tracing::info!(
            provider = self.provider,
            model = %self.model,
            latency_ms = self.latency_ms,
            prompt_tokens = ?self.prompt_tokens,
            completion_tokens = ?self.completion_tokens,
            total_tokens = ?self.total_tokens,
            attempts = self.attempts,
            outcome = self.outcome,
            "model provider call completed"
        );
    }
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> CoreResult<Self> {
        config.validate()?;
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|_| {
                CoreError::validation(
                    "provider_client_initialization",
                    "could not initialize the model HTTP client",
                )
            })?;

        Ok(Self { config, http })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    fn build_request(&self, request: &ModelRequest) -> CoreResult<ApiChatRequest> {
        if request.model != self.config.model {
            return Err(CoreError::validation(
                "provider_model_mismatch",
                "request model does not match the configured provider model",
            ));
        }

        if u64::from(request.context_manifest.tokens_used) >= self.config.context_window_tokens {
            return Err(CoreError::validation(
                "model_context_budget_exceeded",
                "context uses the complete provider context window",
            ));
        }

        let tools = request.tools.iter().map(convert_tool).collect::<Vec<_>>();
        let schema_bytes = serde_json::to_vec(&tools)
            .map_err(|_| {
                CoreError::validation(
                    "tool_schema_serialization",
                    "could not serialize the model-visible ToolView",
                )
            })?
            .len() as u64;

        if schema_bytes > self.config.max_tool_schema_bytes {
            return Err(CoreError::validation(
                "tool_schema_budget_exceeded",
                "model-visible Tool Schemas exceed the provider profile limit",
            ));
        }

        let messages = request
            .messages
            .iter()
            .map(convert_message)
            .collect::<CoreResult<Vec<_>>>()?;
        Ok(ApiChatRequest {
            model: self.config.model.clone(),
            messages,
            tool_choice: (!tools.is_empty()).then_some("auto"),
            tools,
            parallel_tool_calls: false,
            stream: false,
            temperature: request.temperature,
        })
    }

    fn parse_response(
        &self,
        mut response: ApiChatResponse,
        request: &ModelRequest,
    ) -> CoreResult<ModelResponse> {
        if response.choices.len() != 1 {
            return Err(CoreError::validation(
                "invalid_model_response",
                "provider response must contain exactly one choice",
            ));
        }

        let message = response.choices.remove(0).message;
        if message.role != "assistant" {
            return Err(CoreError::validation(
                "invalid_model_response",
                "provider choice must contain an assistant message",
            ));
        }

        if message.tool_calls.len() > 1 {
            return Err(CoreError::validation(
                "parallel_tool_calls_disabled",
                "v0.2 accepts at most one Tool Call per model response",
            ));
        }

        let usage = response.usage.map(|usage| ModelUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        });

        if let Some(tool_call) = message.tool_calls.into_iter().next() {
            if message
                .content
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
            {
                return Err(CoreError::validation(
                    "ambiguous_model_response",
                    "assistant response contains both content and a Tool Call",
                ));
            }
            if tool_call.kind != "function" || tool_call.id.trim().is_empty() {
                return Err(CoreError::validation(
                    "invalid_model_response",
                    "provider Tool Call has an invalid type or empty ID",
                ));
            }

            let Some(spec) = request
                .tools
                .iter()
                .find(|spec| spec.name == tool_call.function.name)
            else {
                return Err(CoreError::validation(
                    "unknown_model_tool",
                    "provider requested a tool outside the supplied ToolView",
                ));
            };

            let arguments = deserialize_with_path::<Value>(
                &tool_call.function.arguments,
                "invalid_tool_arguments",
                "Tool arguments",
            )?;

            return Ok(ModelResponse {
                action: AgentAction::CallTool {
                    call: ToolCall {
                        id: ToolCallId::new(),
                        provider_call_id: Some(tool_call.id),
                        name: spec.name.clone(),
                        arguments,
                        version: spec.version.clone(),
                    },
                },
                raw_content: None,
                usage,
            });
        }

        let content = message.content.ok_or_else(|| {
            CoreError::validation(
                "invalid_model_response",
                "assistant response contains neither content nor a Tool Call",
            )
        })?;
        let action = deserialize_with_path::<AgentAction>(
            &content,
            "invalid_model_response",
            "assistant action",
        )?;

        if matches!(&action, AgentAction::CallTool { .. }) {
            return Err(CoreError::validation(
                "tool_call_in_free_form_content",
                "Tool Calls are accepted only from the structured tool_calls field",
            ));
        }

        Ok(ModelResponse {
            action,
            raw_content: None,
            usage,
        })
    }

    async fn send_once(
        &self,
        body: &ApiChatRequest,
    ) -> Result<ApiChatResponse, NormalizedProviderFailure> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(self.config.api_key.expose())
            .json(body)
            .send()
            .await
            .map_err(|error| normalize_transport_error(&error))?;

        if !response.status().is_success() {
            return Err(normalize_http_status(response.status()));
        }

        response
            .json::<ApiChatResponse>()
            .await
            .map_err(|_| NormalizedProviderFailure {
                code: "invalid_model_response",
                message: "model provider returned malformed response JSON",
                retryable: false,
            })
    }

    fn record_telemetry(
        &self,
        started_at: Instant,
        usage: Option<ApiUsage>,
        attempts: u32,
        outcome: &'static str,
    ) {
        ProviderCallTelemetry {
            provider: PROVIDER_NAME,
            model: self.config.model.clone(),
            latency_ms: started_at.elapsed().as_millis() as u64,
            prompt_tokens: usage.map(|value| value.prompt_tokens),
            completion_tokens: usage.map(|value| value.completion_tokens),
            total_tokens: usage.map(|value| value.total_tokens),
            attempts,
            outcome,
        }
        .record();
    }
}

fn convert_message(message: &ModelMessage) -> CoreResult<ApiRequestMessage> {
    let role = match message.role {
        ModelRole::System => "system",
        ModelRole::User => "user",
        ModelRole::Assistant => "assistant",
        ModelRole::Tool => "tool",
    };

    if message.role == ModelRole::Tool && message.tool_call_id.is_none() {
        return Err(CoreError::validation(
            "missing_tool_call_id",
            "a Tool result message requires the original provider Tool Call ID",
        ));
    }
    if message.role != ModelRole::Tool && message.tool_call_id.is_some() {
        return Err(CoreError::validation(
            "unexpected_tool_call_id",
            "only a Tool result message may contain a Tool Call ID",
        ));
    }

    let tool_calls = match &message.assistant_tool_call {
        Some(call) => {
            if message.role != ModelRole::Assistant {
                return Err(CoreError::validation(
                    "unexpected_assistant_tool_call",
                    "only an assistant message may replay a structured Tool Call",
                ));
            }
            if !message.content.trim().is_empty() {
                return Err(CoreError::validation(
                    "ambiguous_assistant_tool_call",
                    "an assistant Tool Call replay cannot also contain text content",
                ));
            }
            if call.provider_call_id.trim().is_empty() || call.name.trim().is_empty() {
                return Err(CoreError::validation(
                    "invalid_assistant_tool_call",
                    "an assistant Tool Call replay needs a provider ID and tool name",
                ));
            }
            Some(vec![ApiRequestToolCall {
                id: call.provider_call_id.clone(),
                kind: "function",
                function: ApiRequestFunctionCall {
                    name: call.name.clone(),
                    arguments: serde_json::to_string(&call.arguments).map_err(|error| {
                        CoreError::validation("invalid_assistant_tool_call", error.to_string())
                    })?,
                },
            }])
        }
        None => None,
    };

    Ok(ApiRequestMessage {
        role,
        content: if message.assistant_tool_call.is_some() {
            None
        } else {
            Some(message.content.clone())
        },
        tool_call_id: message.tool_call_id.clone(),
        name: message.name.clone(),
        tool_calls,
    })
}

fn convert_tool(tool: &ToolSpec) -> ApiTool {
    ApiTool {
        kind: "function",
        function: ApiFunctionDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

fn deserialize_with_path<T>(input: &str, code: &'static str, label: &str) -> CoreResult<T>
where
    T: DeserializeOwned,
{
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        CoreError::validation(
            code,
            format!("{label} is invalid at JSON path {}", error.path()),
        )
    })?;
    deserializer
        .end()
        .map_err(|_| CoreError::validation(code, format!("{label} contains trailing JSON data")))?;
    Ok(value)
}

#[derive(Clone, Debug)]
struct NormalizedProviderFailure {
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

impl NormalizedProviderFailure {
    fn into_core_error(self) -> CoreError {
        CoreError::validation(self.code, self.message)
    }
}

fn normalize_http_status(status: reqwest::StatusCode) -> NormalizedProviderFailure {
    match status.as_u16() {
        408 | 429 | 502 | 503 | 504 => NormalizedProviderFailure {
            code: "provider_retryable_http",
            message: "model provider is temporarily unavailable",
            retryable: true,
        },
        401 | 403 => NormalizedProviderFailure {
            code: "provider_authentication",
            message: "model provider rejected the configured credentials or permissions",
            retryable: false,
        },
        _ => NormalizedProviderFailure {
            code: "provider_http",
            message: "model provider returned an unsuccessful HTTP status",
            retryable: false,
        },
    }
}

fn normalize_transport_error(error: &reqwest::Error) -> NormalizedProviderFailure {
    if error.is_timeout() {
        return NormalizedProviderFailure {
            code: "provider_timeout",
            message: "model provider did not respond before the request timeout",
            retryable: true,
        };
    }

    NormalizedProviderFailure {
        code: "provider_transport",
        message: "model provider could not be reached",
        retryable: error.is_connect(),
    }
}

#[derive(Clone, Debug, Serialize)]
struct ApiChatRequest {
    model: String,
    messages: Vec<ApiRequestMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    parallel_tool_calls: bool,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
struct ApiRequestMessage {
    role: &'static str,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiRequestToolCall>>,
}

#[derive(Clone, Debug, Serialize)]
struct ApiRequestToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: ApiRequestFunctionCall,
}

#[derive(Clone, Debug, Serialize)]
struct ApiRequestFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Clone, Debug, Serialize)]
struct ApiTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ApiFunctionDefinition,
}

#[derive(Clone, Debug, Serialize)]
struct ApiFunctionDefinition {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct ApiChatResponse {
    choices: Vec<ApiChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
struct ApiChoice {
    message: ApiAssistantMessage,
}

#[derive(Debug, Deserialize)]
struct ApiAssistantMessage {
    role: String,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ApiToolCall>,
}

#[derive(Debug, Deserialize)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: ApiFunctionCall,
}

#[derive(Debug, Deserialize)]
struct ApiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn capabilities(&self) -> ModelCapabilities {
        required_capabilities(self.config.context_window_tokens)
    }

    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse> {
        let body = self.build_request(&request)?;
        let started_at = Instant::now();

        for retry_index in 0..=MAX_TRANSPORT_RETRIES {
            let attempts = retry_index + 1;
            match self.send_once(&body).await {
                Ok(wire_response) => {
                    let usage = wire_response.usage;
                    let parsed = self.parse_response(wire_response, &request);
                    self.record_telemetry(
                        started_at,
                        usage,
                        attempts,
                        if parsed.is_ok() {
                            "succeeded"
                        } else {
                            "invalid_response"
                        },
                    );
                    return parsed;
                }
                Err(failure) if failure.retryable && retry_index < MAX_TRANSPORT_RETRIES => {
                    let delay = RETRY_BASE_DELAY.saturating_mul(1_u32 << retry_index);
                    tokio::time::sleep(delay).await;
                }
                Err(failure) => {
                    self.record_telemetry(started_at, None, attempts, failure.code);
                    return Err(failure.into_core_error());
                }
            }
        }

        unreachable!("the bounded retry loop always returns")
    }
}
