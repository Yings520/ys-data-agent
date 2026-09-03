//! Closed request/response codec for ChatGPT Subscription's fixed Responses backend.
//!
//! This module never routes to OpenAI API or Chat. It validates the only supported model prefix,
//! constructs a fixed-endpoint client configuration from a connected OAuth credential lease, and
//! converts the core tool-call ledger to the OpenAI Responses item wire shape.

#![allow(
    dead_code,
    reason = "The dependency-ordered Liter factory consumes this closed codec in task 3.7."
)]

use std::collections::{BTreeMap, BTreeSet};

use liter_llm::{
    client::{ClientConfig, ClientConfigBuilder},
    types::{CreateResponseRequest, ResponseObject, ResponseTool, ResponseUsage},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ys_agent_core::{
    AgentAction, CredentialLease, ModelMessage, ModelRequest, ModelResponse, ModelRole, ModelUsage,
    ParameterApplicability, ProviderErrorCode, ProviderField, ProviderManagementError,
    ProviderParameterKey, ProviderRemediation, ProviderResult, ToolCall, ToolCallId, ToolSpec,
};

use crate::oauth::chatgpt::with_connected_chatgpt_responses_auth;

/// The ChatGPT subscription backend is intentionally fixed, not a profile-configurable URL.
pub(crate) const CHATGPT_RESPONSES_BACKEND: &str = "https://chatgpt.com/backend-api/codex";
/// Codex's fixed Responses request originator.
pub(crate) const CHATGPT_RESPONSES_ORIGINATOR: &str = "codex_cli_rs";

const CHATGPT_ACCOUNT_HEADER: &str = "ChatGPT-Account-ID";
const CHATGPT_MODEL_PREFIX: &str = "chatgpt/";

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
        Ok(CreateResponseRequest {
            model: model.to_owned(),
            input: Value::Array(input),
            instructions: None,
            tools: (!tools.is_empty()).then_some(tools),
            temperature: request.temperature.map(f64::from),
            max_output_tokens: None,
            metadata: None,
            extra_body: None,
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
            || response.output.len() != 1
        {
            return Err(invalid_response());
        }
        let usage = response.usage.map(convert_usage).transpose()?;
        let item = response
            .output
            .into_iter()
            .next()
            .ok_or_else(invalid_response)?;

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
