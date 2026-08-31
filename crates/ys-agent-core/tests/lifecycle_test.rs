use ys_agent_core::{
    Capability, Principal, Run, RunStatus, Session, Task, TaskId, TaskStatus, WorkflowKind,
    WorkspaceId,
};

#[test]
fn new_session_and_task_have_separate_lifecycles() {
    let principal = Principal::local_operator("ysc");
    let session = Session::new(WorkspaceId::new(), principal.id);
    let task = Task::new(
        session.workspace_id,
        principal.id,
        "Query the last seven complete days of GMV",
    );
    assert_ne!(session.id.to_string(), task.id.to_string());
    assert_eq!(task.status, TaskStatus::Open);
}

#[test]

fn local_operator_has_only_v02_query_capability() {
    let principal = Principal::local_operator("ysc");
    assert!(principal.capabilities.contains(&Capability::DataQuery));
    assert_eq!(principal.capabilities.len(), 1);
}

#[test]
fn terminal_run_cannot_resume() {
    let mut run = Run::new(TaskId::new(), WorkflowKind::Query);

    run.start().expect("queued to running");
    run.succeed().expect("running to succeeded");

    let error = run.resume().expect_err("terminal run must not resume");

    assert!(matches!(
        error,
        ys_agent_core::CoreError::InvalidTransition { .. }
    ));

    assert_eq!(run.status, RunStatus::Succeeded);
}

#[test]
fn first_persistable_running_snapshot_has_version_one() {
    let mut run = Run::new(TaskId::new(), WorkflowKind::Query);

    run.start().expect("queued to running");

    assert_eq!(run.version, 1);
}

#[test]
fn first_persistable_queued_snapshot_has_version_one() {
    let run = Run::new(TaskId::new(), WorkflowKind::Query);

    let snapshot = run.snapshot(serde_json::json!({}), None, None, None);

    assert_eq!(snapshot.version, 1);
}

#[test]
fn waiting_does_not_create_a_new_run() {
    let mut run = Run::new(TaskId::new(), WorkflowKind::Query);

    let original_run_id = run.id;

    run.start().expect("queued to running");

    run.wait_for_input("clarification-1")
        .expect("running to waiting");

    assert_eq!(run.status, RunStatus::WaitingForInput);
    assert_eq!(run.id, original_run_id);

    run.resume().expect("waiting to running");

    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(run.id, original_run_id);
}

#[test]
fn completed_task_cannot_return_to_in_progress() {
    let principal = Principal::local_operator("ysc");
    let mut task = Task::new(WorkspaceId::new(), principal.id, "Query GMV");
    task.start().expect("open to in progress");
    task.complete().expect("in progress to completed");
    assert!(task.resume().is_err());
    assert_eq!(task.status, TaskStatus::Completed)
}
