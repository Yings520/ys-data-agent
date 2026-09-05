#[path = "../../ys-agent-core/tests/support/datasource.rs"]
mod datasource_support;

use std::{fs, path::Path};

use rusqlite::Connection;
use tempfile::TempDir;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, ArtifactKind, ArtifactStore, CommandId,
    CommandReceipt, CommandResultKind, CompatibilityEvidence, CoreError, CreateRunCommand,
    CredentialGeneration, CredentialKind, CredentialMutationIntent, CredentialMutationPhase,
    CredentialPointerCommit, OperationId, PendingRunEvent, ProfileId, ProfileName, ProfileRevision,
    ProfileState, ProviderErrorCode, ProviderId, ProviderModelId, ProviderParameters, PutArtifact,
    RevisionPrecondition, Run, RunEventKind, RunProviderBinding, RunSnapshot, RunStatus,
    RuntimeCommandBatch, RuntimeStore, SaveProfileRevision, Sensitivity, Task, ValidationCommit,
    ValidationCommitPrecondition, ValidationVersions, WorkflowKind, WorkspaceId,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

const RUNTIME_MIGRATION: &str = include_str!("../migrations/0001_runtime.sql");
const PROVIDER_MIGRATION_V2: &str = include_str!("../migrations/0002_provider_management.sql");
const CREDENTIAL_JOURNAL_MIGRATION_V3: &str =
    include_str!("../migrations/0003_credential_journal_recovery.sql");
const RUN_BINDING_MIGRATION_V4: &str =
    include_str!("../migrations/0004_run_binding_activation_revision.sql");

#[tokio::test]
async fn provider_repository_keeps_active_ready_revision_when_a_new_draft_is_saved() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open migrated runtime store");
    let repository = store.provider_repository();
    let profile_id = ProfileId::new();
    let profile_name = ProfileName::new("Primary").expect("valid profile name");

    let initial = ProfileRevision::draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        ProviderParameters::default(),
        None,
    )
    .expect("initial draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: profile_name.clone(),
            revision: initial,
        })
        .await
        .expect("save initial draft");

    let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("credential generation");
    let connection =
        Connection::open(&database).expect("open database to seed credential metadata");
    connection
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES (?1, ?2, 'api_key', 'vault://opaque-locator', 'available', 'now', 'now')",
            [profile_id.to_string(), credential.number().to_string()],
        )
        .expect("seed credential metadata owned by the profile");

    let candidate = ProfileRevision::draft(
        profile_id,
        2,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        ProviderParameters::default(),
        Some(credential),
    )
    .expect("credential-backed draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(1),
            },
            name: profile_name.clone(),
            revision: candidate.clone(),
        })
        .await
        .expect("save candidate revision");

    let versions = ValidationVersions::new("catalog-v1", "probe-v1", "liter-v1", "codec-v1");
    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(versions.clone()));
    let validation_digest = evidence.digest();
    let validation_id = evidence.id();
    repository
        .save_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id,
                revision: 2,
                credential_generation: credential,
                validation_digest: validation_digest.clone(),
            },
            evidence,
            versions,
        })
        .await
        .expect("commit matching passing validation");
    let active = repository
        .activate(ActivateProfileRequest {
            operation_id: OperationId::new(),
            precondition: ActivationPrecondition {
                profile_id,
                revision: 2,
                validation_id,
                validation_digest,
                expected_activation_revision: None,
            },
        })
        .await
        .expect("activate ready revision");

    let edited = ProfileRevision::draft(
        profile_id,
        3,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-b").expect("valid model"),
        ProviderParameters::default(),
        Some(credential),
    )
    .expect("edited draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(2),
            },
            name: profile_name,
            revision: edited,
        })
        .await
        .expect("save newer draft without moving active profile");

    assert_eq!(active.profile_revision(), 2);
    assert_eq!(
        repository
            .active()
            .await
            .expect("read active snapshot")
            .expect("active snapshot")
            .profile_revision(),
        2
    );
    let summary = repository
        .list_profiles()
        .await
        .expect("list profile summaries")
        .pop()
        .expect("one profile");
    assert_eq!(summary.state, ProfileState::Draft);
    assert!(summary.is_active);

    let replacement_generation = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey)
        .expect("replacement generation");
    let mutation_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::replace(
                mutation_id,
                profile_id,
                3,
                credential,
                replacement_generation,
            )
            .expect("replacement intent"),
        )
        .await
        .expect("persist replacement intent");
    repository
        .record_credential_vault_write(mutation_id)
        .await
        .expect("record protected replacement generation");
    let replacement = ProfileRevision::draft(
        profile_id,
        4,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-b").expect("valid model"),
        ProviderParameters::default(),
        Some(replacement_generation),
    )
    .expect("replacement draft");
    repository
        .commit_credential_pointer(
            CredentialPointerCommit::new(mutation_id, profile_id, 3, replacement)
                .expect("valid replacement pointer"),
        )
        .await
        .expect("commit new credential revision");
    repository
        .complete_credential_mutation(mutation_id)
        .await
        .expect("complete retained-generation check");

    assert_eq!(
        repository
            .active()
            .await
            .expect("read active after credential replacement")
            .expect("active snapshot remains")
            .profile_revision(),
        2
    );
    let old_status: String = Connection::open(&database)
        .expect("open database to inspect retirement status")
        .query_row(
            "SELECT status FROM provider_credential_generations
             WHERE profile_id = ?1 AND generation = ?2",
            [profile_id.to_string(), credential.number().to_string()],
            |row| row.get(0),
        )
        .expect("old generation metadata");
    assert_eq!(old_status, "retained");

    let blocked_generation = CredentialGeneration::new(profile_id, 3, CredentialKind::ApiKey)
        .expect("blocked generation");
    let blocked_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::replace(
                blocked_id,
                profile_id,
                4,
                replacement_generation,
                blocked_generation,
            )
            .expect("blocked intent"),
        )
        .await
        .expect("start mutation before protection becomes uncertain");
    repository
        .record_credential_vault_write(blocked_id)
        .await
        .expect("record staged generation");
    repository
        .block_credential_mutation(
            blocked_id,
            ProviderErrorCode::CredentialProtectionUnavailable,
        )
        .await
        .expect("fail closed");
    assert!(
        repository
            .active()
            .await
            .expect("read active after fail-closed transition")
            .is_none(),
        "an uncertain credential state removes its active snapshot"
    );
}

#[tokio::test]
async fn credential_journal_recovers_all_failure_boundaries() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open migrated runtime store");
    let repository = store.provider_repository();
    let profile_id = ProfileId::new();
    let initial = ProfileRevision::draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        ProviderParameters::default(),
        None,
    )
    .expect("initial draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: ProfileName::new("Primary").expect("valid profile name"),
            revision: initial,
        })
        .await
        .expect("save initial profile");

    let first_generation =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");
    let create_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::create(create_id, profile_id, 1, first_generation)
                .expect("create intent"),
        )
        .await
        .expect("persist intent before Vault write");

    drop(repository);
    drop(store);
    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen after intent crash point");
    let repository = reopened.provider_repository();
    let pending = repository
        .pending_credential_mutations()
        .await
        .expect("restore pending journal");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id(), create_id);
    assert_eq!(pending[0].phase(), CredentialMutationPhase::IntentRecorded);
    repository
        .rollback_credential_mutation(create_id)
        .await
        .expect("a failed Vault write restores the prior logical state");
    assert_eq!(
        repository
            .load_revision(profile_id, 1)
            .await
            .expect("initial revision remains current")
            .credential_generation(),
        None
    );

    let retry_create_id = OperationId::new();
    let committed_generation = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey)
        .expect("generation numbers are never reused after rollback");
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::create(retry_create_id, profile_id, 1, committed_generation)
                .expect("retry create intent"),
        )
        .await
        .expect("retry after the failed Vault write");
    repository
        .record_credential_vault_write(retry_create_id)
        .await
        .expect("record protected Vault generation");
    drop(repository);
    drop(reopened);
    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen after Vault-written crash point");
    let repository = reopened.provider_repository();
    assert_eq!(
        repository
            .pending_credential_mutations()
            .await
            .expect("restore Vault-written journal")[0]
            .phase(),
        CredentialMutationPhase::VaultWritten
    );

    let credential_revision = ProfileRevision::draft(
        profile_id,
        2,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        ProviderParameters::default(),
        Some(committed_generation),
    )
    .expect("credential-backed revision");
    let committed = repository
        .commit_credential_pointer(
            CredentialPointerCommit::new(retry_create_id, profile_id, 1, credential_revision)
                .expect("valid pointer commit"),
        )
        .await
        .expect("atomically append revision and commit pointer");
    assert_eq!(committed.phase(), CredentialMutationPhase::PointerCommitted);
    drop(repository);
    drop(reopened);
    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen after pointer-committed crash point");
    let repository = reopened.provider_repository();
    assert_eq!(
        repository
            .pending_credential_mutations()
            .await
            .expect("restore pointer-committed journal")[0]
            .phase(),
        CredentialMutationPhase::PointerCommitted
    );
    repository
        .complete_credential_mutation(retry_create_id)
        .await
        .expect("complete successful creation");
    assert!(
        repository
            .pending_credential_mutations()
            .await
            .expect("no pending successful mutation")
            .is_empty()
    );

    let second_generation = CredentialGeneration::new(profile_id, 3, CredentialKind::ApiKey)
        .expect("second generation");
    let replace_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::replace(
                replace_id,
                profile_id,
                2,
                committed_generation,
                second_generation,
            )
            .expect("replace intent"),
        )
        .await
        .expect("persist replacement intent");
    repository
        .record_credential_vault_write(replace_id)
        .await
        .expect("record staged replacement");

    let concurrent_edit = ProfileRevision::draft(
        profile_id,
        3,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-b").expect("valid model"),
        ProviderParameters::default(),
        Some(committed_generation),
    )
    .expect("concurrent profile edit");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(2),
            },
            name: ProfileName::new("Primary").expect("valid profile name"),
            revision: concurrent_edit,
        })
        .await
        .expect("advance Profile before the late pointer commit");
    let late_revision = ProfileRevision::draft(
        profile_id,
        3,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        ProviderParameters::default(),
        Some(second_generation),
    )
    .expect("late replacement revision");
    let error = repository
        .commit_credential_pointer(
            CredentialPointerCommit::new(replace_id, profile_id, 2, late_revision)
                .expect("structurally valid late commit"),
        )
        .await
        .expect_err("a stale SQLite compare-and-swap cannot move the visible pointer");
    assert_eq!(error.code(), "provider.storage.conflict");
    drop(repository);
    drop(reopened);
    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen after cleanup-pending crash point");
    let repository = reopened.provider_repository();
    assert_eq!(
        repository
            .pending_credential_mutations()
            .await
            .expect("load cleanup state")[0]
            .phase(),
        CredentialMutationPhase::CleanupPending
    );
    assert_eq!(
        repository
            .load_revision(profile_id, 3)
            .await
            .expect("current revision remains the concurrent edit")
            .model()
            .as_str(),
        "deepseek/model-b"
    );
    repository
        .rollback_credential_mutation(replace_id)
        .await
        .expect("record staged generation cleanup");

    let third_generation =
        CredentialGeneration::new(profile_id, 4, CredentialKind::ApiKey).expect("third generation");
    let blocked_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::replace(
                blocked_id,
                profile_id,
                3,
                committed_generation,
                third_generation,
            )
            .expect("blocked replacement intent"),
        )
        .await
        .expect("start another replacement");
    repository
        .record_credential_vault_write(blocked_id)
        .await
        .expect("stage another generation");
    let later_edit = ProfileRevision::draft(
        profile_id,
        4,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-c").expect("valid model"),
        ProviderParameters::default(),
        Some(committed_generation),
    )
    .expect("later concurrent edit");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(3),
            },
            name: ProfileName::new("Primary").expect("valid profile name"),
            revision: later_edit,
        })
        .await
        .expect("advance Profile before another late pointer commit");
    let blocked_revision = ProfileRevision::draft(
        profile_id,
        4,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-b").expect("valid model"),
        ProviderParameters::default(),
        Some(third_generation),
    )
    .expect("stale blocked revision");
    repository
        .commit_credential_pointer(
            CredentialPointerCommit::new(blocked_id, profile_id, 3, blocked_revision)
                .expect("structurally valid blocked commit"),
        )
        .await
        .expect_err("the second stale commit also enters cleanup pending");
    let blocked = repository
        .block_credential_mutation(
            blocked_id,
            ProviderErrorCode::CredentialProtectionUnavailable,
        )
        .await
        .expect("persist stable fail-closed state");
    assert!(blocked.blocks_profile_use());
    drop(repository);
    drop(reopened);
    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen after blocked recovery state");
    let repository = reopened.provider_repository();
    let retry = repository
        .begin_credential_mutation(
            CredentialMutationIntent::replace(
                OperationId::new(),
                profile_id,
                4,
                committed_generation,
                CredentialGeneration::new(profile_id, 5, CredentialKind::ApiKey)
                    .expect("fourth generation"),
            )
            .expect("retry intent"),
        )
        .await
        .expect_err("a blocked Profile rejects new credential calls");
    assert_eq!(retry.code(), "provider.credential.protection_unavailable");
}

#[tokio::test]
async fn credential_delete_commits_a_credential_free_revision_and_cleans_rollback_generation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open migrated runtime store");
    let repository = store.provider_repository();
    let profile_id = ProfileId::new();
    let model =
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: ProfileName::new("Primary").expect("valid profile name"),
            revision: ProfileRevision::draft(
                profile_id,
                1,
                ProviderId::DeepSeek,
                model.clone(),
                ProviderParameters::default(),
                None,
            )
            .expect("initial draft"),
        })
        .await
        .expect("save initial profile");

    let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("credential generation");
    let create_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::create(create_id, profile_id, 1, credential)
                .expect("create intent"),
        )
        .await
        .expect("begin credential creation");
    repository
        .record_credential_vault_write(create_id)
        .await
        .expect("record created Vault generation");
    repository
        .commit_credential_pointer(
            CredentialPointerCommit::new(
                create_id,
                profile_id,
                1,
                ProfileRevision::draft(
                    profile_id,
                    2,
                    ProviderId::DeepSeek,
                    model.clone(),
                    ProviderParameters::default(),
                    Some(credential),
                )
                .expect("credential revision"),
            )
            .expect("create pointer"),
        )
        .await
        .expect("commit credential creation");
    repository
        .complete_credential_mutation(create_id)
        .await
        .expect("complete credential creation");

    let rollback = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey)
        .expect("protected rollback generation");
    let delete_id = OperationId::new();
    repository
        .begin_credential_mutation(
            CredentialMutationIntent::delete(delete_id, profile_id, 2, credential, rollback)
                .expect("delete intent"),
        )
        .await
        .expect("begin credential deletion");
    repository
        .record_credential_vault_write(delete_id)
        .await
        .expect("record protected rollback copy");
    repository
        .commit_credential_pointer(
            CredentialPointerCommit::new(
                delete_id,
                profile_id,
                2,
                ProfileRevision::draft(
                    profile_id,
                    3,
                    ProviderId::DeepSeek,
                    model,
                    ProviderParameters::default(),
                    None,
                )
                .expect("credential-free revision"),
            )
            .expect("delete pointer"),
        )
        .await
        .expect("commit credential-free revision");
    repository
        .complete_credential_mutation(delete_id)
        .await
        .expect("complete rollback cleanup");

    assert_eq!(
        repository
            .load_revision(profile_id, 3)
            .await
            .expect("load credential-free revision")
            .credential_generation(),
        None
    );
    let connection = Connection::open(&database).expect("inspect generation metadata");
    let statuses: (String, String) = connection
        .query_row(
            "SELECT
                MAX(CASE WHEN generation = 1 THEN status END),
                MAX(CASE WHEN generation = 2 THEN status END)
             FROM provider_credential_generations WHERE profile_id = ?1",
            [profile_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("generation statuses");
    assert_eq!(statuses, ("retained".to_owned(), "deleted".to_owned()));
}

fn legacy_runtime_database(path: &Path) {
    let connection = Connection::open(path).expect("open legacy database");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )
        .expect("create migration ledger");
    connection
        .execute_batch(RUNTIME_MIGRATION)
        .expect("apply runtime migration");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 'legacy')",
            [],
        )
        .expect("record runtime migration");
}

#[tokio::test]
async fn credential_journal_migration_blocks_unverifiable_pre_contract_recovery_state() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let mut connection = Connection::open(&database).expect("open v2 database");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )
        .expect("create migration ledger");
    connection
        .execute_batch(RUNTIME_MIGRATION)
        .expect("apply runtime migration");
    connection
        .execute_batch(PROVIDER_MIGRATION_V2)
        .expect("apply pre-contract Provider migration");
    connection
        .execute_batch(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 'legacy');
             INSERT INTO schema_migrations(version, applied_at) VALUES (2, 'legacy');",
        )
        .expect("record legacy migrations");

    let profile_id = ProfileId::new();
    let mutation_id = OperationId::new();
    let transaction = connection.transaction().expect("legacy seed transaction");
    transaction
        .execute(
            "INSERT INTO provider_profiles(
                profile_id, name, current_revision, created_at, updated_at
             ) VALUES (?1, 'Primary', 1, 'legacy', 'legacy')",
            [profile_id.to_string()],
        )
        .expect("legacy profile");
    transaction
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES
                (?1, 1, 'api_key', 'io.ysda.local-credential://legacy:1', 'available', 'legacy', 'legacy'),
                (?1, 2, 'api_key', 'io.ysda.local-credential://legacy:2', 'available', 'legacy', 'legacy')",
            [profile_id.to_string()],
        )
        .expect("legacy generations");
    transaction
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json,
                credential_generation, state, validation_id, created_at
             ) VALUES (
                ?1, 1, 'deep_seek', 'deepseek/model-a',
                '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0,\"provider_specific\":{}}',
                1, 'draft', NULL, 'legacy'
             )",
            [profile_id.to_string()],
        )
        .expect("legacy revision");
    transaction
        .execute(
            "INSERT INTO credential_mutations(
                mutation_id, profile_id, old_generation, new_generation, rollback_generation,
                operation, phase, error_code, created_at, updated_at
             ) VALUES (?1, ?2, 1, 2, NULL, 'replace', 'vault_written', NULL, 'legacy', 'legacy')",
            [mutation_id.to_string(), profile_id.to_string()],
        )
        .expect("legacy pending mutation");
    transaction.commit().expect("commit legacy state");
    drop(connection);

    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("upgrade journal recovery contract");
    let pending = store
        .provider_repository()
        .pending_credential_mutations()
        .await
        .expect("load upgraded recovery record");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id(), mutation_id);
    assert_eq!(pending[0].expected_revision(), 1);
    assert_eq!(pending[0].phase(), CredentialMutationPhase::Blocked);
    assert_eq!(
        pending[0].error_code(),
        Some(ProviderErrorCode::StorageConflict)
    );
}

#[tokio::test]
async fn provider_migration_upgrades_legacy_database_and_leaves_no_active_profile() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    legacy_runtime_database(&database);

    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("upgrade legacy runtime database");
    drop(store);

    let connection = Connection::open(&database).expect("inspect upgraded database");
    let version: i64 = connection
        .query_row(
            "SELECT version FROM schema_migrations WHERE version = 2",
            [],
            |row| row.get(0),
        )
        .expect("provider migration version");
    assert_eq!(version, 2);

    for table in [
        "provider_profiles",
        "provider_profile_revisions",
        "provider_credential_generations",
        "provider_validations",
        "active_provider",
        "credential_mutations",
        "run_provider_bindings",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .expect("query provider table");
        assert!(exists, "migration must create {table}");
    }

    let active_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM active_provider", [], |row| row.get(0))
        .expect("query empty active singleton");
    assert_eq!(
        active_count, 0,
        "a fresh installation has no active Provider"
    );
}

#[tokio::test]
async fn invalid_validation_migration_upgrades_existing_provider_schema_without_losing_contracts() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let connection = Connection::open(&database).expect("open pre-upgrade database");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )
        .expect("create migration ledger");
    connection
        .execute_batch(RUNTIME_MIGRATION)
        .expect("apply runtime migration");
    connection
        .execute_batch(PROVIDER_MIGRATION_V2)
        .expect("apply Provider migration");
    connection
        .execute_batch(CREDENTIAL_JOURNAL_MIGRATION_V3)
        .expect("apply Credential journal migration");
    connection
        .execute_batch(RUN_BINDING_MIGRATION_V4)
        .expect("apply Run binding migration");
    connection
        .execute_batch(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 'legacy');
             INSERT INTO schema_migrations(version, applied_at) VALUES (2, 'legacy');
             INSERT INTO schema_migrations(version, applied_at) VALUES (3, 'legacy');
             INSERT INTO schema_migrations(version, applied_at) VALUES (4, 'legacy');",
        )
        .expect("record pre-validation schema");
    drop(connection);

    SqliteRuntimeStore::open(&database)
        .await
        .expect("upgrade failed-validation persistence contract");
    let connection = Connection::open(&database).expect("inspect upgraded schema");
    let version: i64 = connection
        .query_row(
            "SELECT version FROM schema_migrations WHERE version = 5",
            [],
            |row| row.get(0),
        )
        .expect("validation migration recorded");
    assert_eq!(version, 5);
    let schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'table' AND name = 'provider_profile_revisions'",
            [],
            |row| row.get(0),
        )
        .expect("load rebuilt revision schema");
    assert!(schema.contains("CHECK ((state = 'draft') = (validation_id IS NULL))"));
}

#[tokio::test]
async fn provider_migration_is_idempotent_and_rejects_unknown_parameter_schema() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("apply migrations");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("repeat migrations");

    let mut connection = Connection::open(&database).expect("inspect migrated database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    let transaction = connection.transaction().expect("start test transaction");
    transaction
        .execute_batch("PRAGMA defer_foreign_keys = ON")
        .expect("defer profile revision relationship");
    transaction
        .execute(
            "INSERT INTO provider_profiles(profile_id, name, current_revision, created_at, updated_at)
             VALUES ('profile-1', 'Primary', 1, 'now', 'now')",
            [],
        )
        .expect("insert profile identity");
    transaction
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES ('profile-1', 1, 'api_key', 'vault://opaque-locator', 'available', 'now', 'now')",
            [],
        )
        .expect("insert non-sensitive credential metadata");
    let error = transaction
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model',
                       '{\"schema_version\":999}', 1, 'draft', NULL, 'now')",
            [],
        )
        .expect_err("unknown parameter schema must fail closed");
    assert!(error.to_string().contains("CHECK"));
}

#[tokio::test]
async fn provider_migration_rejects_secret_json_and_revision_overwrites() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("apply migrations");

    let connection = Connection::open(&database).expect("inspect migrated database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    connection
        .execute_batch("PRAGMA defer_foreign_keys = ON; BEGIN")
        .expect("start deferred fixture transaction");
    connection
        .execute(
            "INSERT INTO provider_profiles(profile_id, name, current_revision, created_at, updated_at)
             VALUES ('profile-1', 'Primary', 1, 'now', 'now')",
            [],
        )
        .expect("insert profile identity");
    connection
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES ('profile-1', 1, 'api_key', 'vault://opaque-locator', 'available', 'now', 'now')",
            [],
        )
        .expect("insert non-sensitive credential metadata");
    let secret_error = connection
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model',
                       '{\"schema_version\":1,\"api_key\":\"canary-secret\"}', 1,
                       'draft', NULL, 'now')",
            [],
        )
        .expect_err("secret-shaped JSON must be rejected");
    assert!(secret_error.to_string().contains("non-sensitive schema"));
    let typed_secret_error = connection
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model',
                       '{\"schema_version\":1,\"temperature\":\"canary-secret\"}', 1,
                       'draft', NULL, 'now')",
            [],
        )
        .expect_err("string values are not valid Provider parameters");
    assert!(
        typed_secret_error
            .to_string()
            .contains("non-sensitive schema")
    );
    connection
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model',
                       '{\"schema_version\":1}', 1, 'draft', NULL, 'now')",
            [],
        )
        .expect("insert valid draft revision");
    connection.execute_batch("COMMIT").expect("commit fixture");

    let overwrite_error = connection
        .execute(
            "UPDATE provider_profile_revisions SET model_id = 'xai/overwritten'
             WHERE profile_id = 'profile-1' AND revision = 1",
            [],
        )
        .expect_err("revision configuration must be insert-only");
    assert!(overwrite_error.to_string().contains("immutable"));
    let revision_delete_error = connection
        .execute(
            "DELETE FROM provider_profile_revisions WHERE profile_id = 'profile-1' AND revision = 1",
            [],
        )
        .expect_err("revision history must be retained");
    assert!(
        revision_delete_error
            .to_string()
            .contains("revisions are insert-only")
    );
    let credential_delete_error = connection
        .execute(
            "DELETE FROM provider_credential_generations WHERE profile_id = 'profile-1' AND generation = 1",
            [],
        )
        .expect_err("credential generation metadata must be retained");
    assert!(
        credential_delete_error
            .to_string()
            .contains("credential generations are insert-only")
    );
}

#[tokio::test]
async fn provider_migration_rejects_ready_revision_without_matching_validation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("apply migrations");

    let mut connection = Connection::open(&database).expect("inspect migrated database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    let transaction = connection
        .transaction()
        .expect("start deferred transaction");
    transaction
        .execute_batch("PRAGMA defer_foreign_keys = ON")
        .expect("defer cyclic foreign keys");
    transaction
        .execute(
            "INSERT INTO provider_profiles(profile_id, name, current_revision, created_at, updated_at)
             VALUES ('profile-1', 'Primary', 1, 'now', 'now')",
            [],
        )
        .expect("insert profile identity");
    transaction
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES ('profile-1', 1, 'api_key', 'vault://opaque-locator', 'available', 'now', 'now')",
            [],
        )
        .expect("insert non-sensitive credential metadata");
    let capability_error = transaction
        .execute(
            "INSERT INTO provider_validations(
                validation_id, profile_id, revision, credential_generation, validation_digest,
                tool_calls_supported, non_empty_tool_call_ids, multi_turn_tool_results,
                context_limit, outcome, error_code, evidence_schema_version, checked_at
             ) VALUES ('incomplete-validation', 'profile-1', 1, 1, 'incomplete-digest',
                       0, 0, 0, 1, 'passed', NULL, 1, 'now')",
            [],
        )
        .expect_err("passing validation requires every required capability");
    assert!(capability_error.to_string().contains("CHECK"));
    let error = transaction
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model',
                       '{\"schema_version\":1}', 1, 'ready', 'missing-validation', 'now')",
            [],
        )
        .expect_err("Ready revisions require a matching passing validation");
    assert!(error.to_string().contains("matching passing validation"));
}

#[tokio::test]
async fn provider_migration_rejects_ready_revision_with_failed_validation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("apply migrations");

    let connection = Connection::open(&database).expect("inspect migrated database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    connection
        .execute_batch("PRAGMA defer_foreign_keys = ON; BEGIN")
        .expect("start deferred fixture transaction");
    connection
        .execute_batch(
            "INSERT INTO provider_profiles(profile_id, name, current_revision, created_at, updated_at)
             VALUES ('profile-1', 'Primary', 1, 'now', 'now');
             INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES ('profile-1', 1, 'api_key', 'vault://opaque-locator', 'available', 'now', 'now');
             INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1}', 1,
                       'draft', NULL, 'now');
             INSERT INTO provider_validations(
                validation_id, profile_id, revision, credential_generation, validation_digest,
                tool_calls_supported, non_empty_tool_call_ids, multi_turn_tool_results,
                context_limit, outcome, error_code, evidence_schema_version, checked_at
             ) VALUES ('validation-1', 'profile-1', 1, 1, 'digest-1', 0, 0, 0, 1,
                       'failed', 'provider.model.incompatible', 1, 'now');",
        )
        .expect("seed failed validation");
    let error = connection
        .execute(
            "UPDATE provider_profile_revisions
             SET state = 'ready', validation_id = 'validation-1'
             WHERE profile_id = 'profile-1' AND revision = 1",
            [],
        )
        .expect_err("failed validation must not make a revision Ready");
    assert!(error.to_string().contains("matching passing validation"));
    connection
        .execute_batch("ROLLBACK")
        .expect("discard fixture");
}

#[tokio::test]
async fn provider_migration_rejects_ready_revision_without_credential() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("apply migrations");

    let connection = Connection::open(&database).expect("inspect migrated database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    connection
        .execute_batch("PRAGMA defer_foreign_keys = ON; BEGIN")
        .expect("start deferred fixture transaction");
    connection
        .execute_batch(
            "INSERT INTO provider_profiles(profile_id, name, current_revision, created_at, updated_at)
             VALUES ('profile-1', 'Primary', 1, 'now', 'now');
             INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES ('profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1}',
                       NULL, 'draft', NULL, 'now');
             INSERT INTO provider_validations(
                validation_id, profile_id, revision, credential_generation, validation_digest,
                tool_calls_supported, non_empty_tool_call_ids, multi_turn_tool_results,
                context_limit, outcome, error_code, evidence_schema_version, checked_at
             ) VALUES ('validation-1', 'profile-1', 1, NULL, 'digest-1', 1, 1, 1, 1,
                       'passed', NULL, 1, 'now');",
        )
        .expect("seed credential-less draft and validation");
    let error = connection
        .execute(
            "UPDATE provider_profile_revisions
             SET state = 'ready', validation_id = 'validation-1'
             WHERE profile_id = 'profile-1' AND revision = 1",
            [],
        )
        .expect_err("a Ready revision requires a credential generation");
    assert!(error.to_string().contains("matching passing validation"));
    connection
        .execute_batch("ROLLBACK")
        .expect("discard fixture");
}

#[tokio::test]
async fn provider_migration_rejects_secret_fingerprints_and_cross_profile_validations() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    SqliteRuntimeStore::open(&database)
        .await
        .expect("apply migrations");

    let connection = Connection::open(&database).expect("inspect migrated database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    connection
        .execute_batch("PRAGMA defer_foreign_keys = ON; BEGIN")
        .expect("start deferred fixture transaction");
    connection
        .execute_batch(
            "INSERT INTO tasks(task_id, workspace_id, status, payload_json, created_at, updated_at)
             VALUES ('task-1', 'workspace-1', 'queued', '{}', 'now', 'now');
             INSERT INTO runs(run_id, task_id, status, version, snapshot_json, created_at, updated_at)
             VALUES ('run-1', 'task-1', 'queued', 1, '{}', 'now', 'now');
             INSERT INTO runs(run_id, task_id, status, version, snapshot_json, created_at, updated_at)
             VALUES ('run-2', 'task-1', 'queued', 1, '{}', 'now', 'now');
             INSERT INTO provider_profiles(profile_id, name, current_revision, created_at, updated_at)
             VALUES
                ('profile-1', 'Primary', 1, 'now', 'now'),
                ('profile-2', 'Secondary', 1, 'now', 'now');
             INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES
                ('profile-1', 1, 'api_key', 'vault://opaque-1', 'available', 'now', 'now'),
                ('profile-2', 1, 'api_key', 'vault://opaque-2', 'available', 'now', 'now');
             INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES
                ('profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}', 1,
                 'draft', NULL, 'now'),
                ('profile-2', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}', 1,
                 'draft', NULL, 'now');
             INSERT INTO provider_validations(
                validation_id, profile_id, revision, credential_generation, validation_digest,
                tool_calls_supported, non_empty_tool_call_ids, multi_turn_tool_results,
                context_limit, outcome, error_code, evidence_schema_version, checked_at
             ) VALUES
                ('validation-1', 'profile-1', 1, 1, 'digest-1', 1, 1, 1, 1, 'passed', NULL, 1, 'now'),
                ('validation-2', 'profile-2', 1, 1, 'digest-2', 1, 1, 1, 1, 'passed', NULL, 1, 'now');
             UPDATE provider_profile_revisions
             SET state = 'ready', validation_id = 'validation-2'
             WHERE profile_id = 'profile-2' AND revision = 1;",
        )
        .expect("seed ready revisions and matching validations");

    let draft_binding_error = connection
        .execute(
            "INSERT INTO run_provider_bindings(
                run_id, profile_id, revision, provider, model_id, parameters_json,
                credential_generation, validation_id, validation_digest,
                fingerprint_json, fingerprint_hash, created_at
             ) VALUES (
                'run-2', 'profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}',
                1, 'validation-1', 'digest-1',
                '{\"schema_version\":1,\"profile_id\":\"profile-1\",\"profile_revision\":1,\"provider\":\"deep_seek\",\"model\":{\"provider\":\"deep_seek\",\"value\":\"deepseek/model\"},\"parameters\":{\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}}',
                '0000000000000000000000000000000000000000000000000000000000000000', 'now'
             )",
            [],
        )
        .expect_err("a Draft revision must not be snapshotted into a Run binding");
    assert!(
        draft_binding_error
            .to_string()
            .contains("Run Provider binding")
    );
    connection
        .execute(
            "UPDATE provider_profile_revisions
             SET state = 'ready', validation_id = 'validation-1'
             WHERE profile_id = 'profile-1' AND revision = 1",
            [],
        )
        .expect("promote profile-1 only after passing validation");
    connection
        .execute(
            "INSERT INTO active_provider(
                singleton, profile_id, revision, validation_id, credential_generation,
                validation_digest, activation_revision, activated_at
             ) VALUES (1, 'profile-1', 1, 'validation-1', 1, 'digest-1', 1, 'now')",
            [],
        )
        .expect("activate the matching ready revision");

    let incomplete_fingerprint_error = connection
        .execute(
            "INSERT INTO run_provider_bindings(
                run_id, profile_id, revision, provider, model_id, parameters_json,
                credential_generation, validation_id, validation_digest,
                fingerprint_json, fingerprint_hash, created_at
             ) VALUES (
                'run-2', 'profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}',
                1, 'validation-1', 'digest-1',
                '{\"schema_version\":1,\"profile_id\":\"profile-1\",\"profile_revision\":1,\"provider\":\"deep_seek\",\"model\":{\"provider\":\"deep_seek\",\"value\":\"deepseek/model\"},\"parameters\":{\"timeout_seconds\":30}}',
                '0000000000000000000000000000000000000000000000000000000000000000', 'now'
             )",
            [],
        )
        .expect_err("fingerprints must include every canonical parameter");
    assert!(
        incomplete_fingerprint_error
            .to_string()
            .contains("non-sensitive schema")
    );
    let invalid_hash_error = connection
        .execute(
            "INSERT INTO run_provider_bindings(
                run_id, profile_id, revision, provider, model_id, parameters_json,
                credential_generation, validation_id, validation_digest,
                fingerprint_json, fingerprint_hash, created_at
             ) VALUES (
                'run-2', 'profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}',
                1, 'validation-1', 'digest-1',
                '{\"schema_version\":1,\"profile_id\":\"profile-1\",\"profile_revision\":1,\"provider\":\"deep_seek\",\"model\":{\"provider\":\"deep_seek\",\"value\":\"deepseek/model\"},\"parameters\":{\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}}',
                'not-a-sha256', 'now'
             )",
            [],
        )
        .expect_err("fingerprint hashes must use SHA-256 hexadecimal form");
    assert!(invalid_hash_error.to_string().contains("CHECK"));

    let fingerprint_error = connection
        .execute(
            "INSERT INTO run_provider_bindings(
                run_id, profile_id, revision, provider, model_id, parameters_json,
                credential_generation, validation_id, validation_digest,
                fingerprint_json, fingerprint_hash, created_at
             ) VALUES (
                'run-1', 'profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}',
                1, 'validation-1', 'digest-1',
                '{\"schema_version\":1,\"profile_id\":\"profile-1\",\"profile_revision\":1,\"provider\":\"deep_seek\",\"model\":{\"provider\":\"deep_seek\",\"value\":\"deepseek/model\"},\"parameters\":{\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0,\"api_key\":\"canary-secret\"}}',
                '0000000000000000000000000000000000000000000000000000000000000000', 'now'
             )",
            [],
        )
        .expect_err("fingerprint parameters must reject secret-shaped fields");
    assert!(
        fingerprint_error
            .to_string()
            .contains("non-sensitive schema")
    );

    let validation_error = connection
        .execute(
            "INSERT INTO run_provider_bindings(
                run_id, profile_id, revision, provider, model_id, parameters_json,
                credential_generation, validation_id, validation_digest,
                fingerprint_json, fingerprint_hash, created_at
             ) VALUES (
                'run-1', 'profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}',
                1, 'validation-2', 'digest-2',
                '{\"schema_version\":1,\"profile_id\":\"profile-1\",\"profile_revision\":1,\"provider\":\"deep_seek\",\"model\":{\"provider\":\"deep_seek\",\"value\":\"deepseek/model\"},\"parameters\":{\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}}',
                '0000000000000000000000000000000000000000000000000000000000000000', 'now'
             )",
            [],
        )
        .expect_err("run binding must not reference another Profile validation");
    assert!(
        validation_error
            .to_string()
            .contains("Run Provider binding")
    );
    let fingerprint_identity_error = connection
        .execute(
            "INSERT INTO run_provider_bindings(
                run_id, profile_id, revision, provider, model_id, parameters_json,
                credential_generation, validation_id, validation_digest,
                fingerprint_json, fingerprint_hash, created_at
             ) VALUES (
                'run-1', 'profile-1', 1, 'deep_seek', 'deepseek/model', '{\"schema_version\":1,\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}',
                1, 'validation-1', 'digest-1',
                '{\"schema_version\":1,\"profile_id\":\"profile-1\",\"profile_revision\":2,\"provider\":\"deep_seek\",\"model\":{\"provider\":\"deep_seek\",\"value\":\"deepseek/model\"},\"parameters\":{\"temperature\":null,\"max_tokens\":null,\"timeout_seconds\":30,\"retry_count\":0}}',
                '0000000000000000000000000000000000000000000000000000000000000000', 'now'
             )",
            [],
        )
        .expect_err("fingerprint identity must match the Run binding snapshot");
    assert!(
        fingerprint_identity_error
            .to_string()
            .contains("non-sensitive schema")
    );
    let validation_mutation_error = connection
        .execute(
            "UPDATE provider_validations SET outcome = 'failed', error_code = 'provider.model.incompatible'
             WHERE validation_id = 'validation-1'",
            [],
        )
        .expect_err("validation evidence must remain immutable after activation");
    assert!(
        validation_mutation_error
            .to_string()
            .contains("validations are insert-only")
    );
    let credential_mutation_error = connection
        .execute(
            "UPDATE provider_credential_generations SET vault_locator = 'vault://replaced'
             WHERE profile_id = 'profile-1' AND generation = 1",
            [],
        )
        .expect_err("credential generation identity must be immutable");
    assert!(
        credential_mutation_error
            .to_string()
            .contains("credential generations are immutable")
    );
    connection
        .execute_batch("ROLLBACK")
        .expect("discard fixture");
}

#[tokio::test]
async fn failed_provider_migration_does_not_leave_partial_schema() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let connection = Connection::open(&database).expect("open database");
    connection
        .execute_batch(
            "CREATE TABLE provider_profiles (unexpected TEXT);
             CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
             );",
        )
        .expect("seed incompatible schema");
    drop(connection);

    let error = SqliteRuntimeStore::open(&database)
        .await
        .expect_err("incompatible provider schema must reject migration");
    assert!(error.to_string().contains("provider_profiles"));

    let connection = Connection::open(&database).expect("inspect rejected migration");
    let created_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table'
               AND name IN (
                   'provider_profile_revisions', 'provider_credential_generations',
                   'provider_validations', 'active_provider', 'credential_mutations',
                   'run_provider_bindings'
               )",
            [],
            |row| row.get(0),
        )
        .expect("query partial provider schema");
    assert_eq!(created_tables, 0);
    let version_two: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 2",
            [],
            |row| row.get(0),
        )
        .expect("query rejected migration version");
    assert_eq!(version_two, 0);
}

struct StoreFixture {
    _directory: TempDir,
    store: SqliteRuntimeStore,
}

async fn create_run(store: &SqliteRuntimeStore, snapshot: RunSnapshot) -> CreateRunCommand {
    let profile_id = ProfileId::new();
    let repository = store.provider_repository();
    let model = ProviderModelId::new(ProviderId::DeepSeek, "deepseek/test-model")
        .expect("test model prefix");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: ProfileName::new(format!("Runtime test {profile_id}"))
                .expect("test Profile name"),
            revision: ProfileRevision::draft(
                profile_id,
                1,
                ProviderId::DeepSeek,
                model.clone(),
                ProviderParameters::default(),
                None,
            )
            .expect("initial test Profile revision"),
        })
        .await
        .expect("save initial test Profile");
    let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("test credential generation");
    Connection::open(store.database_path())
        .expect("open test database")
        .execute(
            "INSERT INTO provider_credential_generations(
                profile_id, generation, kind, vault_locator, status, created_at, updated_at
             ) VALUES (?1, 1, 'api_key', ?2, 'available', 'now', 'now')",
            [
                profile_id.to_string(),
                format!("io.ysda.runtime-test://{profile_id}:1"),
            ],
        )
        .expect("seed test Credential metadata");
    let revision = ProfileRevision::draft(
        profile_id,
        2,
        ProviderId::DeepSeek,
        model,
        ProviderParameters::default(),
        Some(credential),
    )
    .expect("test provider revision");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(1),
            },
            name: ProfileName::new(format!("Runtime test {profile_id}"))
                .expect("test Profile name"),
            revision: revision.clone(),
        })
        .await
        .expect("save test Provider revision");
    let versions =
        ValidationVersions::new("test-catalog", "test-probe", "test-liter", "test-codec");
    let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();
    repository
        .save_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id,
                revision: 2,
                credential_generation: credential,
                validation_digest: validation_digest.clone(),
            },
            evidence,
            versions,
        })
        .await
        .expect("save test Provider validation");
    let active = repository
        .activate(ActivateProfileRequest {
            operation_id: OperationId::new(),
            precondition: ActivationPrecondition {
                profile_id,
                revision: 2,
                validation_id,
                validation_digest,
                expected_activation_revision: None,
            },
        })
        .await
        .expect("activate test Provider");
    let run_id = snapshot.run_id;
    CreateRunCommand::new(
        snapshot,
        RunProviderBinding::from_active(run_id, active).expect("test Run binding"),
        datasource_support::datasource_binding(run_id),
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
                create_run: Some(create_run(&self.store, snapshot.clone()).await),
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
            create_run: Some(create_run(&store, initial.clone()).await),
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
        create_run: Some(create_run(&fixture.store, snapshot).await),
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
