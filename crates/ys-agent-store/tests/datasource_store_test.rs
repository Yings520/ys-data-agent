use ys_agent_core::*;
use ys_agent_store::SqliteRuntimeStore;

#[path = "../../ys-agent-core/tests/support/datasource.rs"]
mod datasource_support;

fn save(scope: DatasourceScope, name: &str, expected_version: u64) -> DatasourceCommit {
    let profile_id = ProfileId::new();
    let revision = DatasourceRevision::new(DatasourceRevisionInput {
        schema_version: 1,
        workspace_id: scope.workspace_id,
        profile_id,
        revision: 1,
        adapter_id: "sqlite".try_into().unwrap(),
        adapter_version: "test".try_into().unwrap(),
        config_version: 1,
        source_id: None,
        fields: Default::default(),
        context: DatabaseContext::Unconfigured,
        credential: None,
    })
    .unwrap();
    let profile = DatasourceProfile {
        schema_version: 1,
        workspace_id: scope.workspace_id,
        profile_id,
        source_id: None,
        name: DatasourceName::new(name).unwrap(),
        head_revision: std::num::NonZeroU64::new(1).unwrap(),
        deleted_at: None,
    };
    let change = DatasourceChange::SaveRevision {
        profile,
        revision,
        mutation_id: None,
    };
    DatasourceCommit {
        schema_version: 1,
        write: DatasourceWriteContext {
            command_id: CommandId::new(),
            scope,
            expected_version,
            expected_head_revision: None,
        },
        command_digest: DatasourceDigest::of(&change).unwrap(),
        change,
    }
}

#[tokio::test]
async fn datasource_save_is_durable_idempotent_and_name_unique() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database).await.unwrap();
    let repository = store.datasource_repository();
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    assert!(repository.load(scope).await.unwrap().profiles.is_empty());
    let command = save(scope, "Analytics", 0);
    let receipt = repository.commit(command.clone()).await.unwrap();
    assert_eq!(receipt.committed_version, 1);
    assert_eq!(receipt.snapshot.profiles.len(), 1);
    assert_eq!(receipt.snapshot.profiles[0].state, RevisionState::Draft);
    assert!(receipt.snapshot.selection.current.is_none());
    assert_eq!(repository.commit(command).await.unwrap(), receipt);
    let conflict = repository
        .commit(save(scope, "ANALYTICS", 1))
        .await
        .unwrap_err();
    assert_eq!(conflict.code, DsErrorCode::DuplicateName);
    let reopened = SqliteRuntimeStore::open(database)
        .await
        .unwrap()
        .datasource_repository();
    assert_eq!(reopened.load(scope).await.unwrap(), receipt.snapshot);
}

#[tokio::test]
async fn stale_writers_and_failed_transactions_leave_the_winner_intact() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database).await.unwrap();
    let a = store.datasource_repository();
    let b = SqliteRuntimeStore::open(&database)
        .await
        .unwrap()
        .datasource_repository();
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let (first, second) =
        tokio::join!(a.commit(save(scope, "A", 0)), b.commit(save(scope, "B", 0)));
    assert_ne!(first.is_ok(), second.is_ok());
    let winner = first.or(second).unwrap();
    assert_eq!(a.load(scope).await.unwrap(), winner.snapshot);
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_datasource_receipt BEFORE INSERT ON datasource_command_receipts BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert_eq!(
        a.commit(save(scope, "C", 1)).await.unwrap_err().code,
        DsErrorCode::Storage
    );
    assert_eq!(b.load(scope).await.unwrap(), winner.snapshot);
}

#[tokio::test]
async fn secret_journal_reserves_generations_and_recovery_cannot_commit_removed_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("runtime.db");
    let repository = SqliteRuntimeStore::open(&database)
        .await
        .unwrap()
        .datasource_repository();
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let command = save(scope, "Protected", 0);
    let DatasourceChange::SaveRevision { profile, .. } = &command.change else {
        unreachable!()
    };
    let reference = DatasourceSecretRef::new(scope.workspace_id, profile.profile_id, 1).unwrap();
    let mut mutation = SecretMutation {
        schema_version: 1,
        mutation_id: OperationId::new(),
        write: command.write,
        profile_id: profile.profile_id,
        old: None,
        new: Some(reference),
        phase: SecretMutationPhase::Prepared,
        command_digest: command.command_digest.clone(),
    };
    let journal = |mutation: SecretMutation| DatasourceCommit {
        schema_version: 1,
        write: mutation.write,
        command_digest: mutation.command_digest.clone(),
        change: DatasourceChange::SecretJournal { mutation },
    };
    repository.commit(journal(mutation.clone())).await.unwrap();
    assert_eq!(repository.load(scope).await.unwrap().version, 0);
    assert!(
        repository
            .receipt(command.write.command_id)
            .await
            .unwrap()
            .is_none()
    );
    let mut rival = mutation.clone();
    rival.mutation_id = OperationId::new();
    rival.write.command_id = CommandId::new();
    assert_eq!(
        repository.commit(journal(rival)).await.unwrap_err().code,
        DsErrorCode::Conflict
    );
    let reopened = SqliteRuntimeStore::open(database)
        .await
        .unwrap()
        .datasource_repository();
    assert_eq!(
        reopened
            .pending_secret_mutations(scope.workspace_id)
            .await
            .unwrap(),
        vec![mutation.clone()]
    );
    reopened.claim_secret_cleanup(reference).await.unwrap();
    reopened.finish_secret_cleanup(reference).await.unwrap();
    mutation.phase = SecretMutationPhase::VaultWritten;
    assert_eq!(
        reopened
            .commit(journal(mutation.clone()))
            .await
            .unwrap_err()
            .code,
        DsErrorCode::Conflict
    );
    reopened
        .finish_secret_mutation(mutation.mutation_id)
        .await
        .unwrap();
    assert!(
        reopened
            .pending_secret_mutations(scope.workspace_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(reopened.load(scope).await.unwrap().profiles.is_empty());
}

#[tokio::test]
async fn unresolved_secret_journal_blocks_profile_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let repository = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .unwrap()
        .datasource_repository();
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let receipt = repository.commit(save(scope, "Pending", 0)).await.unwrap();
    let profile = &receipt.snapshot.profiles[0].profile;
    let write = DatasourceWriteContext {
        command_id: CommandId::new(),
        scope,
        expected_version: 1,
        expected_head_revision: Some(profile.head_revision),
    };
    let mutation = SecretMutation {
        schema_version: 1,
        mutation_id: OperationId::new(),
        write,
        profile_id: profile.profile_id,
        old: None,
        new: Some(DatasourceSecretRef::new(scope.workspace_id, profile.profile_id, 1).unwrap()),
        phase: SecretMutationPhase::Prepared,
        command_digest: DatasourceDigest::of(&"pending").unwrap(),
    };
    repository
        .commit(DatasourceCommit {
            schema_version: 1,
            write,
            command_digest: mutation.command_digest.clone(),
            change: DatasourceChange::SecretJournal { mutation },
        })
        .await
        .unwrap();
    let change = DatasourceChange::Delete {
        profile_id: profile.profile_id,
        disposition: DeleteDatasourceDisposition::ConfirmUnconfigured,
    };
    assert_eq!(
        repository
            .commit(DatasourceCommit {
                schema_version: 1,
                write: DatasourceWriteContext {
                    command_id: CommandId::new(),
                    ..write
                },
                command_digest: DatasourceDigest::of(&change).unwrap(),
                change
            })
            .await
            .unwrap_err()
            .code,
        DsErrorCode::Conflict
    );
    assert_eq!(repository.load(scope).await.unwrap(), receipt.snapshot);
}

async fn new_session(store: &SqliteRuntimeStore, workspace: WorkspaceId) -> Session {
    let session = Session::new(workspace, PrincipalId::new());
    let command_id = CommandId::new();
    let command_fingerprint = format!("session:{command_id}");
    store
        .commit_command(RuntimeCommandBatch {
            command_id,
            command_fingerprint: command_fingerprint.clone(),
            receipt: CommandReceipt {
                command_id,
                command_fingerprint,
                result_kind: CommandResultKind::SessionCreated,
                session_id: Some(session.id),
                task_id: None,
                run_id: None,
                artifact_id: None,
                message: None,
                capability: None,
            },
            new_session: Some(session.clone()),
            new_task: None,
            create_run: None,
            new_artifact: None,
            pending_events: vec![],
            snapshot_update: None,
        })
        .await
        .unwrap();
    session
}

#[tokio::test]
async fn workspace_default_initializes_only_new_sessions_and_delete_updates_all_references() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .unwrap();
    let repository = store.datasource_repository();
    let workspace = WorkspaceId::new();
    let earlier = new_session(&store, workspace).await;
    let binding = datasource_support::persisted_binding(&repository, RunId::new(), workspace).await;
    let scope = binding.scope();
    let change = DatasourceChange::Selection {
        revision: binding.revision(),
        kind: DatasourceSelectionKind::WorkspaceDefault,
    };
    let receipt = repository
        .commit(DatasourceCommit {
            schema_version: 1,
            write: DatasourceWriteContext {
                command_id: CommandId::new(),
                scope,
                expected_version: repository.load(scope).await.unwrap().version,
                expected_head_revision: Some(binding.revision().revision),
            },
            command_digest: DatasourceDigest::of(&change).unwrap(),
            change,
        })
        .await
        .unwrap();
    let later = new_session(&store, workspace).await;
    let earlier_scope = DatasourceScope {
        workspace_id: workspace,
        session_id: earlier.id,
    };
    let later_scope = DatasourceScope {
        workspace_id: workspace,
        session_id: later.id,
    };
    assert_eq!(
        repository
            .load(later_scope)
            .await
            .unwrap()
            .selection
            .current,
        Some(binding.revision())
    );
    assert_eq!(
        repository
            .load(earlier_scope)
            .await
            .unwrap()
            .selection
            .current,
        None
    );
    let change = DatasourceChange::Delete {
        profile_id: binding.revision().profile_id,
        disposition: DeleteDatasourceDisposition::ConfirmUnconfigured,
    };
    repository
        .commit(DatasourceCommit {
            schema_version: 1,
            write: DatasourceWriteContext {
                command_id: CommandId::new(),
                scope,
                expected_version: receipt.committed_version,
                expected_head_revision: Some(binding.revision().revision),
            },
            command_digest: DatasourceDigest::of(&change).unwrap(),
            change,
        })
        .await
        .unwrap();
    for session_scope in [scope, earlier_scope, later_scope] {
        let state = repository.load(session_scope).await.unwrap();
        assert!(state.selection.current.is_none());
        assert!(state.selection.workspace_default.is_none());
        assert!(state.selection.header.is_none());
    }
}

#[tokio::test]
async fn replacing_current_validation_retains_prior_evidence_by_validation_id() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database).await.unwrap();
    let repository = store.datasource_repository();
    let binding =
        datasource_support::persisted_binding(&repository, RunId::new(), WorkspaceId::new()).await;
    let evidence = ValidationEvidence::new(
        binding.evidence().inputs().clone(),
        "test".try_into().unwrap(),
        ProbeEvidence {
            authenticated: true,
            target_verified: true,
            read_only_verified: true,
            least_privilege_verified: true,
            capabilities_verified: true,
        },
        chrono::Utc::now(),
    )
    .unwrap();
    let change = DatasourceChange::Validation {
        revision: binding.revision(),
        state: RevisionState::Ready,
        evidence: Some(evidence.clone()),
    };
    repository
        .commit(DatasourceCommit {
            schema_version: 1,
            write: DatasourceWriteContext {
                command_id: CommandId::new(),
                scope: binding.scope(),
                expected_version: repository.load(binding.scope()).await.unwrap().version,
                expected_head_revision: Some(binding.revision().revision),
            },
            command_digest: DatasourceDigest::of(&change).unwrap(),
            change,
        })
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_revision(binding.revision())
            .await
            .unwrap()
            .validation,
        Some(evidence)
    );
    let connection = rusqlite::Connection::open(database).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM datasource_validations WHERE workspace_id=?1 AND profile_id=?2",
            [
                binding.scope().workspace_id.to_string(),
                binding.revision().profile_id.to_string(),
            ],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 2,
        "validation records are evidence, only the current pointer is replaced"
    );
}

#[tokio::test]
async fn incomplete_draft_can_assign_its_source_once_but_cannot_reassign_it() {
    let directory = tempfile::tempdir().unwrap();
    let repository = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .unwrap()
        .datasource_repository();
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let first = repository.commit(save(scope, "Draft", 0)).await.unwrap();
    let mut detail = first.snapshot.profiles[0].clone();
    for (number, source) in [(2, "authorized"), (3, "other")] {
        let mut input = detail.revision.input().clone();
        input.revision = number;
        input.source_id = Some(SourceId::new(source));
        let revision = DatasourceRevision::new(input).unwrap();
        let mut profile = detail.profile.clone();
        profile.head_revision = revision.identity().revision;
        profile.source_id = revision.input().source_id.clone();
        let change = DatasourceChange::SaveRevision {
            profile,
            revision,
            mutation_id: None,
        };
        let result = repository
            .commit(DatasourceCommit {
                schema_version: 1,
                write: DatasourceWriteContext {
                    command_id: CommandId::new(),
                    scope,
                    expected_version: number - 1,
                    expected_head_revision: Some(detail.profile.head_revision),
                },
                command_digest: DatasourceDigest::of(&change).unwrap(),
                change,
            })
            .await;
        if number == 2 {
            detail = result.unwrap().snapshot.profiles.remove(0);
        } else {
            assert_eq!(result.unwrap_err().code, DsErrorCode::InvalidField);
        }
    }
}
