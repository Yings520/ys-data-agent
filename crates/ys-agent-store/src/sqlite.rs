use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use ys_agent_core::{
    ArtifactId, ArtifactMetadata, CommandId, CommandReceipt, CoreError, CoreResult, EventActor,
    EventEnvelope, EventId, PendingRunEvent, RunEventKind, RunId, RunSnapshot, RuntimeCommandBatch,
    RuntimeStore, Session, SessionId, Task, TaskId, VersionedRunEvent, WorkspaceId,
};

const MIGRATION_0001: &str = include_str!("../migrations/0001_runtime.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_provider_management.sql");

#[derive(Debug, Clone)]
pub struct SqliteRuntimeStore {
    database: PathBuf,
}

impl SqliteRuntimeStore {
    pub async fn open(database: impl AsRef<Path>) -> CoreResult<Self> {
        let store = Self {
            database: database.as_ref().to_path_buf(),
        };
        store.with_connection(apply_migrations).await?;
        Ok(store)
    }

    pub fn database_path(&self) -> &Path {
        &self.database
    }

    pub async fn run_count(&self) -> CoreResult<u64> {
        self.with_connection(|connection| {
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
                .map_err(storage_error)?;
            u64::try_from(count).map_err(|error| CoreError::Storage {
                message: error.to_string(),
            })
        })
        .await
    }

    async fn with_connection<T, F>(&self, operation: F) -> CoreResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> CoreResult<T> + Send + 'static,
    {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = open_connection(&database)?;
            operation(&mut connection)
        })
        .await
        .map_err(|error| CoreError::Storage {
            message: format!("SQLite worker task failed: {error}"),
        })?
    }
}

fn open_connection(database: &Path) -> CoreResult<Connection> {
    if let Some(parent) = database.parent() {
        std::fs::create_dir_all(parent).map_err(storage_error)?;
    }

    let connection = Connection::open(database).map_err(storage_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(storage_error)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(storage_error)?;
    Ok(connection)
}

fn apply_migrations(connection: &mut Connection) -> CoreResult<()> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )
        .map_err(storage_error)?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let runtime_applied = migration_is_applied(&transaction, 1)?;
    if !runtime_applied {
        transaction
            .execute_batch(MIGRATION_0001)
            .map_err(storage_error)?;
        record_migration(&transaction, 1)?;
    }

    if !migration_is_applied(&transaction, 2)? {
        transaction
            .execute_batch(MIGRATION_0002)
            .map_err(storage_error)?;
        record_migration(&transaction, 2)?;
    }
    transaction.commit().map_err(storage_error)
}

fn migration_is_applied(transaction: &Transaction<'_>, version: i64) -> CoreResult<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
            [version],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

fn record_migration(transaction: &Transaction<'_>, version: i64) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![version, Utc::now().to_rfc3339()],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn storage_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage {
        message: error.to_string(),
    }
}

fn to_json<T: Serialize>(value: &T) -> CoreResult<String> {
    serde_json::to_string(value).map_err(storage_error)
}

fn from_json<T: DeserializeOwned>(value: &str) -> CoreResult<T> {
    serde_json::from_str(value).map_err(storage_error)
}

fn serialized_name<T: Serialize>(value: &T) -> CoreResult<String> {
    let value = serde_json::to_value(value).map_err(storage_error)?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Storage {
            message: "expected a string-like enum serialization".to_owned(),
        })
}

fn load_json<T: DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    id: String,
    entity: &'static str,
) -> CoreResult<T> {
    let payload: Option<String> = connection
        .query_row(sql, [id.clone()], |row| row.get(0))
        .optional()
        .map_err(storage_error)?;
    let payload = payload.ok_or(CoreError::NotFound { entity, id })?;
    from_json(&payload)
}

fn insert_session(transaction: &Transaction<'_>, session: &Session) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO sessions(
                session_id, workspace_id, principal_id, payload_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.id.to_string(),
                session.workspace_id.to_string(),
                session.principal_id.to_string(),
                to_json(&session)?,
                session.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn insert_task(transaction: &Transaction<'_>, task: &Task) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO tasks(
                task_id, workspace_id, status, payload_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                task.id.to_string(),
                task.workspace_id.to_string(),
                serialized_name(&task.status)?,
                to_json(task)?,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn insert_run(transaction: &Transaction<'_>, snapshot: &RunSnapshot) -> CoreResult<()> {
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "INSERT INTO runs(
                run_id, task_id, status, version, snapshot_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                snapshot.run_id.to_string(),
                snapshot.task_id.to_string(),
                serialized_name(&snapshot.status)?,
                i64::try_from(snapshot.version).map_err(storage_error)?,
                to_json(snapshot)?,
                now,
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn insert_artifact(transaction: &Transaction<'_>, metada: &ArtifactMetadata) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO artifacts(
                artifact_id, workspace_id, task_id, run_id,content_hash,
                metadata_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                metada.id.to_string(),
                metada.workspace_id.to_string(),
                metada.task_id.to_string(),
                metada.run_id.to_string(),
                metada.content_hash,
                to_json(metada)?,
                metada.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn event_type(event: &VersionedRunEvent) -> CoreResult<String> {
    let value = serde_json::to_value(&event.kind).map_err(storage_error)?;
    value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Storage {
            message: "event type tag is missing".to_owned(),
        })
}

fn projection_event(snapshot: &RunSnapshot) -> PendingRunEvent {
    PendingRunEvent {
        actor: EventActor::System,
        kind: RunEventKind::RunStateProjected {
            snapshot: Box::new(snapshot.clone()),
        },
    }
}

fn insert_events(
    transaction: &Transaction<'_>,
    run_id: &RunId,
    events: Vec<PendingRunEvent>,
) -> CoreResult<u64> {
    let snapshot: RunSnapshot = load_json(
        transaction,
        "SELECT snapshot_json FROM runs WHERE run_id = ?1",
        run_id.to_string(),
        "run",
    )?;

    let task: Task = load_json(
        transaction,
        "SELECT payload_json FROM tasks WHERE task_id = ?1",
        snapshot.task_id.to_string(),
        "task",
    )?;

    let last_sequence: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM run_events WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;

    let mut sequence = u64::try_from(last_sequence).map_err(storage_error)?;
    for pending in events {
        sequence = sequence.checked_add(1).ok_or_else(|| CoreError::Storage {
            message: format!("event sequence overflow for run {run_id}"),
        })?;
        let occurred_at = Utc::now();
        let event = VersionedRunEvent::v1(pending.kind);
        let envelope = EventEnvelope {
            event_id: EventId::new(),
            workspace_id: task.workspace_id,
            task_id: task.id,
            run_id: *run_id,
            sequence,
            occurred_at,
            actor: pending.actor,
            event,
        };
        transaction
            .execute(
                "INSERT INTO run_events(
                    event_id, run_id, sequence, event_type, payload_json, occurred_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    envelope.event_id.to_string(),
                    run_id.to_string(),
                    i64::try_from(sequence).map_err(storage_error)?,
                    event_type(&envelope.event)?,
                    to_json(&envelope)?,
                    occurred_at.to_rfc3339(),
                ],
            )
            .map_err(storage_error)?;
    }

    Ok(sequence)
}

fn update_snapshot(
    transaction: &Transaction<'_>,
    expected_version: u64,
    snapshot: &RunSnapshot,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE runs
             SET status = ?1, version = ?2, snapshot_json = ?3, updated_at = ?4
             WHERE run_id = ?5 AND version = ?6",
            params![
                serialized_name(&snapshot.status)?,
                i64::try_from(snapshot.version).map_err(storage_error)?,
                to_json(snapshot)?,
                Utc::now().to_rfc3339(),
                snapshot.run_id.to_string(),
                i64::try_from(expected_version).map_err(storage_error)?,
            ],
        )
        .map_err(storage_error)?;

    if changed != 1 {
        return Err(CoreError::ConcurrencyConflict {
            run_id: snapshot.run_id.to_string(),
        });
    }
    Ok(())
}

fn current_version(transaction: &Transaction<'_>, run_id: &RunId) -> CoreResult<u64> {
    let version: Option<i64> = transaction
        .query_row(
            "SELECT version FROM runs WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    let version = version.ok_or_else(|| CoreError::NotFound {
        entity: "run",
        id: run_id.to_string(),
    })?;
    u64::try_from(version).map_err(storage_error)
}

fn append_in_transaction(
    transaction: &Transaction<'_>,
    run_id: &RunId,
    expected_version: u64,
    artifacts: Vec<ArtifactMetadata>,
    mut events: Vec<PendingRunEvent>,
    snapshot: &RunSnapshot,
) -> CoreResult<()> {
    if snapshot.run_id != *run_id || snapshot.version != expected_version + 1 {
        return Err(CoreError::Validation {
            code: "invalid_snapshot_version",
            message: "snapshot must target the run and advance its version by one".to_owned(),
        });
    }

    if current_version(transaction, run_id)? != expected_version {
        return Err(CoreError::ConcurrencyConflict {
            run_id: run_id.to_string(),
        });
    }

    for artifact in &artifacts {
        if artifact.run_id != *run_id || artifact.task_id != snapshot.task_id {
            return Err(CoreError::validation(
                "artifact_run_mismatch",
                "atomic append Artifact belongs to another Run or Task",
            ));
        }
        insert_artifact(transaction, artifact)?;
    }
    events.push(projection_event(snapshot));
    insert_events(transaction, run_id, events)?;
    update_snapshot(transaction, expected_version, snapshot)
}

fn existing_receipt(
    connection: &Connection,
    command_id: &CommandId,
) -> CoreResult<Option<(String, CommandReceipt)>> {
    let stored: Option<(String, String)> = connection
        .query_row(
            "SELECT command_fingerprint, payload_json
             FROM command_receipts WHERE command_id = ?1",
            [command_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage_error)?;
    stored
        .map(|(fingerprint, payload)| Ok((fingerprint, from_json(&payload)?)))
        .transpose()
}

fn commit_command_on_connection(
    connection: &mut Connection,
    batch: RuntimeCommandBatch,
) -> CoreResult<CommandReceipt> {
    if batch.receipt.command_id != batch.command_id
        || batch.receipt.command_fingerprint != batch.command_fingerprint
    {
        return Err(CoreError::Validation {
            code: "invalid_command_batch",
            message: "batch identity must match its receipt".to_owned(),
        });
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;

    if let Some((fingerprint, receipt)) = existing_receipt(&transaction, &batch.command_id)? {
        if fingerprint == batch.command_fingerprint {
            return Ok(receipt);
        }
        return Err(CoreError::IdempotencyConflict {
            command_id: batch.command_id.to_string(),
        });
    }

    if batch.create_run.is_some() && batch.snapshot_update.is_some() {
        return Err(CoreError::Validation {
            code: "ambiguous_snapshot_mutation",
            message: "one command cannot create and update a run snapshot".to_owned(),
        });
    }
    if batch.create_run.is_some() && !batch.pending_events.is_empty() {
        return Err(CoreError::Validation {
            code: "run_creation_events_outside_command",
            message: "Run creation lifecycle events must be carried by CreateRunCommand".to_owned(),
        });
    }

    if let Some(session) = &batch.new_session {
        insert_session(&transaction, session)?;
    }
    if let Some(task) = &batch.new_task {
        insert_task(&transaction, task)?;
    }
    if let Some(command) = &batch.create_run {
        insert_run(&transaction, command.snapshot())?;
    }
    if let Some(metadata) = &batch.new_artifact {
        insert_artifact(&transaction, metadata)?;
    }

    match (&batch.create_run, &batch.snapshot_update) {
        (Some(command), None) => {
            let snapshot = command.snapshot();
            let mut events = command.initial_events().to_vec();
            events.push(projection_event(snapshot));
            insert_events(&transaction, &snapshot.run_id, events)?;
        }
        (None, Some(snapshot)) => {
            let expected_version =
                snapshot
                    .version
                    .checked_sub(1)
                    .ok_or_else(|| CoreError::Validation {
                        code: "invalid_snapshot_version",
                        message: "updated snapshots must have a positive version".to_owned(),
                    })?;
            append_in_transaction(
                &transaction,
                &snapshot.run_id,
                expected_version,
                vec![],
                batch.pending_events,
                snapshot,
            )?;
        }
        (None, None) if !batch.pending_events.is_empty() => {
            return Err(CoreError::Validation {
                code: "events_without_run",
                message: "pending events need a new or updated run snapshot".to_owned(),
            });
        }
        (None, None) => {}
        (Some(_), Some(_)) => unreachable!("validated above"),
    }

    transaction
        .execute(
            "INSERT INTO command_receipts(
                command_id, command_fingerprint, result_kind, payload_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                batch.command_id.to_string(),
                batch.command_fingerprint,
                serialized_name(&batch.receipt.result_kind)?,
                to_json(&batch.receipt)?,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_error)?;
    transaction.commit().map_err(storage_error)?;
    Ok(batch.receipt)
}

#[async_trait]
impl RuntimeStore for SqliteRuntimeStore {
    async fn load_command(&self, command_id: &CommandId) -> CoreResult<Option<CommandReceipt>> {
        let command_id = *command_id;
        self.with_connection(move |connection| {
            existing_receipt(connection, &command_id)
                .map(|stored| stored.map(|(_, receipt)| receipt))
        })
        .await
    }

    async fn commit_command(&self, batch: RuntimeCommandBatch) -> CoreResult<CommandReceipt> {
        self.with_connection(move |connection| commit_command_on_connection(connection, batch))
            .await
    }

    async fn load_session(&self, session_id: &SessionId) -> CoreResult<Session> {
        let session_id = *session_id;
        self.with_connection(move |connection| {
            load_json(
                connection,
                "SELECT payload_json FROM sessions WHERE session_id = ?1",
                session_id.to_string(),
                "session",
            )
        })
        .await
    }

    async fn load_task(&self, task_id: &TaskId) -> CoreResult<Task> {
        let task_id = *task_id;
        self.with_connection(move |connection| {
            load_json(
                connection,
                "SELECT payload_json FROM tasks WHERE task_id = ?1",
                task_id.to_string(),
                "task",
            )
        })
        .await
    }

    async fn load_run(&self, run_id: &RunId) -> CoreResult<RunSnapshot> {
        let run_id = *run_id;
        self.with_connection(move |connection| {
            load_json(
                connection,
                "SELECT snapshot_json FROM runs WHERE run_id = ?1",
                run_id.to_string(),
                "run",
            )
        })
        .await
    }

    async fn list_runs_for_task(&self, task_id: &TaskId) -> CoreResult<Vec<RunSnapshot>> {
        let task_id = *task_id;
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT snapshot_json FROM runs
                     WHERE task_id = ?1 ORDER BY created_at, run_id",
                )
                .map_err(storage_error)?;
            let rows = statement
                .query_map([task_id.to_string()], |row| row.get::<_, String>(0))
                .map_err(storage_error)?;
            let mut snapshots = Vec::new();
            for row in rows {
                snapshots.push(from_json(&row.map_err(storage_error)?)?);
            }
            Ok(snapshots)
        })
        .await
    }

    async fn load_artifact(&self, artifact_id: &ArtifactId) -> CoreResult<ArtifactMetadata> {
        let artifact_id = *artifact_id;
        self.with_connection(move |connection| {
            load_json(
                connection,
                "SELECT metadata_json FROM artifacts WHERE artifact_id = ?1",
                artifact_id.to_string(),
                "artifact",
            )
        })
        .await
    }

    async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CoreResult<Vec<Task>> {
        let workspace_id = *workspace_id;
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT payload_json FROM tasks
                        WHERE workspace_id = ?1 ORDER BY created_at, task_id",
                )
                .map_err(storage_error)?;
            let rows = statement
                .query_map([workspace_id.to_string()], |row| row.get::<_, String>(0))
                .map_err(storage_error)?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(from_json(&row.map_err(storage_error)?)?);
            }
            Ok(tasks)
        })
        .await
    }

    async fn load_events(
        &self,
        run_id: &RunId,
        after_sequence: u64,
    ) -> CoreResult<Vec<EventEnvelope>> {
        let run_id = *run_id;
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT sequence, payload_json FROM run_events
                        WHERE run_id = ?1 AND sequence > ?2 ORDER BY sequence",
                )
                .map_err(storage_error)?;
            let rows = statement
                .query_map(
                    params![
                        run_id.to_string(),
                        i64::try_from(after_sequence).map_err(storage_error)?
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(storage_error)?;
            let mut events = Vec::new();
            for row in rows {
                let (stored_sequence, payload) = row.map_err(storage_error)?;
                let stored_sequence = u64::try_from(stored_sequence).map_err(storage_error)?;
                let event: EventEnvelope =
                    from_json(&payload).map_err(|error| CoreError::CorruptRunHistory {
                        run_id: run_id.to_string(),
                        reason: format!("invalid Event envelope: {error}"),
                    })?;
                if event.sequence != stored_sequence {
                    return Err(CoreError::CorruptRunHistory {
                        run_id: run_id.to_string(),
                        reason: format!(
                            "run_events sequence {stored_sequence} disagrees with Event payload sequence {}",
                            event.sequence
                        ),
                    });
                }
                events.push(event);
            }
            Ok(events)
        })
        .await
    }

    async fn replace_snapshot_cache(&self, snapshot: &RunSnapshot) -> CoreResult<()> {
        let snapshot = snapshot.clone();
        self.with_connection(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE runs
                     SET status = ?1, version = ?2, snapshot_json = ?3, updated_at = ?4
                     WHERE run_id = ?5",
                    params![
                        serialized_name(&snapshot.status)?,
                        i64::try_from(snapshot.version).map_err(storage_error)?,
                        to_json(&snapshot)?,
                        Utc::now().to_rfc3339(),
                        snapshot.run_id.to_string(),
                    ],
                )
                .map_err(storage_error)?;
            if changed != 1 {
                return Err(CoreError::NotFound {
                    entity: "run",
                    id: snapshot.run_id.to_string(),
                });
            }
            Ok(())
        })
        .await
    }

    async fn append(
        &self,
        run_id: &RunId,
        expected_version: u64,
        artifacts: Vec<ArtifactMetadata>,
        events: Vec<PendingRunEvent>,
        snapshot: &RunSnapshot,
    ) -> CoreResult<()> {
        let run_id = *run_id;
        let snapshot = snapshot.clone();
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            append_in_transaction(
                &transaction,
                &run_id,
                expected_version,
                artifacts,
                events,
                &snapshot,
            )?;
            transaction.commit().map_err(storage_error)
        })
        .await
    }
}
