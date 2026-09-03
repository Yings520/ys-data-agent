use liter_llm::types::ChatCompletionResponse;
use serde_json::{Value, json};
use ys_agent_core::{
    AgentAction, AssistantToolCall, ContextManifest, ModelMessage, ModelRequest, ModelRole,
    ParameterApplicability, ProviderErrorCode, ProviderField, ProviderId, ProviderParameterKey,
    Sensitivity, SideEffect, ToolRisk, ToolSpec,
};

use super::LiterChatCodec;

const CHAT_PROVIDERS: [ProviderId; 8] = [
    ProviderId::OpenCodeGo,
    ProviderId::OpenCodeZen,
    ProviderId::DeepSeek,
    ProviderId::Xai,
    ProviderId::Zai,
    ProviderId::OpenRouter,
    ProviderId::MiniMax,
    ProviderId::Anthropic,
];

fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: "inspect_schema".to_owned(),
        description: "Read a synthetic schema fixture".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": { "source": { "type": "string" } },
            "required": ["source"],
            "additionalProperties": false
        }),
        output_schema: json!({ "type": "object" }),
        risk: ToolRisk::Low,
        side_effect: SideEffect::None,
        idempotent: true,
        timeout_ms: 1_000,
        max_output_bytes: 4_096,
        required_permissions: Vec::new(),
        input_sensitivity: Sensitivity::Internal,
        output_sensitivity: Sensitivity::Internal,
        version: "fixture-v1".to_owned(),
    }
}

fn message(role: ModelRole, content: &str) -> ModelMessage {
    ModelMessage {
        role,
        content: content.to_owned(),
        tool_call_id: None,
        name: None,
        assistant_tool_call: None,
    }
}

fn request(provider: ProviderId) -> ModelRequest {
    ModelRequest {
        model: format!("{}fixture-model", provider.model_prefix()),
        messages: vec![
            message(ModelRole::System, "Use only the supplied fixture tool."),
            message(ModelRole::User, "Inspect the synthetic source."),
        ],
        tools: vec![tool_spec()],
        context_manifest: ContextManifest::empty(8_000),
        temperature: Some(0.2),
    }
}

fn codec(provider: ProviderId) -> LiterChatCodec {
    LiterChatCodec::new(provider, ParameterApplicability::Supported).expect("Chat codec")
}

fn response(message: Value, finish_reason: &str) -> ChatCompletionResponse {
    serde_json::from_value(json!({
        "id": "fixture-response",
        "object": "chat.completion",
        "created": 1,
        "model": "fixture-model",
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
            "logprobs": null
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 4,
            "total_tokens": 14
        }
    }))
    .expect("valid liter response fixture")
}

fn tool_call_response(call_id: &str) -> ChatCompletionResponse {
    response(
        json!({
            "content": null,
            "tool_calls": [{
                "id": call_id,
                "type": "function",
                "function": {
                    "name": "inspect_schema",
                    "arguments": "{\"source\":\"fixture\"}"
                }
            }],
            "refusal": null
        }),
        "tool_calls",
    )
}

#[test]
fn eight_chat_paths_encode_the_same_closed_liter_fixture() {
    for provider in CHAT_PROVIDERS {
        let encoded = codec(provider)
            .encode_request(&request(provider))
            .expect("approved Chat request");
        let value = serde_json::to_value(encoded).expect("serialize liter request");

        assert_eq!(value["model"], "fixture-model");
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][1]["role"], "user");
        assert_eq!(value["temperature"].as_f64(), Some(f64::from(0.2_f32)));
        assert_eq!(value["tools"][0]["function"]["name"], "inspect_schema");
        assert_eq!(value["tool_choice"], "auto");
        assert_eq!(value["parallel_tool_calls"], false);
        assert!(value.get("extra_body").is_none());
    }
}

#[test]
fn tool_call_and_result_round_trip_preserves_the_provider_id() {
    for provider in CHAT_PROVIDERS {
        let mut core_request = request(provider);
        let decoded = codec(provider)
            .decode_response(&core_request, tool_call_response("call_fixture_123"))
            .expect("valid tool call");
        let AgentAction::CallTool { call } = decoded.action else {
            panic!("expected structured tool call");
        };
        assert_eq!(call.provider_call_id.as_deref(), Some("call_fixture_123"));
        assert_eq!(call.version, "fixture-v1");

        core_request.messages.push(ModelMessage {
            role: ModelRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            name: None,
            assistant_tool_call: Some(AssistantToolCall {
                provider_call_id: call.provider_call_id.expect("provider call ID"),
                name: call.name,
                arguments: call.arguments,
            }),
        });
        core_request.messages.push(ModelMessage {
            role: ModelRole::Tool,
            content: "{\"relations\":[]}".to_owned(),
            tool_call_id: Some("call_fixture_123".to_owned()),
            name: Some("inspect_schema".to_owned()),
            assistant_tool_call: None,
        });

        let encoded = codec(provider)
            .encode_request(&core_request)
            .expect("valid tool result round trip");
        let value = serde_json::to_value(encoded).expect("serialize liter request");
        assert_eq!(
            value["messages"][2]["tool_calls"][0]["id"],
            "call_fixture_123"
        );
        assert_eq!(value["messages"][3]["tool_call_id"], "call_fixture_123");
    }
}

#[test]
fn anthropic_path_rejects_ids_that_its_transform_would_rewrite() {
    let provider = ProviderId::Anthropic;
    let error = codec(provider)
        .decode_response(&request(provider), tool_call_response("call.fixture"))
        .expect_err("Anthropic transform would rewrite this ID");

    assert_eq!(
        error.code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );
    assert_eq!(error.field(), Some(&ProviderField::Validation));
}

#[test]
fn anthropic_temperature_limit_is_checked_before_its_transform() {
    let provider = ProviderId::Anthropic;
    let mut core_request = request(provider);
    core_request.temperature = Some(1.1);

    let error = codec(provider)
        .encode_request(&core_request)
        .expect_err("Anthropic rejects temperature above one");

    assert_eq!(error.code(), ProviderErrorCode::ModelIncompatible.as_str());
    assert_eq!(
        error.field(),
        Some(&ProviderField::Parameter(ProviderParameterKey::Temperature))
    );
}

#[test]
fn unsupported_or_unproven_temperature_is_not_silently_stripped() {
    for applicability in [
        ParameterApplicability::Unsupported,
        ParameterApplicability::Conditional,
    ] {
        let provider = ProviderId::DeepSeek;
        let error = LiterChatCodec::new(provider, applicability)
            .expect("Chat codec")
            .encode_request(&request(provider))
            .expect_err("temperature must be rejected");

        assert_eq!(error.code(), ProviderErrorCode::ModelIncompatible.as_str());
        assert_eq!(
            error.field(),
            Some(&ProviderField::Parameter(ProviderParameterKey::Temperature))
        );
    }
}

#[test]
fn blank_missing_duplicate_and_conflicting_ids_fail_closed() {
    let provider = ProviderId::DeepSeek;
    for invalid_id in ["", "   "] {
        let error = codec(provider)
            .decode_response(&request(provider), tool_call_response(invalid_id))
            .expect_err("blank response ID");
        assert_eq!(
            error.code(),
            ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
        );
    }

    let mut missing = request(provider);
    missing.messages.push(ModelMessage {
        role: ModelRole::Tool,
        content: "{}".to_owned(),
        tool_call_id: None,
        name: Some("inspect_schema".to_owned()),
        assistant_tool_call: None,
    });
    assert_eq!(
        codec(provider)
            .encode_request(&missing)
            .expect_err("missing tool result ID")
            .code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );

    let assistant = ModelMessage {
        role: ModelRole::Assistant,
        content: String::new(),
        tool_call_id: None,
        name: None,
        assistant_tool_call: Some(AssistantToolCall {
            provider_call_id: "call_same".to_owned(),
            name: "inspect_schema".to_owned(),
            arguments: json!({"source": "fixture"}),
        }),
    };
    let tool = ModelMessage {
        role: ModelRole::Tool,
        content: "{}".to_owned(),
        tool_call_id: Some("call_same".to_owned()),
        name: Some("inspect_schema".to_owned()),
        assistant_tool_call: None,
    };

    let mut duplicate = request(provider);
    duplicate
        .messages
        .extend([assistant.clone(), tool.clone(), tool]);
    assert_eq!(
        codec(provider)
            .encode_request(&duplicate)
            .expect_err("duplicate result ID")
            .code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );

    let mut conflicting = request(provider);
    conflicting.messages.extend([
        assistant,
        ModelMessage {
            role: ModelRole::Tool,
            content: "{}".to_owned(),
            tool_call_id: Some("call_other".to_owned()),
            name: Some("inspect_schema".to_owned()),
            assistant_tool_call: None,
        },
    ]);
    assert_eq!(
        codec(provider)
            .encode_request(&conflicting)
            .expect_err("conflicting result ID")
            .code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );
}

#[test]
fn malformed_and_ambiguous_provider_responses_have_stable_failures() {
    let provider = ProviderId::OpenRouter;
    let mut cases = vec![
        response(json!({"content": null, "refusal": null}), "stop"),
        response(
            json!({
                "content": "text and call",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "inspect_schema", "arguments": "{}"}
                }],
                "refusal": null
            }),
            "tool_calls",
        ),
        response(
            json!({
                "content": null,
                "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "inspect_schema", "arguments": "{}"}},
                    {"id": "call_2", "type": "function", "function": {"name": "inspect_schema", "arguments": "{}"}}
                ],
                "refusal": null
            }),
            "tool_calls",
        ),
        response(
            json!({
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "unknown_tool", "arguments": "{}"}
                }],
                "refusal": null
            }),
            "tool_calls",
        ),
        response(
            json!({
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "inspect_schema", "arguments": "not-json"}
                }],
                "refusal": null
            }),
            "tool_calls",
        ),
        response(
            json!({
                "content": [{
                    "type": "text",
                    "text": "{\"type\":\"respond\",\"message\":\"done\"}"
                }],
                "refusal": null
            }),
            "stop",
        ),
    ];
    cases.push(
        serde_json::from_value(json!({
            "id": "fixture-response",
            "object": "chat.completion",
            "created": 1,
            "model": "fixture-model",
            "choices": []
        }))
        .expect("empty response fixture"),
    );
    let mut blank_identity = response(
        json!({
            "content": "{\"type\":\"respond\",\"message\":\"done\"}",
            "refusal": null
        }),
        "stop",
    );
    blank_identity.id.clear();
    cases.push(blank_identity);

    for fixture in cases {
        let error = codec(provider)
            .decode_response(&request(provider), fixture)
            .expect_err("invalid provider response");
        assert!(matches!(
            error.code(),
            "provider.protocol.invalid_response" | "provider.protocol.incompatible"
        ));
    }
}

#[test]
fn provider_cannot_reuse_a_completed_history_call_id() {
    let provider = ProviderId::Zai;
    let mut core_request = request(provider);
    core_request.messages.extend([
        ModelMessage {
            role: ModelRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            name: None,
            assistant_tool_call: Some(AssistantToolCall {
                provider_call_id: "call_history".to_owned(),
                name: "inspect_schema".to_owned(),
                arguments: json!({"source": "fixture"}),
            }),
        },
        ModelMessage {
            role: ModelRole::Tool,
            content: "{}".to_owned(),
            tool_call_id: Some("call_history".to_owned()),
            name: Some("inspect_schema".to_owned()),
            assistant_tool_call: None,
        },
    ]);

    let error = codec(provider)
        .decode_response(&core_request, tool_call_response("call_history"))
        .expect_err("provider call IDs are unique across history");

    assert_eq!(
        error.code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );
}

#[test]
fn usage_overflow_fails_instead_of_truncating_core_counters() {
    let provider = ProviderId::OpenCodeGo;
    let mut fixture = response(
        json!({
            "content": "{\"type\":\"respond\",\"message\":\"done\"}",
            "refusal": null
        }),
        "stop",
    );
    fixture.usage.as_mut().expect("usage").total_tokens = u64::from(u32::MAX) + 1;

    let error = codec(provider)
        .decode_response(&request(provider), fixture)
        .expect_err("core usage counters must not truncate");

    assert_eq!(
        error.code(),
        ProviderErrorCode::ProtocolInvalidResponse.as_str()
    );
}

#[test]
fn text_action_and_usage_decode_without_retaining_raw_content() {
    let provider = ProviderId::MiniMax;
    let decoded = codec(provider)
        .decode_response(
            &request(provider),
            response(
                json!({
                    "content": "{\"type\":\"request_clarification\",\"question\":\"Continue?\"}",
                    "refusal": null
                }),
                "stop",
            ),
        )
        .expect("valid structured action");

    assert!(matches!(
        decoded.action,
        AgentAction::RequestClarification { ref question } if question == "Continue?"
    ));
    assert_eq!(decoded.usage.expect("usage").total_tokens, 14);
    assert_eq!(decoded.raw_content, None);
}

#[test]
fn chatgpt_and_mismatched_prefixes_cannot_enter_the_chat_codec() {
    assert_eq!(
        LiterChatCodec::new(
            ProviderId::ChatGptSubscription,
            ParameterApplicability::Supported,
        )
        .expect_err("ChatGPT uses Responses")
        .code(),
        ProviderErrorCode::ProtocolIncompatible.as_str()
    );

    let provider = ProviderId::Xai;
    let mut wrong = request(provider);
    wrong.model = "deepseek/fixture-model".to_owned();
    let error = codec(provider)
        .encode_request(&wrong)
        .expect_err("wrong prefix");
    assert_eq!(error.code(), ProviderErrorCode::InvalidModelPrefix.as_str());
    assert_eq!(error.field(), Some(&ProviderField::Model));
}
