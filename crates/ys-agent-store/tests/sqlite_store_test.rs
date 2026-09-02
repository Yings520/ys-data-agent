use std::fs;

use tempfile::TempDir;
use ys_agent_core::{
    ActiveProviderSnapshot, ArtifactKind, ArtifactStore, CommandId, CommandReceipt,
    CommandResultKind, CompatibilityEvidence, CoreError, CreateRunCommand, CredentialGeneration,
    CredentialKind, PendingRunEvent, ProfileId, ProfileRevision, ProviderId, ProviderModelId,
    ProviderParameters, PutArtifact, Run, RunEventKind, RunProviderBinding, RunSnapshot, RunStatus,
    RuntimeCommandBatch, RuntimeStore, Sensitivity, Task, ValidationVersions, WorkflowKind,
    WorkspaceId,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

struct StoreFixture {
    _directory: TempDir,
    store: SqliteRuntimeStore,
}

fn create_run(snapshot: RunSnapshot) -> CreateRunCommand {
    let profile_id = ProfileId::new();
    let versions =
        ValidationVersions::new("test-catalog", "test-probe", "test-liter", "test-codec");
    let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("test credential generation");
    let mut revision = ProfileRevision::draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/test-model")
            .expect("test model prefix"),
        ProviderParameters::default(),
        Some(credential),
    )
    .expect("test provider revision");
    let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
    revision
        .accept_validation(evidence, versions)
        .expect("test validation evidence");
    let active = ActiveProviderSnapshot::from_ready(&revision, 1).expect("active test Provider");
    let run_id = snapshot.run_id;
    CreateRunCommand::new(
        snapshot,
        RunProviderBinding::from_active(run_id, active).expect("test Run binding"),
        Vec::new(),
    )
    .expect("complete Run create command")
}

impl StoreFixture {
    async fn new() -> Self {
        let directory = TempDir::new().expect("temporary directory");
        let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
            .await
            .expect("open store");
        Self {
            _directory: directory,
            store,
        }
    }

    async fn seed_queued_run(&self) -> RunSnapshot {
        let workspace_id = WorkspaceId::new();
        let principal_id = ys_agent_core::PrincipalId::new();
        let task = Task::new(workspace_id, principal_id, "Query GMV");
        let run = Run::new(task.id, WorkflowKind::Query);
        let snapshot = run.snapshot(serde_json::json!({}), None, None, None);
        let command_id = CommandId::new();
        let fingerprint = format!("seed:{command_id}");
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::RunStarted,
            session_id: None,
            task_id: Some(task.id),
            run_id: Some(run.id),
            artifact_id: None,
            message: None,
            capability: None,
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: Some(task),
                create_run: Some(create_run(snapshot.clone())),
                new_artifact: None,
                pending_events: vec![],
                snapshot_update: None,
            })
            .await
            .expect("seed queued run");
        snapshot
    }
}

fn pending(kind: RunEventKind) -> PendingRunEvent {
    PendingRunEvent {
        actor: ys_agent_core::EventActor::System,
        kind,
    }
}

fn advanced_snapshot(current: &RunSnapshot, status: RunStatus) -> RunSnapshot {
    let mut next = current.clone();
    next.status = status;
    next.version += 1;
    next
}

#[tokio::test]
async fn append_is_atomic_and_optimistically_versioned() {
    let fixture = StoreFixture::new().await;
    let original = fixture.seed_queued_run().await;
    let running = advanced_snapshot(&original, RunStatus::Running);

    fixture
        .store
        .append(
            &original.run_id,
            original.version,
            vec![],
            vec![pending(RunEventKind::RunStarted)],
            &running,
        )
        .await
        .expect("first append");

    let error = fixture
        .store
        .append(
            &original.run_id,
            original.version,
            vec![],
            vec![pending(RunEventKind::RunResumed)],
            &running,
        )
        .await
        .expect_err("stale version must fail");
    assert!(matches!(error, CoreError::ConcurrencyConflict { .. }));
    let events = fixture
        .store
        .load_events(&original.run_id, 0)
        .await
        .expect("load events");
    assert!(
        events.len() >= 2,
        "each successful mutation must append its state projection"
    );
    let last_kind = serde_json::to_value(&events.last().expect("projected event").event.kind)
        .expect("serialize event kind");
    assert_eq!(last_kind["type"], "run_state_projected");
}

#[tokio::test]
async fn reopened_store_loads_the_latest_snapshot_and_events() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open store");

    let workspace_id = WorkspaceId::new();
    let task = Task::new(workspace_id, ys_agent_core::PrincipalId::new(), "Query GMV");
    let run = Run::new(task.id, WorkflowKind::Query);
    let initial = run.snapshot(serde_json::json!({}), None, None, None);
    let command_id = CommandId::new();
    let fingerprint = format!("seed:{command_id}");
    store
        .commit_command(RuntimeCommandBatch {
            command_id,
            command_fingerprint: fingerprint.clone(),
            receipt: CommandReceipt {
                command_id,
                command_fingerprint: fingerprint,
                result_kind: CommandResultKind::RunStarted,
                session_id: None,
                task_id: Some(task.id),
                run_id: Some(run.id),
                artifact_id: None,
                message: None,
                capability: None,
            },
            new_session: None,
            new_task: Some(task),
            create_run: Some(create_run(initial.clone())),
            new_artifact: None,
            pending_events: vec![],
            snapshot_update: None,
        })
        .await
        .expect("seed run");
    let waiting = advanced_snapshot(&initial, RunStatus::WaitingForInput);
    store
        .append(
            &initial.run_id,
            initial.version,
            vec![],
            vec![
                pending(RunEventKind::RunStarted),
                pending(RunEventKind::RunWaiting {
                    reason: "clarification".to_owned(),
                }),
            ],
            &waiting,
        )
        .await
        .expect("persist waiting run");
    drop(store);

    let reopened = SqliteRuntimeStore::open(&database).await.expect("reopen");
    let loaded = reopened.load_run(&initial.run_id).await.expect("load run");
    let events = reopened
        .load_events(&initial.run_id, 0)
        .await
        .expect("load events");

    assert_eq!(loaded.status, RunStatus::WaitingForInput);
    assert_eq!(loaded.version, 2);
    assert_eq!(events.len(), 5);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[4].sequence, 5);
    assert!(matches!(
        events[0].event.kind,
        RunEventKind::ProviderBound { .. }
    ));
    assert!(matches!(
        events[1].event.kind,
        RunEventKind::RunStateProjected { .. }
    ));
    assert!(matches!(
        events[4].event.kind,
        RunEventKind::RunStateProjected { .. }
    ));
}

#[tokio::test]
async fn artifact_bytes_are_addressed_by_hash_not_user_filename() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = LocalArtifactStore::new(directory.path()).expect("artifact store");
    let metadata = store
        .put(PutArtifact {
            workspace_id: WorkspaceId::new(),
            task_id: ys_agent_core::TaskId::new(),
            run_id: ys_agent_core::RunId::new(),
            kind: ArtifactKind::QueryResult,
            media_type: "application/json".to_owned(),
            bytes: b"secret rows".to_vec(),
            sensitivity: Sensitivity::Internal,
            owner: None,
            retention_policy: None,
            expires_at: None,
            producer_step_id: None,
        })
        .await
        .expect("write artifact");

    assert!(!metadata.storage_uri.contains("secret rows"));
    assert!(metadata.storage_uri.starts_with("artifact://sha256/"));
    assert_eq!(
        metadata.content_hash,
        "sha256:b4cfc0753e98c77a975a91f258c430e474af14e9a1853a47bc213aa3b4147269"
    );
}

#[test]
fn artifact_store_preserves_fresh_temporary_files_during_startup_cleanup() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let shard = directory.path().join("artifacts/ab");
    fs::create_dir_all(&shard).expect("create artifact shard");
    let temporary = shard.join(".ysda-tmp-in-progress");
    fs::write(&temporary, b"in progress").expect("write temporary artifact");

    let _store = LocalArtifactStore::new(directory.path()).expect("artifact store");

    assert!(
        temporary.exists(),
        "fresh temporary artifact must be retained"
    );
}

#[tokio::test]
async fn duplicate_command_id_returns_the_origianl_recepit() {
    let fixture = StoreFixture::new().await;
    let workspace_id = WorkspaceId::new();
    let task = Task::new(workspace_id, ys_agent_core::PrincipalId::new(), "Query GMV");
    let run = Run::new(task.id, WorkflowKind::Query);
    let snapshot = run.snapshot(serde_json::json!({}), None, None, None);
    let command_id = CommandId::new();
    let fingerprint = "same-command".to_owned();
    let batch = RuntimeCommandBatch {
        command_id,
        command_fingerprint: fingerprint.clone(),
        receipt: CommandReceipt {
            command_id,
            command_fingerprint: fingerprint,
            result_kind: CommandResultKind::RunStarted,
            session_id: None,
            task_id: Some(task.id),
            run_id: Some(run.id),
            artifact_id: None,
            message: None,
            capability: None,
        },
        new_session: None,
        new_task: Some(task),
        create_run: Some(create_run(snapshot)),
        new_artifact: None,
        pending_events: vec![],
        snapshot_update: None,
    };

    let first = fixture
        .store
        .commit_command(batch.clone())
        .await
        .expect("first command");
    let second = fixture
        .store
        .commit_command(batch)
        .await
        .expect("replayed command");

    assert_eq!(first, second);
    assert_eq!(fixture.store.run_count().await.expect("count runs"), 1);
}
