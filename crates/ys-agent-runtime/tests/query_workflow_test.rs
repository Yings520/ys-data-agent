mod support;

use std::sync::Arc;

use async_trait::async_trait;
use support::{
    QueryWorkflowFixture, call_inspect_schema, call_query_data_execute, call_query_data_preflight,
    completion_response, propose_completion, propose_safe_adhoc_plan, propose_unsafe_adhoc_plan,
};
use ys_agent_core::{QueryIntent, RunStatus};
use ys_agent_runtime::telemetry::{
    TelemetryDispatcher, TelemetryError, TelemetryEvent, TelemetrySink,
};

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
async fn propose_completion_before_query_execution_is_rejected() {
    let fixture =
        QueryWorkflowFixture::with_model_actions(vec![completion_response("GMV is 10")]).await;

    let result = fixture.run("GMV").await.unwrap();

    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(result.failure_code(), Some("completion_gate_failed"));
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
