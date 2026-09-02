use rusqlite::Connection;
use tempfile::TempDir;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, ActiveProviderSnapshot, CommandId,
    CommandReceipt, CommandResultKind, CompatibilityEvidence, CreateRunCommand,
    CredentialGeneration, CredentialKind, CredentialViewStatus, PendingRunEvent, ProfileId,
    ProfileName, ProfileRevision, ProviderId, ProviderModelId, ProviderParameters,
    RevisionPrecondition, Run, RunEventKind, RunProviderBinding, RunProviderBindingRepository,
    RuntimeCommandBatch, RuntimeStore, SaveProfileRevision, Task, ValidationCommit,
    ValidationCommitPrecondition, ValidationVersions, WorkflowKind, WorkspaceId,
};
use ys_agent_store::SqliteRuntimeStore;

struct ActiveFixture {
    snapshot: ActiveProviderSnapshot,
    credential: CredentialGeneration,
}

async fn seed_active_profile(
    store: &SqliteRuntimeStore,
    database: &std::path::Path,
    name: &str,
    model: &str,
    expected_activation_revision: Option<u64>,
) -> ActiveFixture {
    let repository = store.provider_repository();
    let profile_id = ProfileId::new();
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: ProfileName::new(name).expect("valid profile name"),
            revision: ProfileRevision::draft(
                profile_id,
                1,
                ProviderId::DeepSeek,
                ProviderModelId::new(ProviderId::DeepSeek, model).expect("valid model"),
                ProviderParameters::default(),
                None,
            )
            .expect("initial draft"),
        })
        .await
        .expect("save initial Profile revision");

    let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("valid credential generation");
    Connection::open(database)
        .expect("open database")
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES (?1, 1, 'api_key', ?2, 'available', 'now', 'now')",
            [
                profile_id.to_string(),
                format!("io.ysda.test://{profile_id}:1"),
            ],
        )
        .expect("seed non-sensitive Credential metadata");

    let candidate = ProfileRevision::draft(
        profile_id,
        2,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, model).expect("valid model"),
        ProviderParameters::default(),
        Some(credential),
    )
    .expect("credential-backed revision");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(1),
            },
            name: ProfileName::new(name).expect("valid profile name"),
            revision: candidate.clone(),
        })
        .await
        .expect("save candidate revision");
    let versions = ValidationVersions::new("catalog-v1", "probe-v1", "liter-v1", "codec-v1");
    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(versions.clone()));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();
    repository
        .save_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: ys_agent_core::OperationId::new(),
                profile_id,
                revision: 2,
                credential_generation: credential,
                validation_digest: validation_digest.clone(),
            },
            evidence,
            versions,
        })
        .await
        .expect("save passing validation");
    let snapshot = repository
        .activate(ActivateProfileRequest {
            operation_id: ys_agent_core::OperationId::new(),
            precondition: ActivationPrecondition {
                profile_id,
                revision: 2,
                validation_id,
                validation_digest,
                expected_activation_revision,
            },
        })
        .await
        .expect("activate ready Profile");
    ActiveFixture {
        snapshot,
        credential,
    }
}

fn create_batch(task: Option<Task>, run: &Run, binding: RunProviderBinding) -> RuntimeCommandBatch {
    let snapshot = run.snapshot(serde_json::json!({"phase": "created"}), None, None, None);
    let command_id = CommandId::new();
    let command_fingerprint = format!("create-run:{command_id}");
    RuntimeCommandBatch {
        command_id,
        command_fingerprint: command_fingerprint.clone(),
        receipt: CommandReceipt {
            command_id,
            command_fingerprint,
            result_kind: CommandResultKind::RunStarted,
            session_id: None,
            task_id: Some(run.task_id),
            run_id: Some(run.id),
            artifact_id: None,
            message: None,
            capability: None,
        },
        new_session: None,
        new_task: task,
        create_run: Some(
            CreateRunCommand::new(
                snapshot,
                binding,
                vec![PendingRunEvent {
                    actor: ys_agent_core::EventActor::System,
                    kind: RunEventKind::RunStarted,
                }],
            )
            .expect("complete Run creation command"),
        ),
        new_artifact: None,
        pending_events: Vec::new(),
        snapshot_update: None,
    }
}

#[tokio::test]
async fn run_binding_is_atomic_insert_only_recoverable_and_tracks_nonterminal_references() {
    let directory = TempDir::new().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open store");
    let active = seed_active_profile(&store, &database, "Primary", "deepseek/model-a", None).await;
    let task = Task::new(
        WorkspaceId::new(),
        ys_agent_core::PrincipalId::new(),
        "Atomic Provider binding",
    );
    let mut run = Run::new(task.id, WorkflowKind::Query);
    let binding =
        RunProviderBinding::from_active(run.id, active.snapshot).expect("immutable binding");

    store
        .commit_command(create_batch(Some(task), &run, binding.clone()))
        .await
        .expect("atomically persist Run, binding, and events");

    let bindings = store.run_binding_repository();
    assert_eq!(
        bindings
            .load_run_binding(run.id)
            .await
            .expect("load exact binding"),
        binding
    );
    assert!(
        bindings
            .has_nonterminal_profile_references(binding.profile_id())
            .await
            .expect("query Profile references")
    );
    assert!(
        bindings
            .has_nonterminal_credential_references(active.credential)
            .await
            .expect("query Credential references")
    );
    assert_eq!(
        bindings
            .credential_status(active.credential)
            .await
            .expect("read durable credential status"),
        CredentialViewStatus::Saved
    );
    Connection::open(&database)
        .expect("open database for durable credential status update")
        .execute(
            "UPDATE provider_credential_generations
             SET status = 'revoked'
             WHERE profile_id = ?1 AND generation = ?2",
            [active.credential.profile_id().to_string(), "1".to_owned()],
        )
        .expect("mark the exact durable generation revoked");
    assert_eq!(
        bindings
            .credential_status(active.credential)
            .await
            .expect("read revoked durable credential status"),
        CredentialViewStatus::Revoked
    );
    let events = store
        .load_events(&run.id, 0)
        .await
        .expect("load initial lifecycle events");
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0].event.kind,
        RunEventKind::ProviderBound { .. }
    ));

    let connection = Connection::open(&database).expect("open database for immutability check");
    let update_error = connection
        .execute(
            "UPDATE run_provider_bindings SET model_id = 'deepseek/changed' WHERE run_id = ?1",
            [run.id.to_string()],
        )
        .expect_err("a persisted binding is insert-only");
    assert!(update_error.to_string().contains("insert-only"));
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enforce historical references");
    connection
        .execute(
            "DELETE FROM provider_profiles WHERE profile_id = ?1",
            [binding.profile_id().to_string()],
        )
        .expect_err("historical Run binding prevents destructive Profile history deletion");
    drop(connection);

    run.start().expect("start Run");
    run.succeed().expect("finish Run");
    let terminal = run.snapshot(serde_json::json!({"phase": "done"}), None, None, None);
    store
        .append(
            &run.id,
            1,
            Vec::new(),
            vec![PendingRunEvent {
                actor: ys_agent_core::EventActor::System,
                kind: RunEventKind::RunCompleted {
                    primary_artifact_id: ys_agent_core::ArtifactId::new(),
                },
            }],
            &terminal,
        )
        .await
        .expect("persist terminal Run state");
    assert!(
        !bindings
            .has_nonterminal_profile_references(binding.profile_id())
            .await
            .expect("terminal Run releases Profile retirement guard")
    );
    assert!(
        !bindings
            .has_nonterminal_credential_references(active.credential)
            .await
            .expect("terminal Run releases Credential retirement guard")
    );

    drop(bindings);
    drop(store);
    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen store");
    assert_eq!(
        reopened
            .run_binding_repository()
            .load_run_binding(run.id)
            .await
            .expect("recover immutable historical binding"),
        binding
    );
}

#[tokio::test]
async fn active_snapshot_race_rolls_back_run_binding_events_and_receipt() {
    let directory = TempDir::new().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open store");
    let original =
        seed_active_profile(&store, &database, "Original", "deepseek/model-a", None).await;
    let task = Task::new(
        WorkspaceId::new(),
        ys_agent_core::PrincipalId::new(),
        "Race active Provider",
    );
    let run = Run::new(task.id, WorkflowKind::Query);
    let stale_binding =
        RunProviderBinding::from_active(run.id, original.snapshot).expect("stale binding");
    let batch = create_batch(Some(task), &run, stale_binding);

    seed_active_profile(
        &store,
        &database,
        "Replacement",
        "deepseek/model-b",
        Some(1),
    )
    .await;
    store
        .commit_command(batch.clone())
        .await
        .expect_err("active snapshot changed before the transaction commits");

    let connection = Connection::open(&database).expect("inspect rolled-back transaction");
    for (table, key) in [
        ("runs", "run_id"),
        ("run_provider_bindings", "run_id"),
        ("run_events", "run_id"),
    ] {
        let count: i64 = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {key} = ?1"),
                [run.id.to_string()],
                |row| row.get(0),
            )
            .expect("count rolled-back rows");
        assert_eq!(count, 0, "{table} must remain invisible");
    }
    let receipt_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM command_receipts WHERE command_id = ?1",
            [batch.command_id.to_string()],
            |row| row.get(0),
        )
        .expect("count rolled-back receipt");
    assert_eq!(receipt_count, 0);
}

#[tokio::test]
async fn failure_after_binding_insert_rolls_back_every_run_creation_row() {
    let directory = TempDir::new().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open store");
    let active = seed_active_profile(&store, &database, "Primary", "deepseek/model-a", None).await;
    let missing_task = Task::new(
        WorkspaceId::new(),
        ys_agent_core::PrincipalId::new(),
        "Missing atomic Task",
    );
    let run = Run::new(missing_task.id, WorkflowKind::Query);
    let binding =
        RunProviderBinding::from_active(run.id, active.snapshot).expect("immutable binding");

    store
        .commit_command(create_batch(None, &run, binding))
        .await
        .expect_err("event construction cannot load the absent Task");

    let connection = Connection::open(&database).expect("inspect rollback");
    let run_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE run_id = ?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .expect("count Runs");
    let binding_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM run_provider_bindings WHERE run_id = ?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .expect("count bindings");
    assert_eq!((run_count, binding_count), (0, 0));
}
