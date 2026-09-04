//! Closed request/response codec for ChatGPT Subscription's fixed Responses backend.
//!
//! This module never routes to OpenAI API or Chat. It validates the only supported model prefix,
//! constructs a fixed-endpoint client configuration from a connected OAuth credential lease, and
//! converts the core tool-call ledger to the OpenAI Responses item wire shape.

#![allow(
    dead_code,
    reason = "The dependency-ordered Liter factory consumes this closed codec in task 3.7."
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use futures::StreamExt;
use liter_llm::{
    client::{ClientConfig, ClientConfigBuilder},
    types::{
        CreateResponseRequest, ResponseObject, ResponseOutputItem, ResponseTool, ResponseUsage,
    },
};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ys_agent_core::{
    AgentAction, CredentialLease, ModelMessage, ModelRequest, ModelResponse, ModelResponseFormat,
    ModelRole, ModelToolChoice, ModelUsage, ParameterApplicability, ProviderErrorCode,
    ProviderField, ProviderManagementError, ProviderParameterKey, ProviderRemediation,
    ProviderResult, ToolCall, ToolCallId, ToolSpec,
};

use crate::oauth::chatgpt::with_connected_chatgpt_responses_auth;

/// The ChatGPT subscription backend is intentionally fixed, not a profile-configurable URL.
pub(crate) const CHATGPT_RESPONSES_BACKEND: &str = "https://chatgpt.com/backend-api/codex";
/// Codex's fixed Responses request originator.
pub(crate) const CHATGPT_RESPONSES_ORIGINATOR: &str = "codex_cli_rs";
/// Fixed Codex client protocol version accepted by the subscription backend.
pub(crate) const CHATGPT_CODEX_PROTOCOL_VERSION: &str = "0.152.1";
/// Fixed Codex client identity required by both model discovery and Responses requests.
pub(crate) const CHATGPT_CODEX_USER_AGENT: &str = "codex_cli_rs/0.152.1";
/// Codex's Responses endpoint emits Server-Sent Events, including the terminal response object.
pub(crate) const CHATGPT_RESPONSES_ACCEPT: &str = "text/event-stream";

const CHATGPT_ACCOUNT_HEADER: &str = "ChatGPT-Account-ID";
const CHATGPT_MODEL_PREFIX: &str = "chatgpt/";
const CHATGPT_RESPONSES_PATH: &str = "responses";
const MAX_CHATGPT_SSE_EVENT_BYTES: usize = 1_048_576;
const MAX_CHATGPT_SSE_OUTPUT_ITEMS: usize = 16;

/// Converts core model requests to and from ChatGPT's fixed Responses protocol.
///
/// The type has no provider argument by design: constructing it cannot select the OpenAI API or
/// any of the eight Chat protocol providers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChatGptResponsesCodec {
    temperature_applicability: ParameterApplicability,
}

#[derive(Debug, Clone)]
struct DeclaredCall {
    name: String,
    result_seen: bool,
}

/// A deliberately narrow SSE transport for the Codex Subscription backend.
///
/// The public Responses client expects a complete response object in
/// `response.completed`, whereas Codex returns completed output items before a terminal event
/// that contains only turn metadata. This transport preserves the fixed endpoint and credential
/// boundary while assembling the completed items into the codec's closed response shape.
#[derive(Clone)]
pub(crate) struct ChatGptResponsesTransport {
    http: reqwest::Client,
    access_token: SecretString,
    account_id: String,
    base_url: String,
    max_retries: u32,
}

impl ChatGptResponsesTransport {
    pub(crate) fn from_config(config: ClientConfig) -> ProviderResult<Self> {
        let base_url = config
            .base_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(protocol_incompatible)?;
        let account_id = config
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(CHATGPT_ACCOUNT_HEADER))
            .map(|(_, value)| value.clone())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(oauth_not_connected)?;
        if config.api_key.expose_secret().trim().is_empty() {
            return Err(oauth_not_connected());
        }
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|_| network_error())?;

        Ok(Self {
            http,
            access_token: config.api_key,
            account_id,
            base_url,
            max_retries: config.max_retries,
        })
    }

    pub(crate) async fn create_response(
        &self,
        request: CreateResponseRequest,
    ) -> ProviderResult<ResponseObject> {
        let model = request.model.clone();
        let body = streaming_request_body(request)?;
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            CHATGPT_RESPONSES_PATH
        );
        let mut attempts = 0;

        loop {
            let response = self
                .http
                .post(&url)
                .bearer_auth(self.access_token.expose_secret())
                .header(CHATGPT_ACCOUNT_HEADER, &self.account_id)
                .header("originator", CHATGPT_RESPONSES_ORIGINATOR)
                .header("user-agent", CHATGPT_CODEX_USER_AGENT)
                .header("accept", CHATGPT_RESPONSES_ACCEPT)
                .json(&body)
                .send()
                .await
                .map_err(reqwest_error)?;
            let status = response.status();
            if status.is_success() {
                return collect_codex_sse_response(response, &model).await;
            }
            if retryable_status(status) && attempts < self.max_retries {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            return Err(status_error(status));
        }
    }
}

impl ChatGptResponsesCodec {
    pub(crate) const fn new(temperature_applicability: ParameterApplicability) -> Self {
        Self {
            temperature_applicability,
        }
    }

    /// Builds only the fixed ChatGPT configuration. It is crate-visible so the factory can create
    /// the actual `DefaultClient`; callers never receive token/account values separately.
    pub(crate) fn client_config(&self, lease: &CredentialLease) -> ProviderResult<ClientConfig> {
        with_connected_chatgpt_responses_auth(lease, |access_token, account_id| {
            let config = ClientConfigBuilder::new(access_token)
                .load_env(false)
                .base_url(CHATGPT_RESPONSES_BACKEND)
                // Retry policy is owned by the factory. Zero here prevents a codec-only client
                // from acquiring an implicit retry/fallback behavior.
                .max_retries(0)
                .header(CHATGPT_ACCOUNT_HEADER, account_id)
                .map_err(|_| oauth_not_connected())?
                .header("originator", CHATGPT_RESPONSES_ORIGINATOR)
                .map_err(|_| oauth_not_connected())?
                .header("user-agent", CHATGPT_CODEX_USER_AGENT)
                .map_err(|_| oauth_not_connected())?
                .header("accept", CHATGPT_RESPONSES_ACCEPT)
                .map_err(|_| oauth_not_connected())?
                .build();
            Ok(config)
        })
    }

    pub(crate) fn encode_request(
        &self,
        request: &ModelRequest,
    ) -> ProviderResult<CreateResponseRequest> {
        let model = self.validate_model(&request.model)?;
        self.validate_temperature(request.temperature)?;
        if request.messages.is_empty() {
            return Err(protocol_incompatible());
        }

        let tools = convert_tools(&request.tools)?;
        let (input, _) = self.convert_messages(&request.messages, &request.tools)?;
        let has_tools = !tools.is_empty();
        let tool_choice = match request.tool_choice {
            ModelToolChoice::Auto if has_tools => Some("auto"),
            ModelToolChoice::Auto => None,
            ModelToolChoice::Required if has_tools => Some("required"),
            ModelToolChoice::Required => return Err(protocol_incompatible()),
            ModelToolChoice::None if has_tools => Some("none"),
            ModelToolChoice::None => None,
        };
        let mut controls = serde_json::Map::new();
        if let Some(tool_choice) = tool_choice {
            controls.insert("tool_choice".to_owned(), json!(tool_choice));
        }
        if has_tools {
            controls.insert("parallel_tool_calls".to_owned(), json!(false));
        }
        // The subscription backend follows Codex's ephemeral-session protocol. Persisting a
        // probe or a normal application turn is neither needed nor accepted by that transport.
        controls.insert("store".to_owned(), json!(false));
        if request.response_format == ModelResponseFormat::JsonObject {
            controls.insert(
                "text".to_owned(),
                json!({"format": {"type": "json_object"}}),
            );
        }
        Ok(CreateResponseRequest {
            model: model.to_owned(),
            input: Value::Array(input),
            instructions: None,
            tools: (!tools.is_empty()).then_some(tools),
            temperature: request.temperature.map(f64::from),
            max_output_tokens: None,
            metadata: None,
            extra_body: (!controls.is_empty()).then_some(Value::Object(controls)),
            stream: None,
        })
    }

    pub(crate) fn decode_response(
        &self,
        request: &ModelRequest,
        response: ResponseObject,
    ) -> ProviderResult<ModelResponse> {
        self.validate_model(&request.model)?;
        self.validate_temperature(request.temperature)?;
        convert_tools(&request.tools)?;
        let (_, history_calls) = self.convert_messages(&request.messages, &request.tools)?;

        if response.id.trim().is_empty()
            || response.object != "response"
            || response.model.trim().is_empty()
            || response.status != "completed"
            || response.error.is_some()
        {
            return Err(invalid_response());
        }
        let usage = response.usage.map(convert_usage).transpose()?;
        // Reasoning-capable ChatGPT models prepend a non-actionable `reasoning` item to their
        // actual response. It is neither a tool call nor user-visible content, so deliberately
        // discard it here instead of treating a valid response as malformed or surfacing private
        // reasoning in the product. The closed codec still accepts exactly one actionable item.
        let mut actionable_item = None;
        for item in response.output {
            match item.item_type.as_str() {
                "reasoning" => {}
                "function_call" | "message" => {
                    if actionable_item.replace(item).is_some() {
                        return Err(invalid_response());
                    }
                }
                _ => return Err(invalid_response()),
            }
        }
        let item = actionable_item.ok_or_else(invalid_response)?;

        match item.item_type.as_str() {
            "function_call" => {
                let call_id = required_string(&item.content, "call_id", invalid_tool_call_id)?;
                self.validate_call_id(call_id)?;
                if history_calls.contains_key(call_id) {
                    return Err(invalid_tool_call_id());
                }
                let name = required_string(&item.content, "name", protocol_incompatible)?;
                if name.trim().is_empty() || name.trim() != name {
                    return Err(protocol_incompatible());
                }
                let Some(spec) = unique_tool(&request.tools, name) else {
                    return Err(protocol_incompatible());
                };
                let arguments = required_string(&item.content, "arguments", invalid_response)?;
                let arguments = parse_json::<Value>(arguments)?;
                if !arguments.is_object() {
                    return Err(invalid_response());
                }

                Ok(ModelResponse {
                    action: AgentAction::CallTool {
                        call: ToolCall {
                            id: ToolCallId::new(),
                            provider_call_id: Some(call_id.to_owned()),
                            name: spec.name.clone(),
                            arguments,
                            version: spec.version.clone(),
                        },
                    },
                    raw_content: None,
                    usage,
                })
            }
            "message" => {
                let content = response_text(&item.content)?;
                let action = parse_json::<AgentAction>(unwrap_single_json_fence(content))?;
                if matches!(action, AgentAction::CallTool { .. }) {
                    return Err(protocol_incompatible());
                }
                Ok(ModelResponse {
                    action,
                    raw_content: None,
                    usage,
                })
            }
            _ => Err(invalid_response()),
        }
    }

    fn validate_model<'a>(&self, model: &'a str) -> ProviderResult<&'a str> {
        let Some(model) = model.strip_prefix(CHATGPT_MODEL_PREFIX) else {
            return Err(invalid_model_prefix());
        };
        if model.is_empty() || model.chars().any(char::is_whitespace) {
            return Err(invalid_model_prefix());
        }
        Ok(model)
    }

    fn validate_temperature(&self, temperature: Option<f32>) -> ProviderResult<()> {
        let Some(temperature) = temperature else {
            return Ok(());
        };
        if self.temperature_applicability != ParameterApplicability::Supported
            || !temperature.is_finite()
            || !(0.0..=2.0).contains(&temperature)
        {
            return Err(error(
                ProviderErrorCode::ModelIncompatible,
                Some(ProviderField::Parameter(ProviderParameterKey::Temperature)),
                ProviderRemediation::ReturnToEdit,
            ));
        }
        Ok(())
    }

    fn validate_call_id(&self, call_id: &str) -> ProviderResult<()> {
        if call_id.trim().is_empty() || call_id.trim() != call_id {
            return Err(invalid_tool_call_id());
        }
        Ok(())
    }

    fn convert_messages(
        &self,
        messages: &[ModelMessage],
        tools: &[ToolSpec],
    ) -> ProviderResult<(Vec<Value>, BTreeMap<String, DeclaredCall>)> {
        let mut converted = Vec::with_capacity(messages.len());
        let mut calls = BTreeMap::<String, DeclaredCall>::new();

        for message in messages {
            match message.role {
                ModelRole::System | ModelRole::User => {
                    if message.tool_call_id.is_some()
                        || message.assistant_tool_call.is_some()
                        || message.name.is_some()
                    {
                        return Err(protocol_incompatible());
                    }
                    let role = match message.role {
                        ModelRole::System => "system",
                        ModelRole::User => "user",
                        _ => unreachable!("only system and user reach this arm"),
                    };
                    converted.push(json!({
                        "role": role,
                        "content": [{ "type": "input_text", "text": message.content }]
                    }));
                }
                ModelRole::Assistant => {
                    if message.tool_call_id.is_some() {
                        return Err(invalid_tool_call_id());
                    }
                    if message.name.is_some() {
                        return Err(protocol_incompatible());
                    }
                    if let Some(call) = &message.assistant_tool_call {
                        if !message.content.trim().is_empty()
                            || call.name.trim().is_empty()
                            || call.name.trim() != call.name
                            || !call.arguments.is_object()
                            || unique_tool(tools, &call.name).is_none()
                        {
                            return Err(protocol_incompatible());
                        }
                        self.validate_call_id(&call.provider_call_id)?;
                        if calls
                            .insert(
                                call.provider_call_id.clone(),
                                DeclaredCall {
                                    name: call.name.clone(),
                                    result_seen: false,
                                },
                            )
                            .is_some()
                        {
                            return Err(invalid_tool_call_id());
                        }
                        converted.push(json!({
                            "type": "function_call",
                            "call_id": call.provider_call_id,
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments)
                                .map_err(|_| invalid_response())?,
                        }));
                    } else {
                        converted.push(json!({
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": message.content }]
                        }));
                    }
                }
                ModelRole::Tool => {
                    if message.assistant_tool_call.is_some() {
                        return Err(protocol_incompatible());
                    }
                    let call_id = message
                        .tool_call_id
                        .as_deref()
                        .ok_or_else(invalid_tool_call_id)?;
                    self.validate_call_id(call_id)?;
                    let declared = calls.get_mut(call_id).ok_or_else(invalid_tool_call_id)?;
                    if declared.result_seen
                        || message
                            .name
                            .as_deref()
                            .is_some_and(|name| name != declared.name)
                    {
                        return Err(invalid_tool_call_id());
                    }
                    declared.result_seen = true;
                    converted.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": message.content,
                    }));
                }
            }
        }

        if calls.values().any(|call| !call.result_seen) {
            return Err(invalid_tool_call_id());
        }
        Ok((converted, calls))
    }
}

fn streaming_request_body(mut request: CreateResponseRequest) -> ProviderResult<Value> {
    request.stream = Some(true);
    let mut body = serde_json::to_value(request).map_err(|_| protocol_incompatible())?;
    let object = body.as_object_mut().ok_or_else(protocol_incompatible)?;
    if let Some(extra_body) = object.remove("extra_body") {
        let Value::Object(extra_body) = extra_body else {
            return Err(protocol_incompatible());
        };
        object.extend(extra_body);
    }
    // This transport parses SSE, so do not allow an `extra_body` control to desynchronize the
    // request mode from the response parser.
    object.insert("stream".to_owned(), Value::Bool(true));
    Ok(body)
}

async fn collect_codex_sse_response(
    response: reqwest::Response,
    model: &str,
) -> ProviderResult<ResponseObject> {
    let mut bytes = response.bytes_stream();
    let mut pending = Vec::new();
    let mut output = Vec::new();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(reqwest_error)?;
        pending.extend_from_slice(&chunk);
        if pending.len() > MAX_CHATGPT_SSE_EVENT_BYTES {
            return Err(invalid_response());
        }

        while let Some((event_end, delimiter_len)) = sse_event_delimiter(&pending) {
            let event = pending.drain(..event_end).collect::<Vec<_>>();
            pending.drain(..delimiter_len);
            if let Some(response) = consume_codex_sse_event(&event, &mut output, model)? {
                return Ok(response);
            }
        }
    }

    Err(invalid_response())
}

fn sse_event_delimiter(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
}

fn consume_codex_sse_event(
    event: &[u8],
    output: &mut Vec<ResponseOutputItem>,
    model: &str,
) -> ProviderResult<Option<ResponseObject>> {
    let event = std::str::from_utf8(event).map_err(|_| invalid_response())?;
    let data = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>();
    if data.is_empty() {
        return Ok(None);
    }
    let data = data.join("\n");
    if data == "[DONE]" {
        return Ok(None);
    }
    let event: Value = serde_json::from_str(&data).map_err(|_| invalid_response())?;
    let kind = event
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(invalid_response)?;

    match kind {
        "response.output_item.done" => {
            let item = event.get("item").cloned().ok_or_else(invalid_response)?;
            let item = serde_json::from_value(item).map_err(|_| invalid_response())?;
            if output.len() == MAX_CHATGPT_SSE_OUTPUT_ITEMS {
                return Err(invalid_response());
            }
            output.push(item);
            Ok(None)
        }
        "response.completed" => {
            let response = event
                .get("response")
                .and_then(Value::as_object)
                .ok_or_else(invalid_response)?;
            let id = response
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(invalid_response)?;
            Ok(Some(ResponseObject {
                id: id.to_owned(),
                object: "response".to_owned(),
                created_at: 0,
                model: model.to_owned(),
                status: "completed".to_owned(),
                output: std::mem::take(output),
                usage: None,
                error: None,
            }))
        }
        "response.failed" | "response.incomplete" => Err(invalid_response()),
        _ => Ok(None),
    }
}

fn convert_tools(tools: &[ToolSpec]) -> ProviderResult<Vec<ResponseTool>> {
    let mut names = BTreeSet::new();
    let mut converted = Vec::with_capacity(tools.len());
    for tool in tools {
        if tool.name.trim().is_empty()
            || tool.name.trim() != tool.name
            || !tool.input_schema.is_object()
            || !names.insert(tool.name.as_str())
        {
            return Err(protocol_incompatible());
        }
        converted.push(ResponseTool {
            tool_type: "function".to_owned(),
            config: json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            }),
        });
    }
    Ok(converted)
}

fn unique_tool<'a>(tools: &'a [ToolSpec], name: &str) -> Option<&'a ToolSpec> {
    let mut matches = tools.iter().filter(|tool| tool.name == name);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn required_string<'a>(
    value: &'a Value,
    key: &str,
    error: impl FnOnce() -> ProviderManagementError,
) -> ProviderResult<&'a str> {
    value.get(key).and_then(Value::as_str).ok_or_else(error)
}

fn response_text(value: &Value) -> ProviderResult<&str> {
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(invalid_response());
    }
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(invalid_response)?;
    if content.len() != 1 || content[0].get("type").and_then(Value::as_str) != Some("output_text") {
        return Err(protocol_incompatible());
    }
    content[0]
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(invalid_response)
}

fn convert_usage(usage: ResponseUsage) -> ProviderResult<ModelUsage> {
    Ok(ModelUsage {
        prompt_tokens: u32::try_from(usage.input_tokens).map_err(|_| invalid_response())?,
        completion_tokens: u32::try_from(usage.output_tokens).map_err(|_| invalid_response())?,
        total_tokens: u32::try_from(usage.total_tokens).map_err(|_| invalid_response())?,
    })
}

fn parse_json<T>(input: &str) -> ProviderResult<T>
where
    T: DeserializeOwned,
{
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value =
        serde_path_to_error::deserialize(&mut deserializer).map_err(|_| invalid_response())?;
    deserializer.end().map_err(|_| invalid_response())?;
    Ok(value)
}

fn unwrap_single_json_fence(input: &str) -> &str {
    let trimmed = input.trim();
    let Some(after_opening) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some((language, body)) = after_opening.split_once('\n') else {
        return trimmed;
    };
    if !(language.trim().is_empty() || language.trim().eq_ignore_ascii_case("json")) {
        return trimmed;
    }
    let Some(body) = body
        .strip_suffix("\n```")
        .or_else(|| body.strip_suffix("\r\n```"))
    else {
        return trimmed;
    };
    if body.contains("```") {
        return trimmed;
    }
    body.trim()
}

const fn error(
    code: ProviderErrorCode,
    field: Option<ProviderField>,
    remediation: ProviderRemediation,
) -> ProviderManagementError {
    ProviderManagementError::new(code, field, remediation)
}

fn invalid_model_prefix() -> ProviderManagementError {
    error(
        ProviderErrorCode::InvalidModelPrefix,
        Some(ProviderField::Model),
        ProviderRemediation::ReturnToEdit,
    )
}

fn invalid_tool_call_id() -> ProviderManagementError {
    error(
        ProviderErrorCode::ProtocolInvalidToolCallId,
        Some(ProviderField::Validation),
        ProviderRemediation::ValidateProfile,
    )
}

fn invalid_response() -> ProviderManagementError {
    error(
        ProviderErrorCode::ProtocolInvalidResponse,
        Some(ProviderField::Validation),
        ProviderRemediation::ValidateProfile,
    )
}

fn reqwest_error(request_error: reqwest::Error) -> ProviderManagementError {
    if request_error.is_timeout() {
        return error(
            ProviderErrorCode::Timeout,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        );
    }
    network_error()
}

fn network_error() -> ProviderManagementError {
    error(
        ProviderErrorCode::Network,
        Some(ProviderField::Model),
        ProviderRemediation::Retry,
    )
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

fn status_error(status: reqwest::StatusCode) -> ProviderManagementError {
    match status.as_u16() {
        401 | 403 => error(
            ProviderErrorCode::AuthenticationInvalid,
            Some(ProviderField::Credential),
            ProviderRemediation::ReturnToEdit,
        ),
        404 => error(
            ProviderErrorCode::ModelNotFound,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ),
        429 => error(
            ProviderErrorCode::RateLimited,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        ),
        500..=599 => error(
            ProviderErrorCode::Server,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        ),
        _ => error(
            ProviderErrorCode::ModelIncompatible,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ),
    }
}

fn protocol_incompatible() -> ProviderManagementError {
    error(
        ProviderErrorCode::ProtocolIncompatible,
        Some(ProviderField::Validation),
        ProviderRemediation::ValidateProfile,
    )
}

fn oauth_not_connected() -> ProviderManagementError {
    error(
        ProviderErrorCode::OAuthNotConnected,
        Some(ProviderField::OAuth),
        ProviderRemediation::Reauthorize,
    )
}

#[cfg(test)]
#[path = "liter_responses_tests.rs"]
mod liter_responses_tests;
