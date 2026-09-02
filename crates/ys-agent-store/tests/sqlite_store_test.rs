use std::{fs, path::Path};

use rusqlite::Connection;
use tempfile::TempDir;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, ActiveProviderSnapshot, ArtifactKind,
    ArtifactStore, CommandId, CommandReceipt, CommandResultKind, CompatibilityEvidence, CoreError,
    CreateRunCommand, CredentialGeneration, CredentialKind, OperationId, PendingRunEvent,
    ProfileId, ProfileName, ProfileRevision, ProfileState, ProviderId, ProviderModelId,
    ProviderParameters, PutArtifact, RevisionPrecondition, Run, RunEventKind, RunProviderBinding,
    RunSnapshot, RunStatus, RuntimeCommandBatch, RuntimeStore, SaveProfileRevision, Sensitivity,
    Task, ValidationCommit, ValidationCommitPrecondition, ValidationVersions, WorkflowKind,
    WorkspaceId,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

const RUNTIME_MIGRATION: &str = include_str!("../migrations/0001_runtime.sql");

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

    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(
        ValidationVersions::new("catalog-v1", "probe-v1", "liter-v1", "codec-v1"),
    ));
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
