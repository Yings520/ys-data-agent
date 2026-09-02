use std::time::Duration;

use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
use ys_agent_adapters::model::{
    FakeModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider, ProviderCallTelemetry,
    ReplayModelProvider, SecretString,
};
use ys_agent_core::{
    AgentAction, AssistantToolCall, ContextManifest, CoreError, ModelMessage, ModelProvider,
    ModelRequest, ModelResponse, ModelRole, Sensitivity, SideEffect, ToolCall, ToolCallId,
    ToolRisk, ToolSpec,
};

fn schema_tool() -> ToolSpec {
    ToolSpec {
        name: "inspect_schema".to_owned(),
        description: "Read the schema of an allowed data source".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "source_id": { "type": "string" }
            },
            "required": ["source_id"],
            "additionalProperties": false
        }),
        output_schema: json!({
            "type": "object",
            "properties": {
                "relations": { "type": "array" }
            },
            "required": ["relations"]
        }),
        risk: ToolRisk::Low,
        side_effect: SideEffect::None,
        idempotent: true,
        timeout_ms: 5_000,
        max_output_bytes: 4_096,
        required_permissions: vec!["data_query".to_owned()],
        input_sensitivity: Sensitivity::Internal,
        output_sensitivity: Sensitivity::Internal,
        version: "1.0.0".to_owned(),
    }
}

fn model_request_with_schema_tool() -> ModelRequest {
    ModelRequest {
        model: "test-model".to_owned(),
        messages: vec![ModelMessage {
            role: ModelRole::User,
            content: "Which columns exist?".to_owned(),
            tool_call_id: None,
            name: None,
            assistant_tool_call: None,
        }],
        tools: vec![schema_tool()],
        context_manifest: ContextManifest::empty(8_000),
        temperature: Some(0.0),
    }
}

fn valid_config(base_url: String) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig {
        base_url,
        api_key: SecretString::new("test-secret".to_owned()),
        model: "test-model".to_owned(),
        supports_tool_calls: true,
        supports_tool_call_ids: true,
        supports_multi_turn_tool_results: true,
        context_window_tokens: 32_000,
        max_tool_schema_bytes: 16_384,
        request_timeout: Duration::from_secs(2),
    }
}

fn provider_for(server: &MockServer) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(valid_config(server.uri())).expect("valid provider")
}

#[tokio::test]
async fn converts_an_openai_compatible_tool_call_to_agent_action() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "inspect_schema",
                            "arguments": "{\"source_id\":\"warehouse\"}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 12,
                "total_tokens": 112
            }
        })))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let response = provider
        .complete(model_request_with_schema_tool())
        .await
        .expect("valid provider response");

    assert!(matches!(
        response.action,
        AgentAction::CallTool { ref call }
            if call.name == "inspect_schema"
                && call.provider_call_id.as_deref() == Some("call_123")
    ));
    assert_eq!(
        response.usage.as_ref().map(|usage| usage.total_tokens),
        Some(112)
    );
}

#[test]
fn rejects_a_provider_profile_without_tool_calling() {
    let mut config = valid_config("http://127.0.0.1:1".to_owned());
    config.supports_tool_calls = false;

    let error = OpenAiCompatibleProvider::new(config).expect_err("tool calling is required");

    assert!(matches!(error, CoreError::UnsupportedCapability(_)));
}

#[tokio::test]
async fn replay_provider_never_uses_the_network() {
    let provider = ReplayModelProvider::from_responses(vec![ModelResponse {
        action: AgentAction::RequestClarification {
            question: "Use seven complete days?".to_owned(),
        },
        raw_content: None,
        usage: None,
    }]);

    let response = provider
        .complete(ModelRequest {
            model: "test-model".to_owned(),
            messages: Vec::new(),
            tools: Vec::new(),
            context_manifest: ContextManifest::empty(1_000),
            temperature: Some(0.0),
        })
        .await
        .expect("one replay response");

    assert!(matches!(
        response.action,
        AgentAction::RequestClarification { .. }
    ));
}

#[tokio::test]
async fn returns_the_original_tool_call_id_with_a_tool_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "{\"type\":\"request_clarification\",\"question\":\"Continue?\"}"
                }
            }]
        })))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let mut request = model_request_with_schema_tool();
    request.messages = vec![
        ModelMessage {
            role: ModelRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            name: None,
            assistant_tool_call: Some(AssistantToolCall {
                provider_call_id: "call_original".to_owned(),
                name: "inspect_schema".to_owned(),
                arguments: json!({ "source_id": "warehouse" }),
            }),
        },
        ModelMessage {
            role: ModelRole::Tool,
            content: "{\"relations\":[]}".to_owned(),
            tool_call_id: Some("call_original".to_owned()),
            name: Some("inspect_schema".to_owned()),
            assistant_tool_call: None,
        },
    ];

    provider.complete(request).await.expect("valid response");

    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("request JSON");
    assert_eq!(body["messages"][0]["role"], "assistant");
    assert_eq!(body["messages"][0]["content"], serde_json::Value::Null);
    assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_original");
    assert_eq!(
        body["messages"][0]["tool_calls"][0]["function"]["name"],
        "inspect_schema"
    );
    assert_eq!(body["messages"][1]["role"], "tool");
    assert_eq!(body["messages"][1]["tool_call_id"], "call_original");
}

#[tokio::test]
async fn never_accepts_a_tool_call_from_free_form_content() {
    let server = MockServer::start().await;
    let content = serde_json::to_string(&AgentAction::CallTool {
        call: ToolCall {
            id: ToolCallId::new(),
            provider_call_id: Some("call_hidden".to_owned()),
            name: "inspect_schema".to_owned(),
            arguments: json!({ "source_id": "warehouse" }),
            version: "1.0.0".to_owned(),
        },
    })
    .expect("serialize action");

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": content
                }
            }]
        })))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .complete(model_request_with_schema_tool())
        .await
        .expect_err("content must not create a Tool Call");

    assert_eq!(error.code(), "tool_call_in_free_form_content");
}

#[tokio::test]
async fn invalid_typed_action_reports_the_safe_serde_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "{\"type\":\"unknown_action\"}"
                }
            }]
        })))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .complete(model_request_with_schema_tool())
        .await
        .expect_err("unknown action must fail");

    assert_eq!(error.code(), "invalid_model_response");
    assert!(error.to_string().contains("unknown variant"));
    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    assert_eq!(requests.len(), 2, "one protocol correction is allowed");
}

#[tokio::test]
async fn corrects_one_invalid_typed_action_without_replaying_its_content() {
    const CANARY: &str = "private-response-canary";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": format!(
                        r#"{{"type":"request_clarification","private":"{CANARY}"}}"#
                    )
                }
            }]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": r#"{"type":"request_clarification","question":"Which time range?"}"#
                }
            }]
        })))
        .mount(&server)
        .await;

    let response = provider_for(&server)
        .complete(model_request_with_schema_tool())
        .await
        .expect("one protocol correction should recover the typed action");

    assert!(matches!(
        response.action,
        AgentAction::RequestClarification { ref question } if question == "Which time range?"
    ));
    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    assert_eq!(requests.len(), 2);
    let retry_body: serde_json::Value =
        serde_json::from_slice(&requests[1].body).expect("request JSON");
    let correction = retry_body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("protocol correction message");
    assert!(correction.contains("PROTOCOL CORRECTION"));
    assert!(correction.contains("request_clarification"));
    assert!(!correction.contains(CANARY));
}

#[tokio::test]
async fn accepts_one_markdown_fence_around_an_otherwise_valid_typed_action() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```json\n{\"type\":\"request_clarification\",\"question\":\"Which range?\"}\n```"
                }
            }]
        })))
        .mount(&server)
        .await;

    let response = provider_for(&server)
        .complete(model_request_with_schema_tool())
        .await
        .expect("a single JSON fence is a transport wrapper");

    assert!(matches!(
        response.action,
        AgentAction::RequestClarification { .. }
    ));
}

#[tokio::test]
async fn rejects_prose_around_a_fenced_typed_action() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Here is the result:\n```json\n{\"type\":\"request_clarification\",\"question\":\"Which range?\"}\n```"
                }
            }]
        })))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .complete(model_request_with_schema_tool())
        .await
        .expect_err("prose must not be searched for embedded JSON");

    assert_eq!(error.code(), "invalid_model_response");
}

fn two_tools_request() -> ModelRequest {
    let mut request = model_request_with_schema_tool();
    let mut decoy = schema_tool();
    decoy.name = "resolve_metric".to_owned();
    request.tools.push(decoy);
    request
}

fn parallel_tool_calls_body() -> serde_json::Value {
    json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "call_one",
                        "type": "function",
                        "function": {
                            "name": "inspect_schema",
                            "arguments": "{\"source_id\":\"secret-source\",\"sql\":\"SELECT * FROM customers\"}"
                        }
                    },
                    {
                        "id": "call_two",
                        "type": "function",
                        "function": {
                            "name": "resolve_metric",
                            "arguments": "{\"metric_id\":\"commerce.gmv\"}"
                        }
                    }
                ]
            }
        }]
    })
}

#[tokio::test]
async fn retries_once_when_the_model_returns_parallel_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(parallel_tool_calls_body()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_retry",
                        "type": "function",
                        "function": {
                            "name": "inspect_schema",
                            "arguments": "{\"source_id\":\"warehouse\"}"
                        }
                    }]
                }
            }]
        })))
        .mount(&server)
        .await;

    let response = provider_for(&server)
        .complete(two_tools_request())
        .await
        .expect("protocol correction should recover a single Tool Call");

    assert!(matches!(
        response.action,
        AgentAction::CallTool { ref call } if call.name == "inspect_schema"
    ));
}

#[tokio::test]
async fn parallel_tool_calls_fail_closed_after_one_protocol_correction() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(parallel_tool_calls_body()))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .complete(two_tools_request())
        .await
        .expect_err("uncorrected parallel Tool Calls must fail");

    assert_eq!(error.code(), "parallel_tool_calls_disabled");
    let rendered = error.to_string();
    assert!(rendered.contains("received 2"));
    assert!(rendered.contains("inspect_schema"));
    assert!(rendered.contains("resolve_metric"));
    assert!(!rendered.contains("secret-source"));
    assert!(!rendered.contains("customers"));
    assert!(!rendered.contains("commerce.gmv"));
}

#[tokio::test]
async fn reports_invalid_structured_arguments_without_echoing_them() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_bad",
                        "type": "function",
                        "function": {
                            "name": "inspect_schema",
                            "arguments": "{\"source_id\":\"sensitive-canary\""
                        }
                    }]
                }
            }]
        })))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .complete(model_request_with_schema_tool())
        .await
        .expect_err("malformed arguments must fail");
    let rendered = error.to_string();

    assert_eq!(error.code(), "invalid_tool_arguments");
    assert!(!rendered.contains("sensitive-canary"));
}

#[tokio::test]
async fn secret_canary_never_appears_in_debug_errors_or_telemetry() {
    const CANARY: &str = "sk-secret-canary-must-not-leak";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(format!("provider echoed {CANARY}")),
        )
        .mount(&server)
        .await;

    let mut config = valid_config(server.uri());
    config.api_key = SecretString::new(CANARY.to_owned());
    assert!(!format!("{config:?}").contains(CANARY));

    let provider = OpenAiCompatibleProvider::new(config).expect("valid provider");
    assert!(!format!("{provider:?}").contains(CANARY));

    let error = provider
        .complete(model_request_with_schema_tool())
        .await
        .expect_err("401 must fail");
    assert_eq!(error.code(), "provider_authentication");
    assert!(!error.to_string().contains(CANARY));

    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    assert_eq!(requests.len(), 1, "authentication failures are not retried");

    let telemetry = ProviderCallTelemetry::new(
        "openai_compatible",
        CANARY,
        10,
        1,
        "provider_authentication",
    );
    let rendered = format!("{telemetry:?}");
    assert!(!rendered.contains(CANARY));
    assert!(rendered.contains("model_sha256"));
}

#[tokio::test]
async fn fake_provider_can_inspect_each_request() {
    let provider = FakeModelProvider::new(|request| async move {
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "inspect_schema");
        Ok(ModelResponse {
            action: AgentAction::RequestClarification {
                question: "Which warehouse?".to_owned(),
            },
            raw_content: None,
            usage: None,
        })
    });

    let response = provider
        .complete(model_request_with_schema_tool())
        .await
        .expect("fake response");

    assert!(matches!(
        response.action,
        AgentAction::RequestClarification { .. }
    ));
}

#[tokio::test]
async fn replay_exhaustion_is_a_typed_error() {
    let provider = ReplayModelProvider::from_responses(Vec::new());

    let error = provider
        .complete(model_request_with_schema_tool())
        .await
        .expect_err("empty replay must fail");

    assert!(matches!(error, CoreError::ReplayExhausted));
}

#[test]
fn all_providers_report_the_required_v0_2_capabilities() {
    let replay = ReplayModelProvider::from_responses(Vec::new());
    let fake = FakeModelProvider::new(|_request| async {
        Err::<ModelResponse, CoreError>(CoreError::ReplayExhausted)
    });

    for capabilities in [replay.capabilities(), fake.capabilities()] {
        assert!(capabilities.tool_calling);
        assert!(capabilities.structured_outputs);
        assert!(!capabilities.parallel_tool_calls);
        assert!(!capabilities.streaming);
        assert!(capabilities.max_context_tokens > 0);
    }
}

#[test]
fn fake_and_replay_remain_interchangeable_model_provider_ports() {
    fn accepts_model_provider(_: &dyn ModelProvider) {}

    let fake = FakeModelProvider::new(|_request| async {
        Err::<ModelResponse, CoreError>(CoreError::ReplayExhausted)
    });
    let replay = ReplayModelProvider::from_responses(Vec::new());

    accepts_model_provider(&fake);
    accepts_model_provider(&replay);
}
