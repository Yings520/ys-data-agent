use std::{num::NonZeroU64, path::PathBuf};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use ys_agent_core::{
    CommandId, DatasourceChange, DatasourceCommit, DatasourceDetail, DatasourceHeader,
    DatasourceProfile, DatasourceReceipt, DatasourceRepository, DatasourceRevision,
    DatasourceRevisionId, DatasourceScope, DatasourceSecretRef, DatasourceSelectionKind,
    DatasourceSnapshot, DeleteDatasourceDisposition, DsError, DsErrorCode, DsRemediation, DsResult,
    OperationId, ProfileId, RevisionState, RunDatasourceBinding, RunId, SecretMutation,
    SecretMutationPhase, SelectionSnapshot, WorkspaceId,
};

use crate::{SqliteRuntimeStore, sqlite::open_connection};

#[derive(Debug, Clone)]
pub struct SqliteDatasourceRepository {
    database: PathBuf,
}

impl SqliteRuntimeStore {
    pub fn datasource_repository(&self) -> SqliteDatasourceRepository {
        SqliteDatasourceRepository {
            database: self.database_path().to_path_buf(),
        }
    }
}

impl SqliteDatasourceRepository {
    async fn with_connection<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> DsResult<T> + Send + 'static,
    ) -> DsResult<T> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            operation(&mut open_connection(&database).map_err(storage)?)
        })
        .await
        .map_err(storage)?
    }
}

fn error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::Conflict => DsRemediation::Refresh,
            DsErrorCode::InUse => DsRemediation::WaitOrCancelRun,
            DsErrorCode::ValidationStale => DsRemediation::Revalidate,
            DsErrorCode::DuplicateName | DsErrorCode::InvalidField => {
                DsRemediation::EditConfiguration
            }
            _ => DsRemediation::Retry,
        },
        operation_id: None,
    }
}

// SQLite/JSON errors may contain local paths and supplied fields. Never cross the port with them.
fn storage(_: impl std::fmt::Display) -> DsError {
    error(DsErrorCode::Storage)
}
fn encode(value: &impl Serialize) -> DsResult<String> {
    serde_json::to_string(value).map_err(storage)
}
fn decode<T: DeserializeOwned>(value: &str) -> DsResult<T> {
    serde_json::from_str(value).map_err(storage)
}
fn integer(value: u64) -> DsResult<i64> {
    i64::try_from(value).map_err(|_| error(DsErrorCode::InvalidField))
}

fn workspace_version(connection: &Connection, workspace: WorkspaceId) -> DsResult<u64> {
    connection
        .query_row(
            "SELECT version FROM datasource_workspaces WHERE workspace_id=?1",
            [workspace.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage)
        .and_then(|v| u64::try_from(v.unwrap_or(0)).map_err(storage))
}

fn profile(
    connection: &Connection,
    workspace: WorkspaceId,
    id: ProfileId,
) -> DsResult<Option<DatasourceProfile>> {
    let json: Option<String> = connection
        .query_row(
            "SELECT profile_json FROM datasource_profiles WHERE workspace_id=?1 AND profile_id=?2",
            params![workspace.to_string(), id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    json.map(|json| decode(&json)).transpose()
}

fn detail(connection: &Connection, id: DatasourceRevisionId) -> DsResult<DatasourceDetail> {
    let profile = profile(connection, id.workspace_id, id.profile_id)?
        .ok_or_else(|| error(DsErrorCode::Conflict))?;
    let (revision, state, evidence): (String, String, Option<String>) = connection
        .query_row(
            "SELECT r.revision_json, s.state_json, v.evidence_json FROM datasource_revisions r
         JOIN datasource_revision_states s USING(workspace_id, profile_id, revision)
         LEFT JOIN datasource_validations v ON v.validation_id=s.validation_id
         WHERE r.workspace_id=?1 AND r.profile_id=?2 AND r.revision=?3",
            params![
                id.workspace_id.to_string(),
                id.profile_id.to_string(),
                integer(id.revision.get())?
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| error(DsErrorCode::Conflict))?;
    Ok(DatasourceDetail {
        schema_version: 1,
        profile,
        revision: decode(&revision)?,
        state: decode(&state)?,
        validation: evidence.map(|s| decode(&s)).transpose()?,
    })
}

fn selection(
    connection: &Connection,
    scope: DatasourceScope,
    kind: &str,
) -> DsResult<(Option<DatasourceRevisionId>, u64)> {
    let owner = if kind == "default" {
        scope.workspace_id.to_string()
    } else {
        scope.session_id.to_string()
    };
    let row: Option<(Option<String>, Option<i64>, i64)> = connection
        .query_row(
            "SELECT profile_id, revision, version FROM datasource_selections
         WHERE workspace_id=?1 AND selection_kind=?2 AND owner_id=?3",
            params![scope.workspace_id.to_string(), kind, owner],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage)?;
    match row {
        None => Ok((None, 0)),
        Some((None, None, version)) => Ok((None, u64::try_from(version).map_err(storage)?)),
        Some((Some(id), Some(revision), version)) => Ok((
            Some(DatasourceRevisionId {
                workspace_id: scope.workspace_id,
                profile_id: id.parse().map_err(storage)?,
                revision: NonZeroU64::new(u64::try_from(revision).map_err(storage)?)
                    .ok_or_else(|| error(DsErrorCode::Storage))?,
            }),
            u64::try_from(version).map_err(storage)?,
        )),
        _ => Err(error(DsErrorCode::Storage)),
    }
}

fn snapshot(connection: &Connection, scope: DatasourceScope) -> DsResult<DatasourceSnapshot> {
    let mut statement = connection.prepare(
        "SELECT profile_json FROM datasource_profiles WHERE workspace_id=?1 AND deleted_at IS NULL ORDER BY name_key, profile_id"
    ).map_err(storage)?;
    let rows = statement
        .query_map([scope.workspace_id.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(storage)?;
    let mut profiles = Vec::new();
    for row in rows {
        let p: DatasourceProfile = decode(&row.map_err(storage)?)?;
        profiles.push(detail(
            connection,
            DatasourceRevisionId {
                workspace_id: p.workspace_id,
                profile_id: p.profile_id,
                revision: p.head_revision,
            },
        )?);
    }
    let (current, selection_version) = selection(connection, scope, "session")?;
    let (workspace_default, default_version) = selection(connection, scope, "default")?;
    let header = current
        .map(|id| {
            let d = detail(connection, id)?;
            Ok(DatasourceHeader {
                name: d.profile.name,
                adapter_id: d.revision.input().adapter_id.clone(),
                revision: id,
                context_digest: ys_agent_core::DatasourceDigest::of(&d.revision.input().context)
                    .map_err(storage)?,
            })
        })
        .transpose()?;
    Ok(DatasourceSnapshot {
        schema_version: 1,
        version: workspace_version(connection, scope.workspace_id)?,
        profiles,
        selection: SelectionSnapshot {
            schema_version: 1,
            scope,
            current,
            workspace_default,
            selection_version,
            default_version,
            header,
        },
    })
}

fn saved_receipt(connection: &Connection, id: CommandId) -> DsResult<Option<DatasourceReceipt>> {
    let json: Option<String> = connection
        .query_row(
            "SELECT receipt_json FROM datasource_command_receipts WHERE command_id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    json.map(|s| decode(&s)).transpose()
}

fn ensure_head(
    connection: &Connection,
    command: &DatasourceCommit,
    id: ProfileId,
) -> DsResult<Option<DatasourceProfile>> {
    let existing = profile(connection, command.write.scope.workspace_id, id)?;
    if existing.as_ref().is_some_and(|p| p.deleted_at.is_some())
        || existing.as_ref().map(|p| p.head_revision) != command.write.expected_head_revision
    {
        return Err(error(DsErrorCode::Conflict));
    }
    Ok(existing)
}

fn ensure_ready(connection: &Connection, id: DatasourceRevisionId) -> DsResult<DatasourceDetail> {
    let d = detail(connection, id)?;
    if !d
        .validation
        .as_ref()
        .is_some_and(|v| d.is_ready(v.inputs()))
    {
        return Err(error(DsErrorCode::ValidationStale));
    }
    if let Some(reference) = d.revision.input().credential {
        let available: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM datasource_credential_generations
             WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3 AND state='available')",
                params![
                    id.workspace_id.to_string(),
                    id.profile_id.to_string(),
                    integer(reference.generation())?
                ],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !available {
            return Err(error(DsErrorCode::CredentialExpired));
        }
    }
    Ok(d)
}

fn generation_state(
    connection: &Connection,
    reference: DatasourceSecretRef,
) -> DsResult<Option<String>> {
    connection.query_row("SELECT state FROM datasource_credential_generations WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3",
        params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?],
        |row| row.get(0)).optional().map_err(storage)
}

fn load_mutation(connection: &Connection, id: OperationId) -> DsResult<Option<SecretMutation>> {
    let json: Option<String> = connection
        .query_row(
            "SELECT mutation_json FROM datasource_secret_journal WHERE mutation_id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    json.map(|s| decode(&s)).transpose()
}

fn store_mutation(connection: &Connection, mutation: &SecretMutation) -> DsResult<()> {
    let phase = match mutation.phase {
        SecretMutationPhase::Prepared => "prepared",
        SecretMutationPhase::VaultWritten => "vault_written",
        SecretMutationPhase::Committed => "committed",
    };
    connection.execute("INSERT INTO datasource_secret_journal(mutation_id,workspace_id,profile_id,phase,mutation_json)
        VALUES (?1,?2,?3,?4,?5) ON CONFLICT(mutation_id) DO UPDATE SET phase=excluded.phase,mutation_json=excluded.mutation_json",
        params![mutation.mutation_id.to_string(),mutation.write.scope.workspace_id.to_string(),mutation.profile_id.to_string(),phase,encode(mutation)?]).map_err(storage)?;
    Ok(())
}

fn journal_transition(
    transaction: &Transaction<'_>,
    command: &DatasourceCommit,
    mutation: &SecretMutation,
) -> DsResult<()> {
    if mutation.schema_version != 1
        || mutation.write != command.write
        || mutation.command_digest != command.command_digest
        || mutation.old == mutation.new
        || mutation.phase == SecretMutationPhase::Committed
        || [mutation.old, mutation.new].into_iter().flatten().any(|r| {
            r.workspace_id() != mutation.write.scope.workspace_id
                || r.profile_id() != mutation.profile_id
        })
    {
        return Err(error(DsErrorCode::InvalidField));
    }
    if let Some(prior) = load_mutation(transaction, mutation.mutation_id)? {
        let mut comparable = mutation.clone();
        comparable.phase = prior.phase;
        if comparable != prior {
            return Err(error(DsErrorCode::Conflict));
        }
        if mutation
            .new
            .map(|r| generation_state(transaction, r))
            .transpose()?
            .flatten()
            .as_deref()
            .is_some_and(|state| matches!(state, "removing" | "removed"))
        {
            return Err(error(DsErrorCode::Conflict));
        }
        if mutation.phase == prior.phase {
            return Ok(());
        }
        if prior.phase != SecretMutationPhase::Prepared
            || mutation.phase != SecretMutationPhase::VaultWritten
        {
            return Err(error(DsErrorCode::Conflict));
        }
        return store_mutation(transaction, mutation);
    }
    if mutation.phase != SecretMutationPhase::Prepared {
        return Err(error(DsErrorCode::Conflict));
    }
    let existing = ensure_head(transaction, command, mutation.profile_id)?;
    let old = existing
        .map(|p| {
            detail(
                transaction,
                DatasourceRevisionId {
                    workspace_id: p.workspace_id,
                    profile_id: p.profile_id,
                    revision: p.head_revision,
                },
            )
        })
        .transpose()?
        .and_then(|d| d.revision.input().credential);
    if old != mutation.old {
        return Err(error(DsErrorCode::Conflict));
    }
    let occupied: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM datasource_secret_journal WHERE workspace_id=?1 AND profile_id=?2)",
        params![command.write.scope.workspace_id.to_string(),mutation.profile_id.to_string()], |row| row.get(0)).map_err(storage)?;
    if occupied {
        return Err(error(DsErrorCode::Conflict));
    }
    if let Some(reference) = mutation.new {
        let maximum: i64 = transaction.query_row("SELECT COALESCE(MAX(generation),0) FROM datasource_credential_generations WHERE workspace_id=?1 AND profile_id=?2",
            params![reference.workspace_id().to_string(), reference.profile_id().to_string()], |row| row.get(0)).map_err(storage)?;
        if integer(reference.generation())? <= maximum {
            return Err(error(DsErrorCode::Conflict));
        }
        transaction.execute("INSERT INTO datasource_credential_generations(workspace_id,profile_id,generation,state) VALUES (?1,?2,?3,'prepared')",
            params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?]).map_err(storage)?;
    }
    store_mutation(transaction, mutation)
}

fn save_revision(
    transaction: &Transaction<'_>,
    command: &DatasourceCommit,
    p: &DatasourceProfile,
    revision: &DatasourceRevision,
    mutation_id: Option<ys_agent_core::OperationId>,
) -> DsResult<()> {
    let existing = ensure_head(transaction, command, p.profile_id)?;
    let expected = existing
        .as_ref()
        .map_or(Some(1), |p| p.head_revision.get().checked_add(1));
    if p.schema_version != 1
        || p.deleted_at.is_some()
        || p.workspace_id != command.write.scope.workspace_id
        || revision.identity()
            != (DatasourceRevisionId {
                workspace_id: p.workspace_id,
                profile_id: p.profile_id,
                revision: p.head_revision,
            })
        || expected != Some(p.head_revision.get())
        || p.source_id != revision.input().source_id
        || existing
            .as_ref()
            .is_some_and(|old| old.source_id.is_some() && old.source_id != p.source_id)
    {
        return Err(error(DsErrorCode::InvalidField));
    }
    let duplicate: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM datasource_profiles
        WHERE workspace_id=?1 AND name_key=?2 AND profile_id<>?3 AND deleted_at IS NULL)",
            params![
                p.workspace_id.to_string(),
                p.name.uniqueness_key(),
                p.profile_id.to_string()
            ],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if duplicate {
        return Err(error(DsErrorCode::DuplicateName));
    }
    // Credential changes are committed only by the recoverable journal protocol.
    let old_secret = existing
        .as_ref()
        .map(|old| {
            detail(
                transaction,
                DatasourceRevisionId {
                    workspace_id: old.workspace_id,
                    profile_id: old.profile_id,
                    revision: old.head_revision,
                },
            )
        })
        .transpose()?
        .and_then(|d| d.revision.input().credential);
    if let Some(id) = mutation_id {
        let mut mutation =
            load_mutation(transaction, id)?.ok_or_else(|| error(DsErrorCode::Conflict))?;
        if mutation.phase != SecretMutationPhase::VaultWritten
            || mutation.write != command.write
            || mutation.command_digest != command.command_digest
            || mutation.profile_id != p.profile_id
            || mutation.old != old_secret
            || mutation.new != revision.input().credential
        {
            return Err(error(DsErrorCode::Conflict));
        }
        if let Some(reference) = mutation.new {
            if generation_state(transaction, reference)?.as_deref() != Some("prepared") {
                return Err(error(DsErrorCode::Conflict));
            }
            transaction.execute("UPDATE datasource_credential_generations SET state='available' WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3",
                params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?]).map_err(storage)?;
        }
        if let Some(reference) = mutation.old {
            transaction.execute("UPDATE datasource_credential_generations SET state='retired' WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3",
                params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?]).map_err(storage)?;
            transaction.execute("UPDATE datasource_revision_states SET state_json=?4,validation_id=NULL
                WHERE workspace_id=?1 AND profile_id=?2 AND revision IN
                (SELECT revision FROM datasource_revisions WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3)",
                params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?,
                    encode(&RevisionState::Invalid(DsErrorCode::CredentialExpired))?]).map_err(storage)?;
        }
        mutation.phase = SecretMutationPhase::Committed;
        store_mutation(transaction, &mutation)?;
    } else {
        let pending: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM datasource_secret_journal WHERE workspace_id=?1 AND profile_id=?2)",
            params![p.workspace_id.to_string(),p.profile_id.to_string()], |row| row.get(0)).map_err(storage)?;
        if pending || old_secret != revision.input().credential {
            return Err(error(DsErrorCode::Conflict));
        }
    }
    transaction.execute("INSERT INTO datasource_profiles(workspace_id,profile_id,name_key,head_revision,profile_json)
        VALUES (?1,?2,?3,?4,?5) ON CONFLICT(workspace_id,profile_id) DO UPDATE SET
        name_key=excluded.name_key,head_revision=excluded.head_revision,profile_json=excluded.profile_json",
        params![p.workspace_id.to_string(), p.profile_id.to_string(), p.name.uniqueness_key(), integer(p.head_revision.get())?, encode(p)?]).map_err(storage)?;
    transaction.execute("INSERT INTO datasource_revisions(workspace_id,profile_id,revision,generation,revision_json) VALUES (?1,?2,?3,?4,?5)",
        params![p.workspace_id.to_string(), p.profile_id.to_string(), integer(p.head_revision.get())?,
            revision.input().credential.map(|r| integer(r.generation())).transpose()?, encode(revision)?]).map_err(storage)?;
    transaction.execute("INSERT INTO datasource_revision_states(workspace_id,profile_id,revision,state_json) VALUES (?1,?2,?3,?4)",
        params![p.workspace_id.to_string(),p.profile_id.to_string(),integer(p.head_revision.get())?,encode(&RevisionState::Draft)?]).map_err(storage)?;
    Ok(())
}

fn set_selection(
    connection: &Connection,
    scope: DatasourceScope,
    kind: &str,
    revision: Option<DatasourceRevisionId>,
) -> DsResult<()> {
    let owner = if kind == "default" {
        scope.workspace_id.to_string()
    } else {
        scope.session_id.to_string()
    };
    connection.execute("INSERT INTO datasource_selections(workspace_id,selection_kind,owner_id,profile_id,revision,version)
        VALUES (?1,?2,?3,?4,?5,1) ON CONFLICT(workspace_id,selection_kind,owner_id)
        DO UPDATE SET profile_id=excluded.profile_id,revision=excluded.revision,version=version+1",
        params![scope.workspace_id.to_string(), kind, owner, revision.map(|r| r.profile_id.to_string()),
            revision.map(|r| integer(r.revision.get())).transpose()?]).map_err(storage)?;
    Ok(())
}

pub(crate) fn insert_run_binding(
    transaction: &Transaction<'_>,
    binding: &RunDatasourceBinding,
) -> DsResult<()> {
    binding
        .validate_supported()
        .map_err(|_| error(DsErrorCode::ConfigIncompatible))?;
    let workspace: String = transaction
        .query_row(
            "SELECT t.workspace_id FROM runs r JOIN tasks t USING(task_id) WHERE r.run_id=?1",
            [binding.run_id().to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if workspace != binding.scope().workspace_id.to_string() {
        return Err(error(DsErrorCode::Conflict));
    }
    let (selected, version) = selection(transaction, binding.scope(), "session")?;
    if selected != Some(binding.revision()) || version != binding.selection_version() {
        return Err(error(DsErrorCode::Conflict));
    }
    let d = ensure_ready(transaction, binding.revision())?;
    if d.validation.as_ref() != Some(binding.evidence())
        || d.revision.input().credential != binding.credential()
    {
        return Err(error(DsErrorCode::ValidationStale));
    }
    transaction.execute("INSERT INTO run_datasource_bindings(run_id,workspace_id,profile_id,revision,generation,binding_json) VALUES (?1,?2,?3,?4,?5,?6)",
        params![binding.run_id().to_string(),workspace,binding.revision().profile_id.to_string(),integer(binding.revision().revision.get())?,
            binding.credential().map(|r| integer(r.generation())).transpose()?,encode(binding)?]).map_err(storage)?;
    Ok(())
}

pub(crate) fn initialize_session_selection(
    transaction: &Transaction<'_>,
    session: &ys_agent_core::Session,
) -> DsResult<()> {
    let scope = DatasourceScope {
        workspace_id: session.workspace_id,
        session_id: session.id,
    };
    if selection(transaction, scope, "session")?.1 == 0 {
        // Copy the exact default once. An explicit unconfigured choice never inherits later.
        let (default, _) = selection(transaction, scope, "default")?;
        set_selection(transaction, scope, "session", default)?;
    }
    Ok(())
}

fn apply_change(transaction: &Transaction<'_>, command: &DatasourceCommit) -> DsResult<()> {
    let workspace = command.write.scope.workspace_id;
    let profile_id = match &command.change {
        DatasourceChange::Validation { revision, .. }
        | DatasourceChange::Selection { revision, .. } => Some(revision.profile_id),
        DatasourceChange::Delete { profile_id, .. } => Some(*profile_id),
        _ => None,
    };
    if let Some(profile_id) = profile_id {
        let pending: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM datasource_secret_journal WHERE workspace_id=?1 AND profile_id=?2)",
            params![workspace.to_string(),profile_id.to_string()], |row| row.get(0)).map_err(storage)?;
        if pending {
            return Err(error(DsErrorCode::Conflict));
        }
    }
    match &command.change {
        DatasourceChange::SaveRevision {
            profile,
            revision,
            mutation_id,
        } => save_revision(transaction, command, profile, revision, *mutation_id),
        DatasourceChange::Validation {
            revision,
            state,
            evidence,
        } => {
            if revision.workspace_id != workspace {
                return Err(error(DsErrorCode::Conflict));
            }
            ensure_head(transaction, command, revision.profile_id)?;
            let mut d = detail(transaction, *revision)?;
            d.state = state.clone();
            d.validation = evidence.clone();
            if (*state == RevisionState::Ready
                && !evidence.as_ref().is_some_and(|e| d.is_ready(e.inputs())))
                || (*state != RevisionState::Ready && evidence.is_some())
            {
                return Err(error(DsErrorCode::ValidationStale));
            }
            if let Some(evidence) = evidence {
                let prior: Option<String> = transaction
                    .query_row(
                        "SELECT evidence_json FROM datasource_validations WHERE validation_id=?1",
                        [evidence.id().to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;
                if let Some(prior) = prior {
                    if decode::<ys_agent_core::ValidationEvidence>(&prior)? != *evidence {
                        return Err(error(DsErrorCode::Conflict));
                    }
                } else {
                    transaction.execute("INSERT INTO datasource_validations(workspace_id,profile_id,revision,validation_id,evidence_json) VALUES (?1,?2,?3,?4,?5)",
                        params![workspace.to_string(),revision.profile_id.to_string(),integer(revision.revision.get())?,evidence.id().to_string(),encode(evidence)?]).map_err(storage)?;
                }
            }
            transaction
                .execute(
                    "UPDATE datasource_revision_states SET state_json=?4,validation_id=?5
                WHERE workspace_id=?1 AND profile_id=?2 AND revision=?3",
                    params![
                        workspace.to_string(),
                        revision.profile_id.to_string(),
                        integer(revision.revision.get())?,
                        encode(state)?,
                        evidence.as_ref().map(|e| e.id().to_string())
                    ],
                )
                .map_err(storage)?;
            Ok(())
        }
        DatasourceChange::Selection { revision, kind } => {
            if revision.workspace_id != workspace {
                return Err(error(DsErrorCode::Conflict));
            }
            ensure_ready(transaction, *revision)?;
            set_selection(
                transaction,
                command.write.scope,
                match kind {
                    DatasourceSelectionKind::Session => "session",
                    DatasourceSelectionKind::WorkspaceDefault => "default",
                },
                Some(*revision),
            )
        }
        DatasourceChange::Delete {
            profile_id,
            disposition,
        } => {
            let mut p = ensure_head(transaction, command, *profile_id)?
                .ok_or_else(|| error(DsErrorCode::Conflict))?;
            let in_use: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM run_datasource_bindings b JOIN runs r USING(run_id)
                WHERE b.workspace_id=?1 AND b.profile_id=?2 AND r.status NOT IN ('Succeeded','Failed','Cancelled'))",
                params![workspace.to_string(),profile_id.to_string()], |row| row.get(0)).map_err(storage)?;
            if in_use {
                return Err(error(DsErrorCode::InUse));
            }
            let replacement = match disposition {
                DeleteDatasourceDisposition::ConfirmUnconfigured => None,
                DeleteDatasourceDisposition::Replacement(id) => {
                    if id.workspace_id != workspace || id.profile_id == *profile_id {
                        return Err(error(DsErrorCode::InvalidField));
                    }
                    ensure_ready(transaction, *id)?;
                    Some(*id)
                }
            };
            transaction.execute("UPDATE datasource_selections SET profile_id=?3,revision=?4,version=version+1 WHERE workspace_id=?1 AND profile_id=?2",
                params![workspace.to_string(), profile_id.to_string(), replacement.map(|r| r.profile_id.to_string()), replacement.map(|r| integer(r.revision.get())).transpose()?]).map_err(storage)?;
            p.deleted_at = Some(chrono::Utc::now());
            transaction.execute("UPDATE datasource_profiles SET deleted_at=?3,profile_json=?4 WHERE workspace_id=?1 AND profile_id=?2",
                params![workspace.to_string(),profile_id.to_string(),p.deleted_at.map(|t| t.to_rfc3339()),encode(&p)?]).map_err(storage)?;
            transaction.execute("UPDATE datasource_credential_generations SET state='retired' WHERE workspace_id=?1 AND profile_id=?2 AND state='available'",
                params![workspace.to_string(),profile_id.to_string()]).map_err(storage)?;
            Ok(())
        }
        DatasourceChange::SecretJournal { mutation } => {
            journal_transition(transaction, command, mutation)
        }
    }
}

#[async_trait]
impl DatasourceRepository for SqliteDatasourceRepository {
    async fn load(&self, scope: DatasourceScope) -> DsResult<DatasourceSnapshot> {
        self.with_connection(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            let result = snapshot(&transaction, scope)?;
            transaction.commit().map_err(storage)?;
            Ok(result)
        })
        .await
    }
    async fn load_revision(&self, id: DatasourceRevisionId) -> DsResult<DatasourceDetail> {
        self.with_connection(move |connection| detail(connection, id))
            .await
    }
    async fn commit(&self, command: DatasourceCommit) -> DsResult<DatasourceReceipt> {
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            if command.schema_version != 1 { return Err(error(DsErrorCode::ConfigIncompatible)); }
            if let Some(receipt) = saved_receipt(&transaction, command.write.command_id)? {
                let prior: String = transaction.query_row("SELECT request_json FROM datasource_command_receipts WHERE command_id=?1",
                    [command.write.command_id.to_string()], |row| row.get(0)).map_err(storage)?;
                if decode::<DatasourceCommit>(&prior)? != command { return Err(error(DsErrorCode::Conflict)); }
                return Ok(receipt);
            }
            let workspace = command.write.scope.workspace_id;
            let version = workspace_version(&transaction, workspace)?;
            if version != command.write.expected_version { return Err(error(DsErrorCode::Conflict)); }
            transaction.execute("INSERT OR IGNORE INTO datasource_workspaces(workspace_id,version) VALUES (?1,0)",
                [workspace.to_string()]).map_err(storage)?;
            apply_change(&transaction, &command)?;
            if matches!(command.change, DatasourceChange::SecretJournal { .. }) {
                // Internal phase transitions are not completed user commands and consume no CAS version.
                let receipt = DatasourceReceipt { schema_version: 1, command_id: command.write.command_id,
                    command_digest: command.command_digest.clone(), committed_version: version,
                    snapshot: snapshot(&transaction, command.write.scope)? };
                transaction.commit().map_err(storage)?;
                return Ok(receipt);
            }
            let committed_version = version.checked_add(1).ok_or_else(|| error(DsErrorCode::Conflict))?;
            transaction.execute("UPDATE datasource_workspaces SET version=?2 WHERE workspace_id=?1",
                params![workspace.to_string(),integer(committed_version)?]).map_err(storage)?;
            let receipt = DatasourceReceipt { schema_version: 1, command_id: command.write.command_id,
                command_digest: command.command_digest.clone(), committed_version, snapshot: snapshot(&transaction, command.write.scope)? };
            transaction.execute("INSERT INTO datasource_command_receipts(command_id,request_json,receipt_json) VALUES (?1,?2,?3)",
                params![command.write.command_id.to_string(),encode(&command)?,encode(&receipt)?]).map_err(storage)?;
            transaction.commit().map_err(storage)?;
            Ok(receipt)
        }).await
    }
    async fn receipt(&self, command: CommandId) -> DsResult<Option<DatasourceReceipt>> {
        self.with_connection(move |connection| saved_receipt(connection, command))
            .await
    }
    async fn pending_secret_mutations(
        &self,
        workspace: WorkspaceId,
    ) -> DsResult<Vec<SecretMutation>> {
        self.with_connection(move |connection| {
            let mut statement = connection.prepare("SELECT mutation_json FROM datasource_secret_journal WHERE workspace_id=?1 ORDER BY mutation_id").map_err(storage)?;
            let rows = statement.query_map([workspace.to_string()], |row| row.get::<_,String>(0)).map_err(storage)?;
            rows.map(|row| decode(&row.map_err(storage)?)).collect()
        }).await
    }
    async fn load_run_binding(&self, run: RunId) -> DsResult<RunDatasourceBinding> {
        self.with_connection(move |connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT binding_json FROM run_datasource_bindings WHERE run_id=?1",
                    [run.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            decode(&json.ok_or_else(|| error(DsErrorCode::ConfigIncompatible))?)
        })
        .await
    }
    async fn claim_secret_cleanup(&self, reference: DatasourceSecretRef) -> DsResult<()> {
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM run_datasource_bindings b JOIN runs r USING(run_id)
                WHERE b.workspace_id=?1 AND b.profile_id=?2 AND b.generation=?3 AND r.status NOT IN ('Succeeded','Failed','Cancelled'))",
                params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?], |row| row.get(0)).map_err(storage)?;
            if active { return Err(error(DsErrorCode::InUse)); }
            match generation_state(&transaction, reference)?.as_deref() {
                Some("prepared" | "retired" | "removing" | "removed") => {},
                _ => return Err(error(DsErrorCode::Conflict)),
            }
            transaction.execute("UPDATE datasource_credential_generations SET state='removing' WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3 AND state<>'removed'",
                params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?]).map_err(storage)?;
            transaction.commit().map_err(storage)
        }).await
    }
    async fn finish_secret_cleanup(&self, reference: DatasourceSecretRef) -> DsResult<()> {
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            if !matches!(generation_state(&transaction, reference)?.as_deref(), Some("removing" | "removed")) {
                return Err(error(DsErrorCode::Conflict));
            }
            transaction.execute("UPDATE datasource_credential_generations SET state='removed' WHERE workspace_id=?1 AND profile_id=?2 AND generation=?3",
                params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?]).map_err(storage)?;
            transaction.commit().map_err(storage)
        }).await
    }
    async fn obsolete_secret_generations(
        &self,
        workspace: WorkspaceId,
    ) -> DsResult<Vec<DatasourceSecretRef>> {
        self.with_connection(move |connection| {
            let mut statement = connection.prepare("SELECT profile_id,generation FROM datasource_credential_generations
                WHERE workspace_id=?1 AND state IN ('retired','removing') ORDER BY profile_id,generation").map_err(storage)?;
            let rows = statement.query_map([workspace.to_string()], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).map_err(storage)?;
            rows.map(|row| { let (profile,generation) = row.map_err(storage)?;
                DatasourceSecretRef::new(workspace,profile.parse().map_err(storage)?,u64::try_from(generation).map_err(storage)?).map_err(storage)
            }).collect()
        }).await
    }
    async fn finish_secret_mutation(&self, id: OperationId) -> DsResult<()> {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            if let Some(mutation) = load_mutation(&transaction, id)? {
                let cleanup = if mutation.phase == SecretMutationPhase::Committed {
                    mutation.old
                } else {
                    mutation.new
                };
                if let Some(reference) = cleanup {
                    let state = generation_state(&transaction, reference)?;
                    // A durable nonterminal lease is normal retention, not failed cleanup.
                    let retained: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM run_datasource_bindings b JOIN runs r USING(run_id)
                        WHERE b.workspace_id=?1 AND b.profile_id=?2 AND b.generation=?3 AND r.status NOT IN ('Succeeded','Failed','Cancelled'))",
                        params![reference.workspace_id().to_string(),reference.profile_id().to_string(),integer(reference.generation())?], |row| row.get(0)).map_err(storage)?;
                    if state.as_deref() != Some("removed")
                        && !(mutation.phase == SecretMutationPhase::Committed && state.as_deref() == Some("retired") && retained) {
                        return Err(error(DsErrorCode::Conflict));
                    }
                }
                transaction
                    .execute(
                        "DELETE FROM datasource_secret_journal WHERE mutation_id=?1",
                        [id.to_string()],
                    )
                    .map_err(storage)?;
            }
            transaction.commit().map_err(storage)
        })
        .await
    }
}
