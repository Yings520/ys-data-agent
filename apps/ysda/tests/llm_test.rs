use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use ysda::domain::{ColumnSchema, SchemaSnapshot, TableSchema, UserQuestion};
use ysda::llm::{LlmClient, LlmConfig};

#[tokio::test]
async fn generates_structured_query_from_chat_completion() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "content": "{\"sql\":\"SELECT name FROM customers ORDER BY id\",\"explanation\":\"List customers\"}"
                }
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = LlmClient::new(LlmConfig {
        api_key: "test-key".to_owned(),
        base_url: server.uri(),
        model: "test-model".to_owned(),
    });
    let schema = SchemaSnapshot {
        tables: vec![TableSchema {
            name: "customers".to_owned(),
            columns: vec![ColumnSchema {
                name: "name".to_owned(),
                data_type: "TEXT".to_owned(),
                not_null: true,
                primary_key_position: 0,
            }],
        }],
    };

    let generated = client
        .generate(&UserQuestion::new("list customers"), &schema)
        .await
        .expect("mocked model response should decode");

    assert_eq!(generated.sql, "SELECT name FROM customers ORDER BY id");
    assert_eq!(generated.explanation, "List customers");
}

#[tokio::test]
async fn rejects_non_json_model_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": "```sql SELECT 1 ```"}}]
        })))
        .mount(&server)
        .await;

    let client = LlmClient::new(LlmConfig {
        api_key: "test-key".to_owned(),
        base_url: server.uri(),
        model: "test-model".to_owned(),
    });
    let error = client
        .generate(&UserQuestion::new("one"), &SchemaSnapshot::default())
        .await
        .expect_err("markdown response must be rejected");

    assert_eq!(error.category(), "ModelError");
}
