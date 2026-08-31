#[allow(dead_code)]
mod support;

use support::PersistentRuntimeFixture;
use ys_agent_core::{CommandId, RunStatus, RuntimeStore};
use ys_agent_runtime::AgentServiceApi;

#[tokio::test]
async fn waiting_for_input_resumes_the_same_run_after_store_reopen() {
    let mut fixture = PersistentRuntimeFixture::new().await;
    let first = fixture.run_until_clarification("Show GMV recently").await;
    let original_run_id = first.run_id;
    fixture.close_runtime();

    let reopened = fixture.reopen().await.expect("reopen Runtime");
    reopened
        .service
        .answer_clarification(
            CommandId::new(),
            &original_run_id,
            "Use seven complete days in UTC".to_owned(),
        )
        .await
        .expect("answer clarification");
    let completed = reopened.run_to_terminal(&original_run_id).await;

    assert_eq!(completed.run_id, original_run_id);
    assert_eq!(completed.status, RunStatus::Succeeded);
}

#[tokio::test]
async fn started_read_tool_without_terminal_event_becomes_unknown_then_new_call() {
    let mut fixture = PersistentRuntimeFixture::crash_after_low_cost_tool_started().await;
    let original_call = fixture.original_tool_call_id();
    fixture.close_runtime();

    let reopened = fixture.reopen().await.expect("reopen Runtime");
    let completed = reopened.resume_to_terminal(CommandId::new()).await;

    assert_eq!(completed.status, RunStatus::Succeeded);
    assert!(reopened.has_indeterminate_event(&original_call).await);
    assert_ne!(reopened.successful_tool_call_id().await, original_call);
}

#[tokio::test]
async fn unknown_high_cost_query_waits_for_confirmation() {
    let mut fixture = PersistentRuntimeFixture::crash_after_high_cost_tool_started().await;
    fixture.close_runtime();

    let reopened = fixture.reopen().await.expect("reopen Runtime");
    let result = reopened.resume(CommandId::new()).await.expect("resume Run");

    assert_eq!(result.status, RunStatus::WaitingForInput);
    assert_eq!(result.pending_reason(), "confirm_high_cost_retry");
    assert_eq!(reopened.tool_execution_count(), 0);
}

#[tokio::test]
async fn a_terminal_failed_run_is_never_resumed_in_place() {
    let fixture = PersistentRuntimeFixture::failed_run().await;
    let new_run = fixture
        .service()
        .resume_task(CommandId::new(), &fixture.task_id)
        .await
        .expect("retry failed Run");

    assert_ne!(new_run, fixture.failed_run_id);
    let retry = fixture
        .runtime()
        .load_run(&new_run)
        .await
        .expect("load retry");
    assert_eq!(retry.retry_of_run_id, Some(fixture.failed_run_id));
}

#[tokio::test]
async fn event_sequence_gap_is_reported_as_corrupt_history() {
    let fixture = PersistentRuntimeFixture::with_event_sequence_gap().await;
    let error = fixture
        .recovery()
        .reconstruct(&fixture.run_id)
        .await
        .expect_err("sequence gap must fail closed");

    assert_eq!(error.code(), "corrupt_run_history");
}
