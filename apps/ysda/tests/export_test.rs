use std::sync::Arc;

use ys_agent_core::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactKind, ArtifactStore, CommandId,
    CommandReceipt, CommandResultKind, ExportFormat, Principal, PutArtifact, RetentionPolicy,
    RuntimeCommandBatch, RuntimeStore, Sensitivity, TaskId, WorkspaceId,
};
use ys_agent_runtime::export::{
    ArtifactExportService, ArtifactExporter, DefaultExportPolicy, ExportDisposition, ExportPolicy,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};
use ysda::bootstrap::OwnerOnlyExportWriter;

#[test]
fn restricted_artifact_cannot_be_exported() {
    let policy = DefaultExportPolicy;
    let decision = policy.decide(Sensitivity::Restricted);

    assert_eq!(decision, ExportDisposition::Denied);
    assert_eq!(decision.error_code(), Some("artifact_export_denied"));
}

#[tokio::test]
async fn export_command_replay_returns_the_same_persisted_export_artifact() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let workspace_id = WorkspaceId::new();
    let principal = Principal::local_operator("export-test");
    let task_id = TaskId::new();
    let run_id = ys_agent_core::RunId::new();
    let store: Arc<dyn RuntimeStore> = Arc::new(
        SqliteRuntimeStore::open(directory.path().join("runtime.db"))
            .await
            .expect("runtime store"),
    );
    let artifacts: Arc<dyn ArtifactStore> = Arc::new(
        LocalArtifactStore::new(directory.path().join("artifacts")).expect("artifact store"),
    );
    let source = artifacts
        .put(PutArtifact {
            workspace_id,
            task_id,
            run_id,
            kind: ArtifactKind::Query,
            media_type: "application/json".to_owned(),
            bytes: br#"{"question":"GMV","answer_summary":"stable","time_range":null,"executed_sql":null,"warning_codes":[],"result_artifact":null}"#.to_vec(),
            sensitivity: Sensitivity::Internal,
            owner: Some(principal.id),
            retention_policy: None,
            expires_at: None,
            producer_step_id: None,
        })
        .await
        .expect("persist source artifact");
    let seed_command = CommandId::new();
    store
        .commit_command(RuntimeCommandBatch {
            command_id: seed_command,
            command_fingerprint: "seed-artifact".to_owned(),
            receipt: CommandReceipt {
                command_id: seed_command,
                command_fingerprint: "seed-artifact".to_owned(),
                result_kind: CommandResultKind::NoopReplay,
                session_id: None,
                task_id: Some(task_id),
                run_id: Some(run_id),
                artifact_id: None,
                message: None,
                capability: None,
            },
            new_session: None,
            new_task: None,
            new_run_snapshot: None,
            new_artifact: Some(source.clone()),
            pending_events: Vec::new(),
            snapshot_update: None,
            capability: None,
        })
        .await
        .expect("index source artifact");
    let exporter = ArtifactExporter::with_retention_days(
        store,
        artifacts,
        Arc::new(OwnerOnlyExportWriter::new(directory.path().join("exports"))),
        Arc::new(DefaultExportPolicy),
        19,
    );
    let command_id = CommandId::new();
    let access = ArtifactAccessContext {
        workspace_id,
        principal_id: principal.id,
        purpose: ArtifactAccessPurpose::Export,
        max_sensitivity: Sensitivity::Internal,
    };

    let first = exporter
        .export(command_id, &source.id, ExportFormat::Json, access.clone())
        .await
        .expect("first export");
    let replay = exporter
        .export(command_id, &source.id, ExportFormat::Json, access)
        .await
        .expect("replayed export");

    assert_eq!(first.id, replay.id);
    assert_eq!(first.storage_uri, replay.storage_uri);
    assert_eq!(
        first.retention_policy,
        Some(RetentionPolicy::Days { days: 19 })
    );
    let expires_at = first.expires_at.expect("configured export expiry");
    assert_eq!((expires_at - first.created_at).num_days(), 19);
    assert!(std::path::Path::new(&first.storage_uri).is_file());
}
