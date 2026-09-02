use std::{path::PathBuf, str::FromStr};

use chrono::Utc;
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use ys_agent_core::{
    ActivateProfileRequest, ActiveProviderSnapshot, CredentialGeneration, CredentialKind,
    CredentialViewStatus, PersistedCompatibilityEvidence, PersistedProfileRevision, ProfileId,
    ProfileRevision, ProfileState, ProfileSummary, ProviderErrorCode, ProviderField, ProviderId,
    ProviderManagementError, ProviderModelId, ProviderParameters, ProviderRemediation,
    ProviderResult, RevisionPrecondition, SaveProfileRevision, ValidationCommit,
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

/// SQLite implementation of the profile/revision subset of the Provider persistence boundary.
/// Credential mutation journaling and Run-binding persistence are implemented by their dedicated
/// follow-on tasks; this repository only reads already persisted credential metadata.
#[derive(Debug, Clone)]
pub struct SqliteProviderRepository {
    database: PathBuf,
}

impl SqliteRuntimeStore {
    pub fn provider_repository(&self) -> SqliteProviderRepository {
        SqliteProviderRepository {
            database: self.database_path().to_path_buf(),
        }
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
    if matches!(error, rusqlite::Error::SqliteFailure(ref cause, _) if cause.code == ErrorCode::ConstraintViolation)
    {
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

fn internal_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        None,
        ProviderRemediation::ContactSupport,
    )
}
