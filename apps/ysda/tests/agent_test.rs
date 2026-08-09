mod support;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use ysda::agent::QueryAgent;
use ysda::domain::UserQuestion;
use ysda::llm::{LlmClient, LlmConfig};
use ysda::trace::TraceRecorder;

#[tokio::test]
async fn completes_question_to_safe_query_result_and_trace() {
    let database = support::create_test_database();
    let trace_directory = database.directory.path().join("traces");
    let server = MockServer::start().await;
    Mock::given(method("Post"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices":[{
                "message":{
                    "content": "{\"sql\":\"SELECT name FROM customers ORDER BY id\",\"explanation\":\"List customers\"}"
                }
            }]
        })))
        .mount(&server)
        .await;

    let agent = QueryAgent::new(
        LlmClient::new(LlmConfig {
            api_key: "test-key".to_owned(),
            base_url: server.uri(),
            model: "test-model".to_owned(),
        }),
        TraceRecorder::new(&trace_directory),
        100,
    );

    let run = agent
        .run(&database.path, UserQuestion::new("list customers"))
        .await
        .expect("trace persistence should succeed");

    assert!(run.error.is_none());
    assert!(run.policy_decision.expect("policy decision").allowed);
    assert_eq!(run.result.expect("query result").row_count, 2);
    assert!(
        trace_directory
            .join(format!("{}.json", run.run_id))
            .exists()
    );
}

#[tokio::test]
async fn records_policy_rejection_without_executing_sql() {
    let database = support::create_test_database();
    let trace_directory = database.directory.path().join("traces");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "content": "{\"sql\":\"DELETE FROM customers\",\"explanation\":\"unsafe test\"}"
                }
            }]
        })))
        .mount(&server)
        .await;
    let agent = QueryAgent::new(
        LlmClient::new(LlmConfig {
            api_key: "test-key".to_owned(),
            base_url: server.uri(),
            model: "test-model".to_owned(),
        }),
        TraceRecorder::new(&trace_directory),
        100,
    );

    let run = agent
        .run(&database.path, UserQuestion::new("delete customers"))
        .await
        .expect("failure trace should still save");

    assert_eq!(
        run.error.expect("run should fail").category,
        "UnsafeSqlError"
    );
    assert!(run.result.is_none());
}
