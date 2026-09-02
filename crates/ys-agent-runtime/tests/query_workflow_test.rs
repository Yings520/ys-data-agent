mod support;

use std::sync::Arc;

use async_trait::async_trait;
use support::{
    QueryWorkflowFixture, call_inspect_schema, call_query_data_execute, call_query_data_preflight,
    call_resolve_missing_metric, completion_response, propose_completion, propose_safe_adhoc_plan,
    propose_unsafe_adhoc_plan,
};
use ys_agent_core::{QueryIntent, RunStatus};
use ys_agent_runtime::telemetry::{
    TelemetryDispatcher, TelemetryError, TelemetryEvent, TelemetrySink,
};

fn request_for_phase<'a>(
    requests: &'a [ys_agent_core::ModelRequest],
    phase: &str,
) -> &'a ys_agent_core::ModelRequest {
    requests
        .iter()
        .find(|request| {
            request.messages.iter().any(|message| {
                message.role == ys_agent_core::ModelRole::System
                    && message.content.contains(&format!("PHASE: {phase}."))
            })
        })
        .unwrap_or_else(|| panic!("missing model request for phase {phase}"))
}

fn runtime_state(request: &ys_agent_core::ModelRequest) -> serde_json::Value {
    let content = request
        .messages
        .iter()
        .find_map(|message| message.content.strip_prefix("RUNTIME_QUERY_STATE_JSON:\n"))
        .expect("runtime-owned query state message");
    serde_json::from_str(content).expect("runtime query state JSON")
}

fn workflow_evidence(request: &ys_agent_core::ModelRequest) -> Vec<serde_json::Value> {
    request
        .messages
        .iter()
        .filter_map(|message| {
            message
                .content
                .strip_prefix("UNTRUSTED_WORKFLOW_EVIDENCE_JSON:\n")
        })
        .map(|content| serde_json::from_str(content).expect("workflow evidence JSON"))
        .collect()
}

#[derive(Debug)]
struct AlwaysFailTelemetrySink;

#[async_trait]
impl TelemetrySink for AlwaysFailTelemetrySink {
    async fn emit(&self, _event: TelemetryEvent) -> Result<(), TelemetryError> {
        Err(TelemetryError::Unavailable)
    }
}

#[tokio::test]
async fn failing_telemetry_sink_preserves_completed_query_run() {
    let telemetry = Arc::new(TelemetryDispatcher::new(Arc::new(AlwaysFailTelemetrySink)));
    let fixture = QueryWorkflowFixture::successful_metric_query_with_telemetry(telemetry).await;

    let result = fixture
        .run("GMV for the last seven complete days")
        .await
        .expect("query run succeeds despite telemetry failure");

    assert_eq!(result.status, RunStatus::Succeeded);
    assert!(fixture.has_run_completed_event(&result.run_id).await);
    assert!(fixture.telemetry_failure_count() > 0);
}

#[tokio::test]
async fn query_completion_requires_execution_verification_and_artifact() {
    let fixture = QueryWorkflowFixture::successful_metric_query().await;
    let result = fixture
        .run("GMV for the last seven complete days")
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Succeeded);
    let artifact = fixture.load_primary_query_artifact(&result).await;
    assert_eq!(
        artifact.metric.expect("metric reference").id,
        "commerce.gmv"
    );
    assert!(artifact.executed_sql.is_some());
    assert!(artifact.verification.hard_failures.is_empty());
    assert!(artifact.freshness.is_some());
}

#[tokio::test]
async fn each_live_model_phase_receives_the_runtime_identities_it_must_reuse() {
    let fixture = QueryWorkflowFixture::successful_metric_query().await;
    let result = fixture
        .run("GMV for the last seven complete days")
        .await
        .expect("query run");
    assert_eq!(result.status, RunStatus::Succeeded);

    let requests = fixture.model_requests().await;
    assert!(
        requests
            .iter()
            .all(|request| request.model == "deepseek/test-model"),
        "every Query model request must use the immutable Run-bound model"
    );
    let plan = runtime_state(request_for_phase(&requests, "Plan"));
    assert_eq!(plan["source_id"], "sqlite-demo");
    assert_eq!(plan["intent"], "governed_metric");
    assert!(
        plan["current_time_utc"]
            .as_str()
            .is_some_and(|value| value.ends_with('Z'))
    );
    assert_eq!(plan["workspace_timezone"], "UTC");
    assert!(plan["artifacts"]["metric_evidence"]["artifact_id"].is_string());
    assert!(plan["artifacts"]["metric_evidence"]["content_hash"].is_string());
    assert!(
        workflow_evidence(request_for_phase(&requests, "Plan"))
            .iter()
            .any(|evidence| evidence["content"]["id"] == "commerce.gmv")
    );

    let preflight = runtime_state(request_for_phase(&requests, "ValidateAndPreflight"));
    assert!(preflight["artifacts"]["execution_plan"]["artifact_id"].is_string());
    assert!(preflight["artifacts"]["execution_plan"]["content_hash"].is_string());

    let execute = runtime_state(request_for_phase(&requests, "Execute"));
    assert_eq!(
        execute["artifacts"]["execution_plan"],
        preflight["artifacts"]["execution_plan"]
    );
    assert!(execute["artifacts"]["preflight"]["artifact_id"].is_string());
    assert!(execute["artifacts"]["preflight"]["content_hash"].is_string());

    let verify = runtime_state(request_for_phase(&requests, "Verify"));
    assert!(verify["artifacts"]["query_result"]["artifact_id"].is_string());
    assert!(
        workflow_evidence(request_for_phase(&requests, "Verify"))
            .iter()
            .any(|evidence| evidence["content"]["time_column"] == "paid_at")
    );

    let complete = runtime_state(request_for_phase(&requests, "ReadyToComplete"));
    assert!(complete["artifacts"]["verification_report"]["artifact_id"].is_string());
    let completion_evidence = workflow_evidence(request_for_phase(&requests, "ReadyToComplete"));
    assert!(completion_evidence.iter().any(|evidence| {
        evidence["content"]["model_preview"]
            .as_str()
            .is_some_and(|preview| !preview.is_empty())
    }));
    assert!(
        completion_evidence
            .iter()
            .any(|evidence| evidence["content"]["hard_failures"].is_array())
    );
    assert!(
        completion_evidence
            .iter()
            .all(|evidence| evidence["content"].get("rows").is_none()),
        "raw result rows must not enter the model context"
    );
    let verify_evidence = workflow_evidence(request_for_phase(&requests, "Verify"));
    assert!(verify_evidence.iter().any(|evidence| {
        evidence["content"].get("time_column").is_some()
            || evidence["content"].get("latest_data_at").is_some()
            || evidence["content"].get("relation").is_some()
    }));
}

#[tokio::test]
async fn propose_completion_before_query_execution_is_rejected() {
    let fixture =
        QueryWorkflowFixture::with_model_actions(vec![completion_response("GMV is 10")]).await;

    let result = fixture.run("GMV").await.unwrap();

    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(result.failure_code(), Some("completion_gate_failed"));
}

#[tokio::test]
async fn a_metric_registry_miss_in_resolve_context_continues_as_adhoc() {
    let fixture = QueryWorkflowFixture::with_model_actions(vec![
        call_resolve_missing_metric(),
        call_inspect_schema(),
        propose_safe_adhoc_plan(),
        call_query_data_preflight(),
        call_query_data_execute(),
        propose_completion(),
    ])
    .await;

    let result = fixture
        .run("List distinct order channels")
        .await
        .expect("adhoc after metric miss");

    assert_eq!(result.status, RunStatus::Succeeded);
    assert_eq!(fixture.primary_artifact().intent, QueryIntent::AdHocRead);
    assert_eq!(fixture.tool_call_count("resolve_metric"), 1);
    assert_eq!(fixture.tool_call_count("inspect_schema"), 1);
}

#[tokio::test]
async fn invalid_sql_can_be_revised_by_the_model_without_transport_retry() {
    let fixture = QueryWorkflowFixture::with_model_actions(vec![
        call_inspect_schema(),
        propose_unsafe_adhoc_plan(),
        propose_safe_adhoc_plan(),
        call_query_data_preflight(),
        call_query_data_execute(),
        propose_completion(),
    ])
    .await;

    let result = fixture.run("List order channels").await.unwrap();

    assert_eq!(result.status, RunStatus::Succeeded);
    assert_eq!(fixture.tool_call_count("query_data"), 2);
    assert_eq!(fixture.transport_retry_count(), 0);
}

#[tokio::test]
async fn material_metric_ambiguity_waits_for_user_input() {
    let fixture = QueryWorkflowFixture::with_ambiguous_metrics().await;
    let result = fixture.run("Show GMV recently").await.unwrap();

    assert_eq!(result.status, RunStatus::WaitingForInput);
    assert_eq!(result.pending_reason(), "material_query_ambiguity");
}

#[tokio::test]
async fn metadata_query_completes_from_observed_evidence_without_sql() {
    let fixture = QueryWorkflowFixture::metadata_query().await;
    let result = fixture
        .run("What columns are in mart_orders?")
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Succeeded);
    assert_eq!(fixture.tool_call_count("query_data"), 0);
    assert_eq!(fixture.primary_artifact().intent, QueryIntent::Metadata);
}

#[tokio::test]
async fn empty_result_is_not_reported_as_zero() {
    let fixture = QueryWorkflowFixture::empty_metric_result().await;
    let result = fixture.run("GMV for 1990-01-01").await.unwrap();

    assert_eq!(result.status, RunStatus::Succeeded);
    assert!(
        fixture
            .primary_artifact()
            .warning_codes
            .contains(&"empty_result".to_owned())
    );
    assert!(
        !fixture
            .primary_artifact()
            .answer_summary
            .contains("GMV is 0")
    );
}
