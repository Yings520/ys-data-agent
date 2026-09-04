use liter_llm::types::ResponseObject;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use ys_agent_core::{
    AgentAction, AssistantToolCall, ContextManifest, CredentialLease, ModelMessage, ModelRequest,
    ModelResponseFormat, ModelRole, ModelToolChoice, ParameterApplicability, ProviderErrorCode,
    ProviderField, ProviderParameterKey, SecretValue, Sensitivity, SideEffect, ToolRisk, ToolSpec,
};

use super::{
    CHATGPT_RESPONSES_ACCEPT, CHATGPT_RESPONSES_BACKEND, CHATGPT_RESPONSES_ORIGINATOR,
    ChatGptResponsesCodec,
};

const FIXTURE_ACCESS_TOKEN: &str = "fixture-access-token";
const FIXTURE_ACCOUNT_ID: &str = "fixture-account";

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

fn request() -> ModelRequest {
    ModelRequest {
        model: "chatgpt/fixture-model".to_owned(),
        messages: vec![
            message(ModelRole::System, "Use only the supplied fixture tool."),
            message(ModelRole::User, "Inspect the synthetic source."),
        ],
        tools: vec![tool_spec()],
        context_manifest: ContextManifest::empty(8_000),
        temperature: Some(0.2),
        tool_choice: ModelToolChoice::Auto,
        response_format: ModelResponseFormat::JsonObject,
    }
}

fn codec() -> ChatGptResponsesCodec {
    ChatGptResponsesCodec::new(ParameterApplicability::Supported)
}

fn response(output: Value) -> ResponseObject {
    serde_json::from_value(json!({
        "id": "fixture-response",
        "object": "response",
        "created_at": 1,
        "model": "fixture-model",
        "status": "completed",
        "output": output,
        "usage": {
            "input_tokens": 10,
            "output_tokens": 4,
            "total_tokens": 14
        },
        "error": null
    }))
    .expect("valid Responses fixture")
}

fn tool_call_response(call_id: &str) -> ResponseObject {
    response(json!([{
        "type": "function_call",
        "call_id": call_id,
        "name": "inspect_schema",
        "arguments": "{\"source\":\"fixture\"}"
    }]))
}

fn connected_lease() -> CredentialLease {
    oauth_lease(4_102_444_800_u64)
}

fn oauth_lease(expires_at_epoch_seconds: u64) -> CredentialLease {
    let header = base64_url_encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64_url_encode(
        serde_json::to_string(&json!({
            "iss": "https://auth.openai.com",
            "aud": "app_EMoamEEZ73f0CkXaXp7hrann",
            "sub": "fixture-subject",
            "exp": expires_at_epoch_seconds,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": FIXTURE_ACCOUNT_ID
            }
        }))
        .expect("JWT claims serialize")
        .as_bytes(),
    );
    let serialized = json!({
        "schema_version": 1,
        "access_token": FIXTURE_ACCESS_TOKEN,
        "refresh_token": "fixture-refresh-token",
        "id_token": format!("{header}.{payload}.fixture-signature"),
        "account_id": FIXTURE_ACCOUNT_ID,
        "subject": "fixture-subject",
        "expires_at_epoch_seconds": expires_at_epoch_seconds
    })
    .to_string();
    CredentialLease::new(SecretValue::from_utf8(serialized))
}

#[test]
fn fixed_responses_config_uses_connected_oauth_and_redacts_values() {
    let config = codec()
        .client_config(&connected_lease())
        .expect("connected OAuth config");

    assert_eq!(config.base_url.as_deref(), Some(CHATGPT_RESPONSES_BACKEND));
    assert!(!config.load_env);
    assert_eq!(config.max_retries, 0);
    assert_eq!(config.api_key.expose_secret(), FIXTURE_ACCESS_TOKEN);
    assert_eq!(
        config.headers(),
        [
            (
                "ChatGPT-Account-ID".to_owned(),
                FIXTURE_ACCOUNT_ID.to_owned()
            ),
            (
                "originator".to_owned(),
                CHATGPT_RESPONSES_ORIGINATOR.to_owned()
            ),
            ("user-agent".to_owned(), "codex_cli_rs/0.152.1".to_owned()),
            ("accept".to_owned(), CHATGPT_RESPONSES_ACCEPT.to_owned()),
        ]
    );
    let debug = format!("{config:?}");
    assert!(!debug.contains(FIXTURE_ACCESS_TOKEN));
    assert!(!debug.contains(FIXTURE_ACCOUNT_ID));
}

#[test]
fn request_and_multi_turn_tool_result_preserve_the_provider_call_id() {
    let mut core_request = request();
    let decoded = codec()
        .decode_response(&core_request, tool_call_response("call_fixture_123"))
        .expect("valid Responses tool call");
    let AgentAction::CallTool { call } = decoded.action else {
        panic!("expected structured tool call");
    };
    assert_eq!(call.provider_call_id.as_deref(), Some("call_fixture_123"));
    assert_eq!(call.version, "fixture-v1");
    assert_eq!(decoded.raw_content, None);

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

    let encoded = codec()
        .encode_request(&core_request)
        .expect("valid Responses tool result round trip");
    let value = serde_json::to_value(encoded).expect("serialize Responses request");
    assert_eq!(value["model"], "fixture-model");
    assert_eq!(value["input"][0]["role"], "system");
    assert_eq!(value["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(value["input"][2]["type"], "function_call");
    assert_eq!(value["input"][2]["call_id"], "call_fixture_123");
    assert_eq!(value["input"][3]["type"], "function_call_output");
    assert_eq!(value["input"][3]["call_id"], "call_fixture_123");
    assert_eq!(value["tools"][0]["type"], "function");
    assert_eq!(value["extra_body"]["tool_choice"], "auto");
    assert_eq!(value["extra_body"]["parallel_tool_calls"], false);
    assert_eq!(value["extra_body"]["store"], false);
    assert_eq!(value["extra_body"]["text"]["format"]["type"], "json_object");
}

#[test]
fn text_action_and_usage_decode_without_retaining_raw_payload() {
    let decoded = codec()
        .decode_response(
            &request(),
            response(json!([{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "{\"type\":\"request_clarification\",\"question\":\"Continue?\"}"
                }]
            }])),
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
fn reasoning_before_a_single_tool_call_is_not_treated_as_a_protocol_failure() {
    let decoded = codec()
        .decode_response(
            &request(),
            response(json!([
                {
                    "type": "reasoning",
                    "id": "rsn_fixture",
                    "summary": [],
                    "content": []
                },
                {
                    "type": "function_call",
                    "call_id": "call_reasoned_123",
                    "name": "inspect_schema",
                    "arguments": "{\"source\":\"fixture\"}"
                }
            ])),
        )
        .expect("reasoning plus one tool call is a valid Responses result");

    assert!(matches!(
        decoded.action,
        AgentAction::CallTool { ref call }
            if call.provider_call_id.as_deref() == Some("call_reasoned_123")
    ));
    assert_eq!(decoded.raw_content, None);
}

#[test]
fn blank_missing_duplicate_and_conflicting_ids_fail_closed() {
    let blank = codec()
        .decode_response(&request(), tool_call_response("   "))
        .expect_err("blank Responses call ID");
    assert_eq!(
        blank.code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );

    let mut missing = request();
    missing.messages.push(ModelMessage {
        role: ModelRole::Tool,
        content: "{}".to_owned(),
        tool_call_id: None,
        name: Some("inspect_schema".to_owned()),
        assistant_tool_call: None,
    });
    assert_eq!(
        codec()
            .encode_request(&missing)
            .expect_err("missing tool result ID")
            .code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );

    let mut duplicate = request();
    for _ in 0..2 {
        duplicate.messages.push(ModelMessage {
            role: ModelRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            name: None,
            assistant_tool_call: Some(AssistantToolCall {
                provider_call_id: "call_duplicate".to_owned(),
                name: "inspect_schema".to_owned(),
                arguments: json!({ "source": "fixture" }),
            }),
        });
    }
    assert_eq!(
        codec()
            .encode_request(&duplicate)
            .expect_err("duplicate provider call ID")
            .code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );

    let mut conflict = request();
    conflict.messages.extend([
        ModelMessage {
            role: ModelRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            name: None,
            assistant_tool_call: Some(AssistantToolCall {
                provider_call_id: "call_conflict".to_owned(),
                name: "inspect_schema".to_owned(),
                arguments: json!({ "source": "fixture" }),
            }),
        },
        ModelMessage {
            role: ModelRole::Tool,
            content: "{}".to_owned(),
            tool_call_id: Some("call_conflict".to_owned()),
            name: Some("different_tool".to_owned()),
            assistant_tool_call: None,
        },
    ]);
    assert_eq!(
        codec()
            .encode_request(&conflict)
            .expect_err("tool name must agree with declared call")
            .code(),
        ProviderErrorCode::ProtocolInvalidToolCallId.as_str()
    );
}

#[test]
fn non_connected_oauth_and_incompatible_models_or_parameters_fail_closed() {
    let oauth = codec()
        .client_config(&CredentialLease::new(SecretValue::from_utf8(
            "not-an-oauth-bundle".to_owned(),
        )))
        .expect_err("unreadable bundle is not connected");
    assert_eq!(oauth.code(), ProviderErrorCode::OAuthNotConnected.as_str());
    assert_eq!(oauth.field(), Some(&ProviderField::OAuth));

    let expired = codec()
        .client_config(&oauth_lease(1))
        .expect_err("expired OAuth connection cannot construct a client");
    assert_eq!(
        expired.code(),
        ProviderErrorCode::OAuthNotConnected.as_str()
    );

    let mut wrong_model = request();
    wrong_model.model = "openai/fixture-model".to_owned();
    assert_eq!(
        codec()
            .encode_request(&wrong_model)
            .expect_err("OpenAI API model must not enter ChatGPT codec")
            .code(),
        ProviderErrorCode::InvalidModelPrefix.as_str()
    );

    let incompatible = ChatGptResponsesCodec::new(ParameterApplicability::Conditional)
        .encode_request(&request())
        .expect_err("conditional temperature cannot be silently dropped");
    assert_eq!(
        incompatible.code(),
        ProviderErrorCode::ModelIncompatible.as_str()
    );
    assert_eq!(
        incompatible.field(),
        Some(&ProviderField::Parameter(ProviderParameterKey::Temperature))
    );
}

fn base64_url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(third & 0x3f) as usize] as char);
        }
    }
    output
}
