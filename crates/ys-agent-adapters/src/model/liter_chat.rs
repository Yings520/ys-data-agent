//! Closed request/response codec for the eight API-key Providers that use
//! `liter-llm`'s Chat protocol.
//!
//! The codec deliberately stops at typed `liter-llm` values. Transport construction, credentials,
//! retry, and endpoint selection belong to `LiterProviderFactory`. Keeping this boundary pure lets
//! compatibility probes exercise the exact message and tool-ID contract without network access.

use std::collections::{BTreeMap, BTreeSet};

use liter_llm::types::{
    AssistantContent, AssistantMessage, ChatCompletionRequest, ChatCompletionResponse,
    ChatCompletionTool, FinishReason, FunctionCall, FunctionDefinition, Message, SystemMessage,
    ToolCall as LiterToolCall, ToolChoice, ToolChoiceMode, ToolMessage, ToolType, UserContent,
    UserMessage,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use ys_agent_core::{
    AgentAction, ModelMessage, ModelRequest, ModelResponse, ModelRole, ModelUsage,
    ParameterApplicability, ProviderErrorCode, ProviderField, ProviderId, ProviderManagementError,
    ProviderParameterKey, ProviderRemediation, ProviderResult, ToolCall, ToolCallId, ToolSpec,
};

/// Version input for compatibility evidence. Bump whenever the encoded contract changes.
pub const LITER_CHAT_CODEC_VERSION: &str = "1";

/// Converts stable core messages to and from the Chat types owned by `liter-llm`.
///
/// `temperature_applicability` is the already-resolved catalog/model evidence decision. A
/// configured value is sent only when it is `Supported`; `Conditional` is intentionally not an
/// implicit approval.
#[derive(Debug, Clone, Copy)]
pub struct LiterChatCodec {
    provider: ProviderId,
    temperature_applicability: ParameterApplicability,
}

#[derive(Debug, Clone)]
struct DeclaredCall {
    name: String,
    result_seen: bool,
}

impl LiterChatCodec {
    pub fn new(
        provider: ProviderId,
        temperature_applicability: ParameterApplicability,
    ) -> ProviderResult<Self> {
        if provider == ProviderId::ChatGptSubscription {
            return Err(error(
                ProviderErrorCode::ProtocolIncompatible,
                Some(ProviderField::Provider),
                ProviderRemediation::ValidateProfile,
            ));
        }
        Ok(Self {
            provider,
            temperature_applicability,
        })
    }

    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn encode_request(&self, request: &ModelRequest) -> ProviderResult<ChatCompletionRequest> {
        self.validate_model(&request.model)?;
        self.validate_temperature(request.temperature)?;
        if request.messages.is_empty() {
            return Err(protocol_incompatible());
        }

        let tools = convert_tools(&request.tools)?;
        let messages = self.convert_messages(&request.messages, &request.tools)?.0;
        let has_tools = !tools.is_empty();

        Ok(ChatCompletionRequest {
            model: request.model.clone(),
            messages,
            temperature: request.temperature.map(f64::from),
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some(ToolChoice::Mode(ToolChoiceMode::Auto)),
            // Core accepts one action per turn. Asking providers not to emit parallel calls makes
            // that constraint explicit; decode still rejects a provider that ignores it.
            parallel_tool_calls: has_tools.then_some(false),
            ..ChatCompletionRequest::default()
        })
    }

    pub fn decode_response(
        &self,
        request: &ModelRequest,
        mut response: ChatCompletionResponse,
    ) -> ProviderResult<ModelResponse> {
        self.validate_model(&request.model)?;
        self.validate_temperature(request.temperature)?;
        convert_tools(&request.tools)?;
        let (_, history_calls) = self.convert_messages(&request.messages, &request.tools)?;

        if response.id.trim().is_empty()
            || response.model.trim().is_empty()
            || response.choices.len() != 1
        {
            return Err(invalid_response());
        }
        let choice = response.choices.remove(0);
        if choice.index != 0
            || choice.message.function_call.is_some()
            || choice.message.refusal_text().is_some()
        {
            return Err(invalid_response());
        }

        let usage = response.usage.map(convert_usage).transpose()?;
        let message_text = match &choice.message.content {
            Some(AssistantContent::Text(text)) => Some(text.clone()),
            None => None,
            // Core's stable response contract is text-only. Accepting `Parts` through `text()`
            // would silently discard image/audio/refusal blocks.
            Some(AssistantContent::Parts(_)) => return Err(protocol_incompatible()),
        };
        let tool_calls = choice.message.tool_calls.unwrap_or_default();
        if tool_calls.len() > 1 {
            return Err(invalid_response());
        }

        if let Some(tool_call) = tool_calls.into_iter().next() {
            if !matches!(
                choice.finish_reason,
                Some(FinishReason::ToolCalls | FinishReason::FunctionCall)
            ) || message_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
            {
                return Err(invalid_response());
            }

            self.validate_call_id(&tool_call.id)?;
            if history_calls.contains_key(&tool_call.id) {
                return Err(invalid_tool_call_id());
            }
            if tool_call.function.name.trim().is_empty()
                || tool_call.function.name.trim() != tool_call.function.name
            {
                return Err(protocol_incompatible());
            }
            let Some(spec) = unique_tool(&request.tools, &tool_call.function.name) else {
                return Err(protocol_incompatible());
            };
            let arguments = parse_json::<Value>(&tool_call.function.arguments)?;
            if !arguments.is_object() {
                return Err(invalid_response());
            }

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

        if !matches!(choice.finish_reason, Some(FinishReason::Stop)) {
            return Err(invalid_response());
        }
        let content = message_text.ok_or_else(invalid_response)?;
        let action = parse_json::<AgentAction>(unwrap_single_json_fence(&content))?;
        if matches!(action, AgentAction::CallTool { .. }) {
            return Err(protocol_incompatible());
        }

        Ok(ModelResponse {
            action,
            // Provider payloads and business content are not retained by this codec.
            raw_content: None,
            usage,
        })
    }

    fn validate_model(&self, model: &str) -> ProviderResult<()> {
        let prefix = self.provider.model_prefix();
        if !model.starts_with(prefix)
            || model.len() == prefix.len()
            || model.chars().any(char::is_whitespace)
        {
            return Err(error(
                ProviderErrorCode::InvalidModelPrefix,
                Some(ProviderField::Model),
                ProviderRemediation::ReturnToEdit,
            ));
        }
        Ok(())
    }

    fn validate_temperature(&self, temperature: Option<f32>) -> ProviderResult<()> {
        let Some(temperature) = temperature else {
            return Ok(());
        };
        if self.temperature_applicability != ParameterApplicability::Supported
            || !temperature.is_finite()
            || !(0.0..=2.0).contains(&temperature)
            || (self.provider == ProviderId::Anthropic && temperature > 1.0)
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
        // liter-llm's Anthropic transform replaces every other character with `_`. Rejecting
        // before that transform is the only way to uphold the core exact-ID contract.
        if self.provider == ProviderId::Anthropic
            && !call_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(invalid_tool_call_id());
        }
        Ok(())
    }

    fn convert_messages(
        &self,
        messages: &[ModelMessage],
        tools: &[ToolSpec],
    ) -> ProviderResult<(Vec<Message>, BTreeMap<String, DeclaredCall>)> {
        let mut converted = Vec::with_capacity(messages.len());
        let mut calls = BTreeMap::<String, DeclaredCall>::new();

        for message in messages {
            match message.role {
                ModelRole::System | ModelRole::User => {
                    if message.tool_call_id.is_some() || message.assistant_tool_call.is_some() {
                        return Err(protocol_incompatible());
                    }
                    let content = UserContent::Text(message.content.clone());
                    converted.push(match message.role {
                        ModelRole::System => Message::System(SystemMessage {
                            content,
                            name: message.name.clone(),
                        }),
                        ModelRole::User => Message::User(UserMessage {
                            content,
                            name: message.name.clone(),
                        }),
                        _ => unreachable!("the match arm admits only system and user"),
                    });
                }
                ModelRole::Assistant => {
                    if message.tool_call_id.is_some() {
                        return Err(invalid_tool_call_id());
                    }
                    let tool_calls = if let Some(call) = &message.assistant_tool_call {
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
                        Some(vec![LiterToolCall {
                            id: call.provider_call_id.clone(),
                            call_type: ToolType::Function,
                            function: FunctionCall {
                                name: call.name.clone(),
                                arguments: serde_json::to_string(&call.arguments)
                                    .map_err(|_| invalid_response())?,
                            },
                        }])
                    } else {
                        None
                    };
                    converted.push(Message::Assistant(AssistantMessage {
                        content: tool_calls
                            .is_none()
                            .then(|| AssistantContent::Text(message.content.clone())),
                        name: message.name.clone(),
                        tool_calls,
                        ..AssistantMessage::default()
                    }));
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
                    converted.push(Message::Tool(ToolMessage {
                        content: UserContent::Text(message.content.clone()),
                        tool_call_id: call_id.to_owned(),
                        name: message.name.clone(),
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

fn convert_tools(tools: &[ToolSpec]) -> ProviderResult<Vec<ChatCompletionTool>> {
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
        converted.push(ChatCompletionTool {
            tool_type: ToolType::Function,
            function: FunctionDefinition {
                name: tool.name.clone(),
                description: (!tool.description.is_empty()).then(|| tool.description.clone()),
                parameters: Some(tool.input_schema.clone()),
                strict: None,
            },
        });
    }
    Ok(converted)
}

fn unique_tool<'a>(tools: &'a [ToolSpec], name: &str) -> Option<&'a ToolSpec> {
    let mut matches = tools.iter().filter(|tool| tool.name == name);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn convert_usage(usage: liter_llm::types::Usage) -> ProviderResult<ModelUsage> {
    Ok(ModelUsage {
        prompt_tokens: u32::try_from(usage.prompt_tokens).map_err(|_| invalid_response())?,
        completion_tokens: u32::try_from(usage.completion_tokens)
            .map_err(|_| invalid_response())?,
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

#[cfg(test)]
#[path = "liter_chat_tests.rs"]
mod liter_chat_tests;
