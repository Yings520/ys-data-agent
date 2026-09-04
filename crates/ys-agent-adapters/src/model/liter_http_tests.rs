//! Loopback transport contracts for the governed `liter-llm` client path.
//!
//! `ClientConfig::base_url` exists here solely to route a `DefaultClient` to an in-process
//! fixture. Production construction remains exclusively in `LiterProviderFactory`, where no
//! profile or caller can supply a URL and every config has `load_env(false)`.

use liter_llm::client::ClientConfigBuilder;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};
use ys_agent_core::{
    AgentAction, AssistantToolCall, ContextManifest, ModelMessage, ModelProvider, ModelRequest,
    ModelRole, ParameterApplicability, ProviderErrorCode, ProviderId, Sensitivity, SideEffect,
    ToolRisk, ToolSpec,
};

use super::{ClientPlan, LiterChatCodec, LiterModelProvider};
use crate::model::liter_responses::ChatGptResponsesCodec;

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
const FIXTURE_CALL_ID: &str = "call_fixture_123";
const CONTEXT_CANARY: &str = "customer-context-canary-must-not-reach-provider";
const RAW_BODY_CANARY: &str = "provider-raw-body-canary-must-not-leak";

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

fn request(model: String) -> ModelRequest {
    ModelRequest {
        model,
        messages: vec![
            message(ModelRole::System, "Use only the supplied fixture tool."),
            message(ModelRole::User, "Inspect the synthetic source."),
        ],
        tools: vec![tool_spec()],
        // Context is part of the stable core request, but is not provider payload. The server
        // checks this canary is absent from the real wire body.
        context_manifest: ContextManifest::empty(8_000).omit(CONTEXT_CANARY, "fixture"),
        temperature: Some(0.2),
        tool_choice: ys_agent_core::ModelToolChoice::Auto,
        response_format: ys_agent_core::ModelResponseFormat::JsonObject,
    }
}

fn fixture_config(server: &MockServer, retries: u32) -> liter_llm::client::ClientConfig {
    ClientConfigBuilder::new("fixture-transport-credential")
        .load_env(false)
        .base_url(server.uri())
        .max_retries(retries)
        .build()
}

fn chat_client(provider: ProviderId, server: &MockServer, retries: u32) -> LiterModelProvider {
    let model = format!("{}fixture-model", provider.model_prefix());
    LiterModelProvider::from_plan(ClientPlan::Chat {
        model_hint: model.clone(),
        model,
        temperature: Some(0.2),
        config: fixture_config(server, retries),
        codec: LiterChatCodec::new(provider, ParameterApplicability::Supported)
            .expect("governed Chat codec"),
    })
    .expect("loopback Chat client")
}

fn responses_client(server: &MockServer, retries: u32) -> LiterModelProvider {
    let model = "chatgpt/fixture-model".to_owned();
    LiterModelProvider::from_plan(ClientPlan::Responses {
        model,
        temperature: Some(0.2),
        config: ClientConfigBuilder::new("fixture-transport-credential")
            .load_env(false)
            .base_url(server.uri())
            .max_retries(retries)
            .header("ChatGPT-Account-ID", "fixture-account")
            .expect("fixture account header")
            .header("originator", "codex_cli_rs")
            .expect("fixture originator header")
            .header("accept", "text/event-stream")
            .expect("fixture stream accept header")
            .build(),
        codec: ChatGptResponsesCodec::new(ParameterApplicability::Supported),
    })
    .expect("loopback Responses client")
}

fn is_follow_up(request: &Request) -> bool {
    let body = String::from_utf8_lossy(&request.body);
    body.contains("\"tool_call_id\"")
        || body.contains("\"tool_result\"")
        || body.contains("\"function_call_output\"")
}

fn chat_response(follow_up: bool) -> ResponseTemplate {
    let message = if follow_up {
        json!({
            "content": "{\"type\":\"request_clarification\",\"question\":\"fixture complete?\"}",
            "tool_calls": null,
            "refusal": null
        })
    } else {
        json!({
            "content": null,
            "tool_calls": [{
                "id": FIXTURE_CALL_ID,
                "type": "function",
                "function": {
                    "name": "inspect_schema",
                    "arguments": "{\"source\":\"fixture\"}"
                }
            }],
            "refusal": null
        })
    };
    let finish_reason = if follow_up { "stop" } else { "tool_calls" };
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "fixture-chat-response",
        "object": "chat.completion",
        "created": 1,
        "model": "fixture-model",
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
            "logprobs": null
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14 }
    }))
}

fn anthropic_response(follow_up: bool) -> ResponseTemplate {
    let content = if follow_up {
        json!([{
            "type": "text",
            "text": "{\"type\":\"request_clarification\",\"question\":\"fixture complete?\"}"
        }])
    } else {
        json!([{
            "type": "tool_use",
            "id": FIXTURE_CALL_ID,
            "name": "inspect_schema",
            "input": { "source": "fixture" }
        }])
    };
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "fixture-anthropic-response",
        "type": "message",
        "role": "assistant",
        "model": "fixture-model",
        "content": content,
        "stop_reason": if follow_up { "end_turn" } else { "tool_use" },
        "usage": { "input_tokens": 10, "output_tokens": 4 }
    }))
}

fn responses_stream_response(follow_up: bool) -> ResponseTemplate {
    let item = if follow_up {
        json!({
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": "{\"type\":\"request_clarification\",\"question\":\"fixture complete?\"}"
            }]
        })
    } else {
        json!({
            "type": "function_call",
            "call_id": FIXTURE_CALL_ID,
            "name": "inspect_schema",
            "arguments": "{\"source\":\"fixture\"}"
        })
    };
    let output_item_done = json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": item,
    });
    // Codex's terminal event intentionally carries only turn metadata. Its completed response
    // items arrive earlier as `response.output_item.done` events, unlike the public JSON
    // Responses endpoint that returns a full response object here.
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "fixture-responses-response",
            "usage": { "input_tokens": 10, "output_tokens": 4, "total_tokens": 14 },
        }
    });
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(format!(
            "event: response.output_item.done\ndata: {output_item_done}\n\n\
             event: response.completed\ndata: {completed}\n\n"
        ))
}

async fn mount_success_fixture(server: &MockServer) {
    Mock::given(method("POST"))
        .respond_with(|request: &Request| match request.url.path() {
            "/messages" => anthropic_response(is_follow_up(request)),
            "/responses" => responses_stream_response(is_follow_up(request)),
            "/chat/completions" => chat_response(is_follow_up(request)),
            _ => ResponseTemplate::new(404),
        })
        .mount(server)
        .await;
}

fn append_tool_result(request: &mut ModelRequest, call: AssistantToolCall) {
    let call_id = call.provider_call_id.clone();
    request.messages.push(ModelMessage {
        role: ModelRole::Assistant,
        content: String::new(),
        tool_call_id: None,
        name: None,
        assistant_tool_call: Some(call),
    });
    request.messages.push(ModelMessage {
        role: ModelRole::Tool,
        content: "{\"relations\":[]}".to_owned(),
        tool_call_id: Some(call_id),
        name: Some("inspect_schema".to_owned()),
        assistant_tool_call: None,
    });
}

async fn assert_multi_turn_transport(client: LiterModelProvider, mut core_request: ModelRequest) {
    let first = client
        .complete(core_request.clone())
        .await
        .expect("fixture tool call must cross the real client");
    let AgentAction::CallTool { call } = first.action else {
        panic!("fixture must return a tool call");
    };
    assert_eq!(call.provider_call_id.as_deref(), Some(FIXTURE_CALL_ID));
    assert_eq!(first.raw_content, None);

    append_tool_result(
        &mut core_request,
        AssistantToolCall {
            provider_call_id: call.provider_call_id.expect("fixture Provider call ID"),
            name: call.name,
            arguments: call.arguments,
        },
    );
    let second = client
        .complete(core_request)
        .await
        .expect("fixture tool result must preserve the Provider call ID");
    assert!(matches!(
        second.action,
        AgentAction::RequestClarification { ref question } if question == "fixture complete?"
    ));
    assert_eq!(second.raw_content, None);
}

#[tokio::test]
async fn nine_governed_provider_fixtures_drive_real_liter_clients() {
    let server = MockServer::start().await;
    mount_success_fixture(&server).await;

    for provider in CHAT_PROVIDERS {
        assert_multi_turn_transport(
            chat_client(provider, &server, 0),
            request(format!("{}fixture-model", provider.model_prefix())),
        )
        .await;
    }
    assert_multi_turn_transport(
        responses_client(&server, 0),
        request("chatgpt/fixture-model".to_owned()),
    )
    .await;

    let requests = server
        .received_requests()
        .await
        .expect("fixture request capture");
    assert_eq!(requests.len(), 18);
    assert!(
        requests
            .iter()
            .all(|request| request.method.as_str() == "POST")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.url.path() == "/messages")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.url.path() == "/chat/completions")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.url.path() == "/responses")
    );
    assert!(requests.iter().all(|request| {
        let body = String::from_utf8_lossy(&request.body);
        body.contains("\"temperature\":0.2")
            && !body.contains(CONTEXT_CANARY)
            && !body.contains(RAW_BODY_CANARY)
    }));
    let responses_requests: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path() == "/responses")
        .collect();
    assert_eq!(responses_requests.len(), 2);
    assert!(responses_requests.iter().all(|request| {
        let body = String::from_utf8_lossy(&request.body);
        body.contains("\"stream\":true")
            && request
                .headers
                .get("accept")
                .is_some_and(|value| value == "text/event-stream")
            && request
                .headers
                .get("user-agent")
                .is_some_and(|value| value == "codex_cli_rs/0.152.1")
            && request
                .headers
                .get("originator")
                .is_some_and(|value| value == "codex_cli_rs")
            && request
                .headers
                .get("chatgpt-account-id")
                .is_some_and(|value| value == "fixture-account")
    }));
}

#[tokio::test]
async fn codex_sse_terminal_failure_is_sanitized() {
    let server = MockServer::start().await;
    let failed = json!({
        "type": "response.failed",
        "response": { "id": "fixture-failed-response" },
        "error": { "message": RAW_BODY_CANARY },
    });
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("event: response.failed\\ndata: {failed}\\n\\n")),
        )
        .mount(&server)
        .await;

    let error = responses_client(&server, 0)
        .complete(request("chatgpt/fixture-model".to_owned()))
        .await
        .expect_err("Codex terminal failure must not be decoded as a successful response");
    assert_eq!(
        error.code(),
        ProviderErrorCode::ProtocolInvalidResponse.as_str()
    );
    assert!(!error.to_string().contains(RAW_BODY_CANARY));
    assert!(!format!("{error:?}").contains(RAW_BODY_CANARY));
}

#[tokio::test]
async fn transport_errors_are_sanitized_and_retry_only_the_bounded_classes() {
    for (status, retries, expected_requests, expected_code) in [
        (401, 2, 1, ProviderErrorCode::AuthenticationInvalid),
        (404, 2, 1, ProviderErrorCode::ModelNotFound),
        (429, 2, 3, ProviderErrorCode::RateLimited),
        (500, 2, 3, ProviderErrorCode::Server),
    ] {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(status).set_body_json(json!({
            "error": { "message": RAW_BODY_CANARY }
        }));
        if status == 429 {
            response = response.insert_header("Retry-After", "0");
        }
        Mock::given(method("POST"))
            .respond_with(response)
            .mount(&server)
            .await;

        let error = chat_client(ProviderId::DeepSeek, &server, retries)
            .complete(request("deepseek/fixture-model".to_owned()))
            .await
            .expect_err("fixture transport must fail");
        assert_eq!(error.code(), expected_code.as_str());
        assert!(!error.to_string().contains(RAW_BODY_CANARY));
        assert!(!format!("{error:?}").contains(RAW_BODY_CANARY));
        assert_eq!(
            server
                .received_requests()
                .await
                .expect("fixture request capture")
                .len(),
            expected_requests
        );
    }
}
