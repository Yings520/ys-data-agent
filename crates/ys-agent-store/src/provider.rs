use std::{path::PathBuf, str::FromStr};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use ys_agent_core::{
    ActivateProfileRequest, ActiveProviderSnapshot, CredentialGeneration, CredentialKind,
    CredentialMutationIntent, CredentialMutationOperation, CredentialMutationPhase,
    CredentialMutationRecord, CredentialMutationRepository, CredentialPointerCommit,
    CredentialViewStatus, OperationId, PersistedCompatibilityEvidence,
    PersistedCredentialMutationRecord, PersistedProfileRevision, ProfileId, ProfileRevision,
    ProfileRevisionRepository, ProfileState, ProfileSummary, ProviderErrorCode, ProviderField,
    ProviderId, ProviderManagementError, ProviderModelId, ProviderParameters, ProviderRemediation,
    ProviderResult, RevisionPrecondition, RunId, RunProviderBinding, RunProviderBindingRepository,
    SaveProfileRevision, ValidationCommit,
};

use crate::{SqliteRuntimeStore, sqlite::open_connection};

type RevisionRow = (
    String,
    String,
    String,
    Option<i64>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

type CredentialMutationRow = (
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    String,
    String,
    Option<String>,
);

/// SQLite implementation of Provider revision state and the credential mutation journal.
/// Run-binding persistence is added by its dedicated follow-on task.
#[derive(Debug, Clone)]
pub struct SqliteProviderRepository {
    database: PathBuf,
}

/// Read-only access to immutable Run bindings and their nonterminal retention guards.
#[derive(Debug, Clone)]
pub struct SqliteRunBindingRepository {
    database: PathBuf,
}

impl SqliteRuntimeStore {
    pub fn provider_repository(&self) -> SqliteProviderRepository {
        SqliteProviderRepository {
            database: self.database_path().to_path_buf(),
        }
    }

    pub fn run_binding_repository(&self) -> SqliteRunBindingRepository {
        SqliteRunBindingRepository {
            database: self.database_path().to_path_buf(),
        }
    }
}

impl SqliteRunBindingRepository {
    async fn with_connection<T, F>(&self, operation: F) -> ProviderResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> ProviderResult<T> + Send + 'static,
    {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = open_connection(&database).map_err(|_| internal_error())?;
            operation(&connection)
        })
        .await
        .map_err(|_| internal_error())?
    }
}

#[async_trait]
impl RunProviderBindingRepository for SqliteRunBindingRepository {
    async fn load_run_binding(&self, run_id: RunId) -> ProviderResult<RunProviderBinding> {
        self.with_connection(move |connection| load_run_binding(connection, run_id))
            .await
    }

    async fn has_nonterminal_profile_references(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<bool> {
        self.with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM run_provider_bindings AS binding
                        JOIN runs AS run ON run.run_id = binding.run_id
                        WHERE binding.profile_id = ?1
                          AND run.status NOT IN ('Succeeded', 'Failed', 'Cancelled')
                     )",
                    [profile_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(provider_storage_error)
        })
        .await
    }

    async fn has_nonterminal_credential_references(
        &self,
        credential: CredentialGeneration,
    ) -> ProviderResult<bool> {
        self.with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM run_provider_bindings AS binding
                        JOIN runs AS run ON run.run_id = binding.run_id
                        WHERE binding.profile_id = ?1
                          AND binding.credential_generation = ?2
                          AND run.status NOT IN ('Succeeded', 'Failed', 'Cancelled')
                     )",
                    params![
                        credential.profile_id().to_string(),
                        to_i64(credential.number())?
                    ],
                    |row| row.get(0),
                )
                .map_err(provider_storage_error)
        })
        .await
    }
}

impl SqliteProviderRepository {
    async fn with_connection<T, F>(&self, operation: F) -> ProviderResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> ProviderResult<T> + Send + 'static,
    {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = open_connection(&database).map_err(|_| internal_error())?;
            operation(&mut connection)
        })
        .await
        .map_err(|_| internal_error())?
    }

    pub async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT profile.profile_id, profile.name, revision.provider, revision.state,
                            credential.status,
                            EXISTS(
                                SELECT 1 FROM active_provider AS active
                                WHERE active.profile_id = profile.profile_id
                            )
                     FROM provider_profiles AS profile
                     JOIN provider_profile_revisions AS revision
                       ON revision.profile_id = profile.profile_id
                      AND revision.revision = profile.current_revision
                     LEFT JOIN provider_credential_generations AS credential
                       ON credential.profile_id = revision.profile_id
                      AND credential.generation = revision.credential_generation
                     ORDER BY profile.name COLLATE NOCASE",
                )
                .map_err(provider_storage_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, bool>(5)?,
                    ))
                })
                .map_err(provider_storage_error)?;
            rows.map(|row| {
                let (profile_id, name, provider, state, credential_status, is_active) =
                    row.map_err(provider_storage_error)?;
                Ok(ProfileSummary {
                    profile_id: parse_id(&profile_id)?,
                    name,
                    provider: parse_enum(&provider)?,
                    state: parse_enum(&state)?,
                    credential_status: credential_view_status(credential_status.as_deref()),
                    is_active,
                })
            })
            .collect()
        })
        .await
    }

    pub async fn load_revision(
        &self,
        profile_id: ProfileId,
        revision: u64,
    ) -> ProviderResult<ProfileRevision> {
        self.with_connection(move |connection| load_revision(connection, profile_id, revision))
            .await
    }

    pub async fn load_current_revision(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<ProfileRevision> {
        self.with_connection(move |connection| {
            let revision =
                current_revision(connection, profile_id)?.ok_or_else(storage_conflict_error)?;
            load_revision(connection, profile_id, revision)
        })
        .await
    }

    pub async fn save_revision(
        &self,
        request: SaveProfileRevision,
    ) -> ProviderResult<ProfileRevision> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let current_revision = current_revision(&transaction, request.precondition.profile_id)?;
            validate_save_precondition(&request.precondition, &request.revision, current_revision)?;

            let now = Utc::now().to_rfc3339();
            if current_revision.is_none() {
                transaction
                    .execute(
                        "INSERT INTO provider_profiles(
                            profile_id, name, current_revision, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, ?4, ?4)",
                        params![
                            request.revision.profile_id().to_string(),
                            request.name.as_str(),
                            to_i64(request.revision.revision())?,
                            now,
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }

            insert_revision(&transaction, &request.revision, &now)?;
            if current_revision.is_some() {
                transaction
                    .execute(
                        "UPDATE provider_profiles
                         SET name = ?1, current_revision = ?2, updated_at = ?3
                         WHERE profile_id = ?4",
                        params![
                            request.name.as_str(),
                            to_i64(request.revision.revision())?,
                            now,
                            request.revision.profile_id().to_string(),
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }
            transaction.commit().map_err(provider_storage_error)?;
            Ok(request.revision)
        })
        .await
    }

    pub async fn save_validation(
        &self,
        commit: ValidationCommit,
    ) -> ProviderResult<ProfileRevision> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let current = current_revision(&transaction, commit.precondition.profile_id)?;
            if current != Some(commit.precondition.revision) {
                return Err(validation_stale_error());
            }
            if commit.evidence.digest() != commit.precondition.validation_digest {
                return Err(validation_stale_error());
            }

            let revision = load_revision(
                &transaction,
                commit.precondition.profile_id,
                commit.precondition.revision,
            )?;
            if revision.credential_generation() != Some(commit.precondition.credential_generation)
                || revision.state() != ProfileState::Draft
            {
                return Err(validation_stale_error());
            }

            let state = if commit.evidence.passed() {
                ProfileState::Ready
            } else {
                ProfileState::Invalid
            };
            let persisted = commit.evidence.persisted();
            let hydrated = ProfileRevision::hydrate(PersistedProfileRevision {
                profile_id: revision.profile_id(),
                revision: revision.revision(),
                provider: revision.provider(),
                model: revision.model().clone(),
                parameters: revision.parameters().clone(),
                credential_generation: revision.credential_generation(),
                state,
                validation: Some(persisted),
            })
            .map_err(|_| internal_error())?;
            insert_validation(&transaction, &commit, state)?;
            transaction
                .execute(
                    "UPDATE provider_profile_revisions
                     SET state = ?1, validation_id = ?2
                     WHERE profile_id = ?3 AND revision = ?4 AND state = 'draft'",
                    params![
                        enum_name(&state)?,
                        commit.evidence.id().to_string(),
                        commit.precondition.profile_id.to_string(),
                        to_i64(commit.precondition.revision)?,
                    ],
                )
                .map_err(provider_storage_error)?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(hydrated)
        })
        .await
    }

    pub async fn activate(
        &self,
        request: ActivateProfileRequest,
    ) -> ProviderResult<ActiveProviderSnapshot> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let revision = load_revision(
                &transaction,
                request.precondition.profile_id,
                request.precondition.revision,
            )?;
            let validation = revision.validation().ok_or_else(activation_error)?;
            if revision.state() != ProfileState::Ready
                || validation.id() != request.precondition.validation_id
                || validation.digest() != request.precondition.validation_digest
            {
                return Err(activation_error());
            }

            let current_activation: Option<i64> = transaction
                .query_row(
                    "SELECT activation_revision FROM active_provider WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(provider_storage_error)?;
            let current_activation = current_activation
                .map(|value| u64::try_from(value).map_err(|_| internal_error()))
                .transpose()?;
            if current_activation != request.precondition.expected_activation_revision {
                return Err(activation_error());
            }
            let activation_revision = current_activation
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(internal_error)?;
            let snapshot = ActiveProviderSnapshot::from_ready(&revision, activation_revision)
                .map_err(|_| activation_error())?;
            let credential_generation = revision
                .credential_generation()
                .ok_or_else(activation_error)?;
            transaction
                .execute(
                    "INSERT INTO active_provider(
                        singleton, profile_id, revision, validation_id, credential_generation,
                        validation_digest, activation_revision, activated_at
                     ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(singleton) DO UPDATE SET
                        profile_id = excluded.profile_id,
                        revision = excluded.revision,
                        validation_id = excluded.validation_id,
                        credential_generation = excluded.credential_generation,
                        validation_digest = excluded.validation_digest,
                        activation_revision = excluded.activation_revision,
                        activated_at = excluded.activated_at",
                    params![
                        revision.profile_id().to_string(),
                        to_i64(revision.revision())?,
                        validation.id().to_string(),
                        to_i64(credential_generation.number())?,
                        validation.digest().as_str(),
                        to_i64(activation_revision)?,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(provider_storage_error)?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(snapshot)
        })
        .await
    }

    pub async fn active(&self) -> ProviderResult<Option<ActiveProviderSnapshot>> {
        self.with_connection(|connection| {
            let active: Option<(String, i64, i64)> = connection
                .query_row(
                    "SELECT profile_id, revision, activation_revision
                     FROM active_provider WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(provider_storage_error)?;
            active
                .map(|(profile_id, revision, activation_revision)| {
                    let profile_id = parse_id(&profile_id)?;
                    let blocked_code: Option<String> = connection
                        .query_row(
                            "SELECT error_code FROM credential_mutations
                             WHERE profile_id = ?1 AND phase = 'blocked'
                             ORDER BY updated_at DESC LIMIT 1",
                            [profile_id.to_string()],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(provider_storage_error)?;
                    if let Some(code) = blocked_code {
                        return Err(persisted_block_error(parse_enum(&code)?));
                    }
                    let revision = u64::try_from(revision).map_err(|_| internal_error())?;
                    let activation_revision =
                        u64::try_from(activation_revision).map_err(|_| internal_error())?;
                    let revision = load_revision(connection, profile_id, revision)?;
                    ActiveProviderSnapshot::from_ready(&revision, activation_revision)
                        .map_err(|_| internal_error())
                })
                .transpose()
        })
        .await
    }

    pub async fn begin_credential_mutation(
        &self,
        intent: CredentialMutationIntent,
    ) -> ProviderResult<CredentialMutationRecord> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            if let Some((phase, error_code)) = transaction
                .query_row(
                    "SELECT phase, error_code
                     FROM credential_mutations
                     WHERE profile_id = ?1 AND phase NOT IN ('completed', 'rolled_back')
                     ORDER BY created_at DESC LIMIT 1",
                    [intent.profile_id().to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()
                .map_err(provider_storage_error)?
            {
                if phase == "blocked" {
                    let code = error_code
                        .as_deref()
                        .map(parse_enum)
                        .transpose()?
                        .ok_or_else(internal_error)?;
                    return Err(persisted_block_error(code));
                }
                return Err(operation_stale_error());
            }

            let current = current_revision(&transaction, intent.profile_id())?;
            if current != Some(intent.expected_revision()) {
                return Err(storage_conflict_error());
            }
            let revision = load_revision(
                &transaction,
                intent.profile_id(),
                intent.expected_revision(),
            )?;
            if revision.credential_generation() != intent.expected_generation() {
                return Err(storage_conflict_error());
            }
            for generation in [intent.new_generation(), intent.rollback_generation()]
                .into_iter()
                .flatten()
            {
                if generation.kind() != revision.provider().required_credential_kind()
                    || generation_metadata_exists(
                        &transaction,
                        intent.profile_id(),
                        generation.number(),
                    )?
                    || !generation_is_newest(
                        &transaction,
                        intent.profile_id(),
                        generation.number(),
                    )?
                {
                    return Err(storage_conflict_error());
                }
            }

            let record = CredentialMutationRecord::intent_recorded(intent);
            let now = Utc::now().to_rfc3339();
            let staged = staged_generation(&record).ok_or_else(internal_error)?;
            transaction
                .execute(
                    "INSERT INTO provider_credential_generations(
                        profile_id, generation, kind, vault_locator, status, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, 'retained', ?5, ?5)",
                    params![
                        staged.profile_id().to_string(),
                        to_i64(staged.number())?,
                        enum_name(&staged.kind())?,
                        credential_locator(staged),
                        now,
                    ],
                )
                .map_err(provider_storage_error)?;
            transaction
                .execute(
                    "INSERT INTO credential_mutations(
                        mutation_id, profile_id, expected_revision, old_generation,
                        new_generation, rollback_generation, operation, phase,
                        error_code, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'intent_recorded', NULL, ?8, ?8)",
                    params![
                        record.operation_id().to_string(),
                        record.profile_id().to_string(),
                        to_i64(record.expected_revision())?,
                        optional_generation_number(record.old_generation())?,
                        optional_generation_number(record.new_generation())?,
                        optional_generation_number(record.rollback_generation())?,
                        enum_name(&record.operation())?,
                        now,
                    ],
                )
                .map_err(provider_storage_error)?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(record)
        })
        .await
    }

    pub async fn record_credential_vault_write(
        &self,
        mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let record = load_credential_mutation(&transaction, mutation_id)?;
            if record.phase() == CredentialMutationPhase::VaultWritten {
                return Ok(record);
            }
            if record.phase() != CredentialMutationPhase::IntentRecorded {
                return Err(operation_stale_error());
            }
            let generation = staged_generation(&record).ok_or_else(internal_error)?;
            let locator = credential_locator(generation);
            let changed = transaction
                .execute(
                    "UPDATE provider_credential_generations
                     SET status = 'available', updated_at = ?1
                     WHERE profile_id = ?2 AND generation = ?3
                       AND kind = ?4 AND vault_locator = ?5 AND status = 'retained'",
                    params![
                        Utc::now().to_rfc3339(),
                        generation.profile_id().to_string(),
                        to_i64(generation.number())?,
                        enum_name(&generation.kind())?,
                        locator,
                    ],
                )
                .map_err(provider_storage_error)?;
            if changed != 1 {
                return Err(storage_conflict_error());
            }
            let persisted: Option<(String, String)> = transaction
                .query_row(
                    "SELECT kind, vault_locator
                     FROM provider_credential_generations
                     WHERE profile_id = ?1 AND generation = ?2",
                    params![
                        generation.profile_id().to_string(),
                        to_i64(generation.number())?
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(provider_storage_error)?;
            if persisted
                != Some((
                    enum_name(&generation.kind())?,
                    credential_locator(generation),
                ))
            {
                return Err(storage_conflict_error());
            }
            let record = transition_credential_mutation(
                &transaction,
                record,
                CredentialMutationPhase::VaultWritten,
                None,
            )?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(record)
        })
        .await
    }

    pub async fn commit_credential_pointer(
        &self,
        commit: CredentialPointerCommit,
    ) -> ProviderResult<CredentialMutationRecord> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let record = load_credential_mutation(&transaction, commit.mutation_id())?;
            let commit_matches_record = record.profile_id() == commit.profile_id()
                && record.expected_revision() == commit.expected_revision()
                && record.new_generation() == commit.new_generation();
            if record.phase() == CredentialMutationPhase::PointerCommitted {
                let persisted = load_revision(
                    &transaction,
                    commit.profile_id(),
                    commit.replacement_revision().revision(),
                )?;
                if commit_matches_record
                    && revision_configuration_matches(&persisted, commit.replacement_revision())
                {
                    return Ok(record);
                }
                return Err(storage_conflict_error());
            }
            if record.phase() != CredentialMutationPhase::VaultWritten {
                return Err(operation_stale_error());
            }
            let current = current_revision(&transaction, commit.profile_id())?;
            let configuration_matches = if current == Some(commit.expected_revision()) {
                let prior = load_revision(
                    &transaction,
                    commit.profile_id(),
                    commit.expected_revision(),
                )?;
                revision_profile_configuration_matches(&prior, commit.replacement_revision())
            } else {
                false
            };
            let stale = !commit_matches_record
                || current != Some(commit.expected_revision())
                || !configuration_matches;
            if stale {
                let record = transition_credential_mutation(
                    &transaction,
                    record,
                    CredentialMutationPhase::CleanupPending,
                    None,
                )?;
                debug_assert_eq!(record.phase(), CredentialMutationPhase::CleanupPending);
                transaction.commit().map_err(provider_storage_error)?;
                return Err(storage_conflict_error());
            }

            let now = Utc::now().to_rfc3339();
            insert_revision(&transaction, commit.replacement_revision(), &now)?;
            let changed = transaction
                .execute(
                    "UPDATE provider_profiles
                     SET current_revision = ?1, updated_at = ?2
                     WHERE profile_id = ?3 AND current_revision = ?4",
                    params![
                        to_i64(commit.replacement_revision().revision())?,
                        now,
                        commit.profile_id().to_string(),
                        to_i64(commit.expected_revision())?,
                    ],
                )
                .map_err(provider_storage_error)?;
            if changed != 1 {
                return Err(storage_conflict_error());
            }
            if let Some(old_generation) = record.old_generation() {
                transaction
                    .execute(
                        "UPDATE provider_credential_generations
                         SET status = 'retained', updated_at = ?1
                         WHERE profile_id = ?2 AND generation = ?3 AND status != 'deleted'",
                        params![
                            Utc::now().to_rfc3339(),
                            old_generation.profile_id().to_string(),
                            to_i64(old_generation.number())?,
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }
            let record = transition_credential_mutation(
                &transaction,
                record,
                CredentialMutationPhase::PointerCommitted,
                None,
            )?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(record)
        })
        .await
    }

    pub async fn complete_credential_mutation(
        &self,
        mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let record = load_credential_mutation(&transaction, mutation_id)?;
            if record.phase() == CredentialMutationPhase::Completed {
                return Ok(record);
            }
            if record.phase() != CredentialMutationPhase::PointerCommitted {
                return Err(operation_stale_error());
            }
            if let Some(old_generation) = record.old_generation() {
                transaction
                    .execute(
                        "UPDATE provider_credential_generations
                         SET status = 'retained', updated_at = ?1
                         WHERE profile_id = ?2 AND generation = ?3",
                        params![
                            Utc::now().to_rfc3339(),
                            record.profile_id().to_string(),
                            to_i64(old_generation.number())?,
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }
            if let Some(rollback_generation) = record.rollback_generation() {
                transaction
                    .execute(
                        "UPDATE provider_credential_generations
                         SET status = 'deleted', updated_at = ?1
                         WHERE profile_id = ?2 AND generation = ?3",
                        params![
                            Utc::now().to_rfc3339(),
                            record.profile_id().to_string(),
                            to_i64(rollback_generation.number())?,
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }
            let record = transition_credential_mutation(
                &transaction,
                record,
                CredentialMutationPhase::Completed,
                None,
            )?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(record)
        })
        .await
    }

    pub async fn rollback_credential_mutation(
        &self,
        mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let record = load_credential_mutation(&transaction, mutation_id)?;
            if record.phase() == CredentialMutationPhase::RolledBack {
                return Ok(record);
            }
            if !matches!(
                record.phase(),
                CredentialMutationPhase::IntentRecorded
                    | CredentialMutationPhase::VaultWritten
                    | CredentialMutationPhase::CleanupPending
            ) {
                return Err(operation_stale_error());
            }
            if let Some(generation) = staged_generation(&record) {
                transaction
                    .execute(
                        "UPDATE provider_credential_generations
                         SET status = 'deleted', updated_at = ?1
                         WHERE profile_id = ?2 AND generation = ?3",
                        params![
                            Utc::now().to_rfc3339(),
                            generation.profile_id().to_string(),
                            to_i64(generation.number())?,
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }
            let record = transition_credential_mutation(
                &transaction,
                record,
                CredentialMutationPhase::RolledBack,
                None,
            )?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(record)
        })
        .await
    }

    pub async fn block_credential_mutation(
        &self,
        mutation_id: OperationId,
        error_code: ProviderErrorCode,
    ) -> ProviderResult<CredentialMutationRecord> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let record = load_credential_mutation(&transaction, mutation_id)?;
            if record.phase() == CredentialMutationPhase::Blocked {
                return if record.error_code() == Some(error_code) {
                    Ok(record)
                } else {
                    Err(storage_conflict_error())
                };
            }
            if record.phase().is_terminal() {
                return Err(operation_stale_error());
            }
            if let Some(generation) = staged_generation(&record) {
                transaction
                    .execute(
                        "UPDATE provider_credential_generations
                         SET status = 'revoked', updated_at = ?1
                         WHERE profile_id = ?2 AND generation = ?3",
                        params![
                            Utc::now().to_rfc3339(),
                            generation.profile_id().to_string(),
                            to_i64(generation.number())?,
                        ],
                    )
                    .map_err(provider_storage_error)?;
            }
            transaction
                .execute(
                    "DELETE FROM active_provider WHERE profile_id = ?1",
                    [record.profile_id().to_string()],
                )
                .map_err(provider_storage_error)?;
            let record = transition_credential_mutation(
                &transaction,
                record,
                CredentialMutationPhase::Blocked,
                Some(error_code),
            )?;
            transaction.commit().map_err(provider_storage_error)?;
            Ok(record)
        })
        .await
    }

    pub async fn pending_credential_mutations(
        &self,
    ) -> ProviderResult<Vec<CredentialMutationRecord>> {
        self.with_connection(|connection| {
            let rows = {
                let mut statement = connection
                    .prepare(
                        "SELECT mutation_id, profile_id, expected_revision, old_generation,
                                new_generation, rollback_generation, operation, phase, error_code
                         FROM credential_mutations
                         WHERE phase NOT IN ('completed', 'rolled_back')
                         ORDER BY created_at, mutation_id",
                    )
                    .map_err(provider_storage_error)?;
                let mapped = statement
                    .query_map([], credential_mutation_row)
                    .map_err(provider_storage_error)?;
                mapped
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(provider_storage_error)?
            };
            rows.into_iter()
                .map(|row| hydrate_credential_mutation(connection, row))
                .collect()
        })
        .await
    }

    pub async fn retire_credential_generation(
        &self,
        generation: CredentialGeneration,
    ) -> ProviderResult<()> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(provider_storage_error)?;
            let active_reference: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM active_provider
                        WHERE profile_id = ?1 AND credential_generation = ?2
                     )",
                    params![
                        generation.profile_id().to_string(),
                        to_i64(generation.number())?
                    ],
                    |row| row.get(0),
                )
                .map_err(provider_storage_error)?;
            let run_reference: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM run_provider_bindings AS binding
                        JOIN runs AS run ON run.run_id = binding.run_id
                        WHERE binding.profile_id = ?1
                          AND binding.credential_generation = ?2
                          AND run.status NOT IN ('Succeeded', 'Failed', 'Cancelled')
                     )",
                    params![
                        generation.profile_id().to_string(),
                        to_i64(generation.number())?
                    ],
                    |row| row.get(0),
                )
                .map_err(provider_storage_error)?;
            if active_reference || run_reference {
                return Err(operation_stale_error());
            }
            let changed = transaction
                .execute(
                    "UPDATE provider_credential_generations
                     SET status = 'deleted', updated_at = ?1
                     WHERE profile_id = ?2 AND generation = ?3 AND status = 'retained'",
                    params![
                        Utc::now().to_rfc3339(),
                        generation.profile_id().to_string(),
                        to_i64(generation.number())?,
                    ],
                )
                .map_err(provider_storage_error)?;
            if changed != 1 {
                return Err(storage_conflict_error());
            }
            transaction.commit().map_err(provider_storage_error)
        })
        .await
    }
}

#[async_trait]
impl ProfileRevisionRepository for SqliteProviderRepository {
    async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
        SqliteProviderRepository::list_profiles(self).await
    }

    async fn load_current_revision(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<ProfileRevision> {
        SqliteProviderRepository::load_current_revision(self, profile_id).await
    }

    async fn load_revision(
        &self,
        profile_id: ProfileId,
        revision: u64,
    ) -> ProviderResult<ProfileRevision> {
        SqliteProviderRepository::load_revision(self, profile_id, revision).await
    }

    async fn save_revision(&self, request: SaveProfileRevision) -> ProviderResult<ProfileRevision> {
        SqliteProviderRepository::save_revision(self, request).await
    }

    async fn active(&self) -> ProviderResult<Option<ActiveProviderSnapshot>> {
        SqliteProviderRepository::active(self).await
    }
}

#[async_trait]
impl CredentialMutationRepository for SqliteProviderRepository {
    async fn begin_credential_mutation(
        &self,
        intent: CredentialMutationIntent,
    ) -> ProviderResult<CredentialMutationRecord> {
        SqliteProviderRepository::begin_credential_mutation(self, intent).await
    }

    async fn record_credential_vault_write(
        &self,
        mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        SqliteProviderRepository::record_credential_vault_write(self, mutation_id).await
    }

    async fn commit_credential_pointer(
        &self,
        commit: CredentialPointerCommit,
    ) -> ProviderResult<CredentialMutationRecord> {
        SqliteProviderRepository::commit_credential_pointer(self, commit).await
    }

    async fn complete_credential_mutation(
        &self,
        mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        SqliteProviderRepository::complete_credential_mutation(self, mutation_id).await
    }

    async fn rollback_credential_mutation(
        &self,
        mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        SqliteProviderRepository::rollback_credential_mutation(self, mutation_id).await
    }

    async fn block_credential_mutation(
        &self,
        mutation_id: OperationId,
        error_code: ProviderErrorCode,
    ) -> ProviderResult<CredentialMutationRecord> {
        SqliteProviderRepository::block_credential_mutation(self, mutation_id, error_code).await
    }

    async fn pending_credential_mutations(&self) -> ProviderResult<Vec<CredentialMutationRecord>> {
        SqliteProviderRepository::pending_credential_mutations(self).await
    }

    async fn retire_credential_generation(
        &self,
        generation: CredentialGeneration,
    ) -> ProviderResult<()> {
        SqliteProviderRepository::retire_credential_generation(self, generation).await
    }
}

fn credential_mutation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialMutationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn load_credential_mutation(
    connection: &Connection,
    mutation_id: OperationId,
) -> ProviderResult<CredentialMutationRecord> {
    let row = connection
        .query_row(
            "SELECT mutation_id, profile_id, expected_revision, old_generation,
                    new_generation, rollback_generation, operation, phase, error_code
             FROM credential_mutations WHERE mutation_id = ?1",
            [mutation_id.to_string()],
            credential_mutation_row,
        )
        .optional()
        .map_err(provider_storage_error)?
        .ok_or_else(operation_stale_error)?;
    hydrate_credential_mutation(connection, row)
}

fn hydrate_credential_mutation(
    connection: &Connection,
    row: CredentialMutationRow,
) -> ProviderResult<CredentialMutationRecord> {
    let (
        mutation_id,
        profile_id,
        expected_revision,
        old_generation,
        new_generation,
        rollback_generation,
        operation,
        phase,
        error_code,
    ) = row;
    let profile_id = parse_id(&profile_id)?;
    let expected_revision = u64::try_from(expected_revision).map_err(|_| internal_error())?;
    let (kind_count, kind): (i64, Option<String>) = connection
        .query_row(
            "SELECT COUNT(DISTINCT kind), MIN(kind)
             FROM provider_credential_generations
             WHERE profile_id = ?1
               AND generation IN (?2, ?3, ?4)",
            params![
                profile_id.to_string(),
                old_generation,
                new_generation,
                rollback_generation,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(provider_storage_error)?;
    if kind_count != 1 {
        return Err(internal_error());
    }
    let kind = parse_enum::<CredentialKind>(&kind.ok_or_else(internal_error)?)?;
    let generation = |number: Option<i64>| -> ProviderResult<Option<CredentialGeneration>> {
        number
            .map(|number| {
                CredentialGeneration::new(
                    profile_id,
                    u64::try_from(number).map_err(|_| internal_error())?,
                    kind,
                )
                .map_err(|_| internal_error())
            })
            .transpose()
    };
    CredentialMutationRecord::hydrate(PersistedCredentialMutationRecord {
        operation_id: mutation_id.parse().map_err(|_| internal_error())?,
        profile_id,
        expected_revision,
        operation: parse_enum(&operation)?,
        old_generation: generation(old_generation)?,
        new_generation: generation(new_generation)?,
        rollback_generation: generation(rollback_generation)?,
        phase: parse_enum(&phase)?,
        error_code: error_code.as_deref().map(parse_enum).transpose()?,
    })
    .map_err(|_| internal_error())
}

fn transition_credential_mutation(
    connection: &Connection,
    record: CredentialMutationRecord,
    phase: CredentialMutationPhase,
    error_code: Option<ProviderErrorCode>,
) -> ProviderResult<CredentialMutationRecord> {
    let previous_phase = record.phase();
    let record = record
        .transition(phase, error_code)
        .map_err(|_| internal_error())?;
    let changed = connection
        .execute(
            "UPDATE credential_mutations
             SET phase = ?1, error_code = ?2, updated_at = ?3
             WHERE mutation_id = ?4 AND phase = ?5",
            params![
                enum_name(&record.phase())?,
                record.error_code().map(|code| code.as_str()),
                Utc::now().to_rfc3339(),
                record.operation_id().to_string(),
                enum_name(&previous_phase)?,
            ],
        )
        .map_err(provider_storage_error)?;
    if changed != 1 {
        return Err(storage_conflict_error());
    }
    Ok(record)
}

fn staged_generation(record: &CredentialMutationRecord) -> Option<CredentialGeneration> {
    match record.operation() {
        CredentialMutationOperation::Create
        | CredentialMutationOperation::Replace
        | CredentialMutationOperation::Refresh => record.new_generation(),
        CredentialMutationOperation::Delete | CredentialMutationOperation::Revoke => {
            record.rollback_generation()
        }
    }
}

fn revision_configuration_matches(left: &ProfileRevision, right: &ProfileRevision) -> bool {
    left.profile_id() == right.profile_id()
        && left.revision() == right.revision()
        && left.provider() == right.provider()
        && left.model() == right.model()
        && left.parameters() == right.parameters()
        && left.credential_generation() == right.credential_generation()
}

fn revision_profile_configuration_matches(left: &ProfileRevision, right: &ProfileRevision) -> bool {
    left.profile_id() == right.profile_id()
        && left.provider() == right.provider()
        && left.model() == right.model()
        && left.parameters() == right.parameters()
}

fn optional_generation_number(
    generation: Option<CredentialGeneration>,
) -> ProviderResult<Option<i64>> {
    generation
        .map(|generation| to_i64(generation.number()))
        .transpose()
}

fn generation_metadata_exists(
    connection: &Connection,
    profile_id: ProfileId,
    generation: u64,
) -> ProviderResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM provider_credential_generations
                WHERE profile_id = ?1 AND generation = ?2
             )",
            params![profile_id.to_string(), to_i64(generation)?],
            |row| row.get(0),
        )
        .map_err(provider_storage_error)
}

fn generation_is_newest(
    connection: &Connection,
    profile_id: ProfileId,
    generation: u64,
) -> ProviderResult<bool> {
    let maximum: Option<i64> = connection
        .query_row(
            "SELECT MAX(generation) FROM provider_credential_generations WHERE profile_id = ?1",
            [profile_id.to_string()],
            |row| row.get(0),
        )
        .map_err(provider_storage_error)?;
    maximum
        .map(|maximum| {
            u64::try_from(maximum)
                .map(|maximum| generation > maximum)
                .map_err(|_| internal_error())
        })
        .unwrap_or(Ok(true))
}

fn credential_locator(generation: CredentialGeneration) -> String {
    format!(
        "io.ysda.provider://{}:{}",
        generation.profile_id(),
        generation.number()
    )
}

fn current_revision(connection: &Connection, profile_id: ProfileId) -> ProviderResult<Option<u64>> {
    connection
        .query_row(
            "SELECT current_revision FROM provider_profiles WHERE profile_id = ?1",
            [profile_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(provider_storage_error)?
        .map(|revision| u64::try_from(revision).map_err(|_| internal_error()))
        .transpose()
}

fn validate_save_precondition(
    precondition: &RevisionPrecondition,
    revision: &ProfileRevision,
    current_revision: Option<u64>,
) -> ProviderResult<()> {
    if precondition.profile_id != revision.profile_id()
        || revision.state() != ProfileState::Draft
        || revision.validation().is_some()
        || precondition.expected_current_revision != current_revision
    {
        return Err(storage_conflict_error());
    }
    let expected_revision = current_revision.map_or(1, |current| current + 1);
    if revision.revision() != expected_revision {
        return Err(storage_conflict_error());
    }
    Ok(())
}

fn insert_revision(
    connection: &Connection,
    revision: &ProfileRevision,
    created_at: &str,
) -> ProviderResult<()> {
    connection
        .execute(
            "INSERT INTO provider_profile_revisions(
                profile_id, revision, provider, model_id, parameters_json, credential_generation,
                state, validation_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'draft', NULL, ?7)",
            params![
                revision.profile_id().to_string(),
                to_i64(revision.revision())?,
                enum_name(&revision.provider())?,
                revision.model().as_str(),
                parameters_json(revision.parameters())?,
                revision
                    .credential_generation()
                    .map(|generation| to_i64(generation.number()))
                    .transpose()?,
                created_at,
            ],
        )
        .map_err(provider_storage_error)?;
    Ok(())
}

fn insert_validation(
    connection: &Connection,
    commit: &ValidationCommit,
    state: ProfileState,
) -> ProviderResult<()> {
    let (
        tool_calls_supported,
        non_empty_tool_call_ids,
        multi_turn_tool_results,
        context_limit,
        outcome,
        error_code,
    ) = if commit.evidence.passed() {
        (1_i64, 1_i64, 1_i64, Some(1_i64), "passed", None)
    } else {
        (
            0_i64,
            0_i64,
            0_i64,
            None,
            "failed",
            Some("provider.model.incompatible"),
        )
    };
    let expected_state = if commit.evidence.passed() {
        ProfileState::Ready
    } else {
        ProfileState::Invalid
    };
    if state != expected_state {
        return Err(internal_error());
    }
    connection
        .execute(
            "INSERT INTO provider_validations(
                validation_id, profile_id, revision, credential_generation, validation_digest,
                tool_calls_supported, non_empty_tool_call_ids, multi_turn_tool_results,
                context_limit, outcome, error_code, evidence_schema_version, checked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12)",
            params![
                commit.evidence.id().to_string(),
                commit.precondition.profile_id.to_string(),
                to_i64(commit.precondition.revision)?,
                to_i64(commit.precondition.credential_generation.number())?,
                commit.precondition.validation_digest.as_str(),
                tool_calls_supported,
                non_empty_tool_call_ids,
                multi_turn_tool_results,
                context_limit,
                outcome,
                error_code,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(provider_storage_error)?;
    Ok(())
}

fn load_revision(
    connection: &Connection,
    profile_id: ProfileId,
    revision: u64,
) -> ProviderResult<ProfileRevision> {
    let row: Option<RevisionRow> = connection
        .query_row(
            "SELECT revision.provider, revision.model_id, revision.parameters_json,
                    revision.credential_generation, revision.state, revision.validation_id,
                    validation.validation_digest, validation.outcome, credential.kind
             FROM provider_profile_revisions AS revision
             LEFT JOIN provider_validations AS validation
               ON validation.validation_id = revision.validation_id
              AND validation.profile_id = revision.profile_id
              AND validation.revision = revision.revision
             LEFT JOIN provider_credential_generations AS credential
               ON credential.profile_id = revision.profile_id
              AND credential.generation = revision.credential_generation
             WHERE revision.profile_id = ?1 AND revision.revision = ?2",
            params![profile_id.to_string(), to_i64(revision)?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(provider_storage_error)?;
    let (
        provider,
        model,
        parameters,
        generation,
        state,
        validation_id,
        validation_digest,
        outcome,
        credential_kind,
    ) = row.ok_or_else(internal_error)?;
    let provider: ProviderId = parse_enum(&provider)?;
    let model = ProviderModelId::new(provider, model).map_err(|_| invalid_model_error())?;
    let parameters = parse_parameters(&parameters)?;
    let credential_generation = match (generation, credential_kind) {
        (Some(generation), Some(kind)) => Some(
            CredentialGeneration::new(
                profile_id,
                u64::try_from(generation).map_err(|_| internal_error())?,
                parse_enum::<CredentialKind>(&kind)?,
            )
            .map_err(|_| internal_error())?,
        ),
        (None, None) => None,
        _ => return Err(internal_error()),
    };
    let state: ProfileState = parse_enum(&state)?;
    let validation = match (validation_id, validation_digest, outcome) {
        (Some(id), Some(digest), Some(outcome)) => {
            let passed = match outcome.as_str() {
                "passed" => true,
                "failed" => false,
                _ => return Err(internal_error()),
            };
            Some(
                PersistedCompatibilityEvidence::new(
                    id.parse().map_err(|_| internal_error())?,
                    digest,
                    passed,
                )
                .map_err(|_| internal_error())?,
            )
        }
        (None, None, None) => None,
        _ => return Err(internal_error()),
    };
    ProfileRevision::hydrate(PersistedProfileRevision {
        profile_id,
        revision,
        provider,
        model,
        parameters,
        credential_generation,
        state,
        validation,
    })
    .map_err(|_| internal_error())
}

fn load_run_binding(connection: &Connection, run_id: RunId) -> ProviderResult<RunProviderBinding> {
    type RunBindingRow = (
        String,
        i64,
        String,
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        i64,
    );
    let row: Option<RunBindingRow> = connection
        .query_row(
            "SELECT binding.profile_id, binding.revision, binding.provider,
                    binding.model_id, binding.parameters_json,
                    binding.credential_generation, binding.validation_id,
                    binding.validation_digest, binding.fingerprint_json,
                    binding.fingerprint_hash, binding.activation_revision
             FROM run_provider_bindings AS binding
             WHERE binding.run_id = ?1",
            [run_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .optional()
        .map_err(provider_storage_error)?;
    let (
        profile_id,
        revision,
        provider,
        model,
        parameters,
        credential_generation,
        validation_id,
        validation_digest,
        fingerprint_json,
        fingerprint_hash,
        activation_revision,
    ) = row.ok_or_else(internal_error)?;
    let profile_id = parse_id(&profile_id)?;
    let revision = u64::try_from(revision).map_err(|_| internal_error())?;
    let provider: ProviderId = parse_enum(&provider)?;
    let credential_kind: String = connection
        .query_row(
            "SELECT kind FROM provider_credential_generations
             WHERE profile_id = ?1 AND generation = ?2",
            params![profile_id.to_string(), credential_generation],
            |row| row.get(0),
        )
        .map_err(provider_storage_error)?;
    let credential_generation = CredentialGeneration::new(
        profile_id,
        u64::try_from(credential_generation).map_err(|_| internal_error())?,
        parse_enum(&credential_kind)?,
    )
    .map_err(|_| internal_error())?;
    let revision = ProfileRevision::hydrate(PersistedProfileRevision {
        profile_id,
        revision,
        provider,
        model: ProviderModelId::new(provider, model).map_err(|_| internal_error())?,
        parameters: parse_parameters(&parameters)?,
        credential_generation: Some(credential_generation),
        state: ProfileState::Ready,
        validation: Some(
            PersistedCompatibilityEvidence::new(
                validation_id.parse().map_err(|_| internal_error())?,
                validation_digest,
                true,
            )
            .map_err(|_| internal_error())?,
        ),
    })
    .map_err(|_| internal_error())?;
    let active = ActiveProviderSnapshot::from_ready(
        &revision,
        u64::try_from(activation_revision).map_err(|_| internal_error())?,
    )
    .map_err(|_| internal_error())?;
    let binding = RunProviderBinding::from_active(run_id, active).map_err(|_| internal_error())?;
    if binding.fingerprint().canonical_json() != fingerprint_json
        || binding.fingerprint().digest() != fingerprint_hash
    {
        return Err(internal_error());
    }
    Ok(binding)
}

fn parameters_json(parameters: &ProviderParameters) -> ProviderResult<String> {
    let mut value = serde_json::to_value(parameters).map_err(|_| internal_error())?;
    value
        .as_object_mut()
        .ok_or_else(internal_error)?
        .insert("schema_version".to_owned(), serde_json::Value::from(1));
    serde_json::to_string(&value).map_err(|_| internal_error())
}

fn parse_parameters(value: &str) -> ProviderResult<ProviderParameters> {
    serde_json::from_str(value).map_err(|_| internal_error())
}

fn enum_name<T: Serialize>(value: &T) -> ProviderResult<String> {
    serde_json::to_value(value)
        .map_err(|_| internal_error())?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(internal_error)
}

fn parse_enum<T: DeserializeOwned>(value: &str) -> ProviderResult<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| internal_error())
}

fn parse_id(value: &str) -> ProviderResult<ProfileId> {
    ProfileId::from_str(value).map_err(|_| internal_error())
}

fn to_i64(value: u64) -> ProviderResult<i64> {
    i64::try_from(value).map_err(|_| internal_error())
}

fn credential_view_status(status: Option<&str>) -> CredentialViewStatus {
    match status {
        Some("available" | "retained") => CredentialViewStatus::Saved,
        Some("expired") => CredentialViewStatus::Expired,
        Some("revoked") => CredentialViewStatus::Revoked,
        Some("deleted") | None => CredentialViewStatus::Missing,
        Some(_) => CredentialViewStatus::ReconciliationRequired,
    }
}

fn provider_storage_error(error: rusqlite::Error) -> ProviderManagementError {
    let is_profile_name_conflict = matches!(
        &error,
        rusqlite::Error::SqliteFailure(cause, Some(message))
            if cause.code == ErrorCode::ConstraintViolation
                && message.contains("provider_profiles.name")
    );
    if is_profile_name_conflict {
        ProviderManagementError::new(
            ProviderErrorCode::ProfileNameConflict,
            Some(ProviderField::ProfileName),
            ProviderRemediation::ReturnToEdit,
        )
    } else {
        storage_conflict_error()
    }
}

fn invalid_model_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::InvalidModelPrefix,
        Some(ProviderField::Model),
        ProviderRemediation::ReturnToEdit,
    )
}

fn validation_stale_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ValidationStale,
        Some(ProviderField::Validation),
        ProviderRemediation::ValidateProfile,
    )
}

fn activation_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ActivationPreconditionFailed,
        Some(ProviderField::Activation),
        ProviderRemediation::WaitForCurrentOperation,
    )
}

fn storage_conflict_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::StorageConflict,
        None,
        ProviderRemediation::Retry,
    )
}

fn operation_stale_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::OperationStale,
        None,
        ProviderRemediation::WaitForCurrentOperation,
    )
}

fn persisted_block_error(code: ProviderErrorCode) -> ProviderManagementError {
    let remediation = match code {
        ProviderErrorCode::CredentialProtectionUnavailable => {
            ProviderRemediation::ConfigureCredentialStore
        }
        ProviderErrorCode::OAuthNotConnected | ProviderErrorCode::RemoteRevokeFailed => {
            ProviderRemediation::Reauthorize
        }
        ProviderErrorCode::StorageConflict | ProviderErrorCode::OperationStale => {
            ProviderRemediation::Retry
        }
        _ => ProviderRemediation::ContactSupport,
    };
    ProviderManagementError::new(code, Some(ProviderField::Credential), remediation)
}

fn internal_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        None,
        ProviderRemediation::ContactSupport,
    )
}
