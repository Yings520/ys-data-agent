use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, params_from_iter, types::Value as SqliteValue};
use ys_agent_core::{
    CapabilityDescriptor, CatalogReader, CellValue, CoreError, CoreResult, FreshnessObservation,
    FreshnessReader, ObservedColumn, ObservedRelation, ObservedSchema, QueryCostEstimate,
    QueryParameter, QueryPreflight, QueryPreflightDecision, QueryPreflightReader, QueryRequest,
    QueryResult, SchemaKnowledgeKind, SourceId, SqlQueryExecutor,
};
use ys_agent_core::{
    ConnectorOpenInput, DatabaseContext, DatasourceGovernanceContext, DsError, DsErrorCode,
    DsRemediation, DsResult, FieldId, FieldValue, ManagedConnector, ProbeEvidence, QueryBudget,
};

use super::result_policy::{
    DecodedQueryResult, GovernedQueryResult, RestrictedResultContext, ResultPolicy,
};
use super::sql_policy::SqlReadOnlyPolicy;

#[derive(Debug, Clone)]
pub struct SqliteConnectorConfig {
    pub source_id: SourceId,
    pub database_path: PathBuf,
    pub max_concurrency: usize,
    pub freshness_columns: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct SqliteConnector {
    config: SqliteConnectorConfig,
    sql_policy: SqlReadOnlyPolicy,
    result_policy: ResultPolicy,
    managed: Option<Arc<ManagedSqliteState>>,
}

impl SqliteConnector {
    pub fn new(
        config: SqliteConnectorConfig,
        sql_policy: SqlReadOnlyPolicy,
        result_policy: ResultPolicy,
    ) -> Self {
        Self {
            config,
            sql_policy,
            result_policy,
            managed: None,
        }
    }

    pub fn capabilities(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            source_id: self.config.source_id.clone(),
            dialect: "sqlite".to_owned(),
            catalog_reader: true,
            sql_query_executor: true,
            freshness_reader: true,
            supports_explain: false,
            supports_read_only_tx: false,
            preflight_reader: true,
            read_only_mechanism: Some(ys_agent_core::ReadOnlyMechanism::FileReadOnly),
            max_concurrency: self.config.max_concurrency,
        }
    }

    pub async fn execute_governed(
        &self,
        mut request: QueryRequest,
        restricted_context: Option<&RestrictedResultContext>,
    ) -> CoreResult<GovernedQueryResult> {
        validate_request_source(&self.config.source_id, &request)?;
        if let Some(state) = &self.managed {
            state.validate_request(&request)?;
            request.budget = limit_budget(&request.budget, &state.governance.budget);
        }
        if request.sql.len() > request.budget.max_sql_bytes {
            return Err(core_failure("sql_too_large"));
        }
        let policy_decision = self.sql_policy.evaluate(&request.sql, &request.scope);
        policy_decision.ensure_allowed()?;

        let sql = request.sql.clone();
        let parameters = request.parameters.clone();
        let budget = request.budget.clone();
        let scope = request.scope.clone();
        let managed = self.managed.is_some();
        let decoded = self
            .on_connection(&request.budget, move |connection| {
                if managed {
                    install_query_authorizer(connection, scope)?;
                }
                execute_read(
                    connection,
                    &sql,
                    parameters,
                    budget.max_rows,
                    budget.max_result_bytes,
                )
            })
            .await?;

        self.result_policy.apply(
            &request.source_id,
            &policy_decision.referenced_relations,
            &policy_decision.referenced_columns,
            decoded,
            request.budget.max_result_bytes,
            restricted_context,
        )
    }
}

fn open_read_only(path: &Path) -> CoreResult<Connection> {
    open_read_only_flags(path, false)
}

fn open_read_only_flags(path: &Path, nofollow: bool) -> CoreResult<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | if nofollow {
                OpenFlags::SQLITE_OPEN_NOFOLLOW
            } else {
                OpenFlags::empty()
            },
    )
    .map_err(|error| storage_error("open SQLite database", error))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| storage_error("set SQLite query_only", error))?;
    Ok(connection)
}

fn execute_read(
    connection: &Connection,
    sql: &str,
    parameters: Vec<QueryParameter>,
    max_rows: usize,
    max_result_bytes: usize,
) -> CoreResult<DecodedQueryResult> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| storage_error("prepare SQLite query", error))?;
    let columns = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let column_count = columns.len();
    let bound = parameters
        .into_iter()
        .map(sqlite_parameter)
        .collect::<CoreResult<Vec<_>>>()?;
    let mut cursor = statement
        .query(params_from_iter(bound.iter()))
        .map_err(|error| storage_error("execute SQLite query", error))?;
    let mut rows = Vec::new();
    let mut serialized_bytes = 0usize;
    let mut truncated = false;

    while let Some(row) = cursor
        .next()
        .map_err(|error| storage_error("read SQLite row", error))?
    {
        if rows.len() == max_rows {
            truncated = true;
            break;
        }

        let mut values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            let value = row
                .get_ref(index)
                .map_err(|error| storage_error("decode SQLite cell", error))?;
            values.push(decode_sqlite_value(value));
        }
        let row_bytes = serde_json::to_vec(&values)
            .map_err(|error| {
                CoreError::validation("result_serialization_failed", error.to_string())
            })?
            .len();
        if serialized_bytes.saturating_add(row_bytes) > max_result_bytes {
            truncated = true;
            break;
        }
        serialized_bytes += row_bytes;
        rows.push(values);
    }

    Ok(DecodedQueryResult {
        columns,
        rows,
        truncated,
        remote_query_id: None,
        warning_codes: Vec::new(),
    })
}

fn sqlite_parameter(parameter: QueryParameter) -> CoreResult<SqliteValue> {
    match parameter {
        QueryParameter::Timestamp(value) => Ok(SqliteValue::Text(value.to_rfc3339())),
        QueryParameter::Text(value) => Ok(SqliteValue::Text(value)),
        QueryParameter::Integer(value) => Ok(SqliteValue::Integer(value)),
        QueryParameter::Real(value) => Ok(SqliteValue::Real(value)),
        QueryParameter::Boolean(value) => Ok(SqliteValue::Integer(if value { 1 } else { 0 })),
    }
}

fn decode_sqlite_value(value: ValueRef<'_>) -> CellValue {
    match value {
        ValueRef::Null => CellValue::Null,
        ValueRef::Integer(value) => CellValue::Integer(value),
        ValueRef::Real(value) => CellValue::Real(value),
        ValueRef::Text(bytes) => CellValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => CellValue::BlobSummary { bytes: bytes.len() },
    }
}

fn inspect_catalog(
    connection: &Connection,
    source_id: &SourceId,
    result_policy: &ResultPolicy,
) -> CoreResult<ObservedSchema> {
    let scope = result_policy.allowed_scope(ys_agent_core::WorkspaceId::new(), source_id)?;
    let mut relations = Vec::new();

    for relation in scope.relations.keys() {
        if relation.contains('.') {
            return Err(CoreError::validation(
                "sqlite_qualified_relation",
                format!("SQLite relation {relation} must be unqualified"),
            ));
        }
        let pragma = format!("PRAGMA table_info({})", quote_identifier(relation));
        let mut statement = connection
            .prepare(&pragma)
            .map_err(|error| storage_error("prepare SQLite catalog query", error))?;
        let columns = statement
            .query_map([], |row| {
                let name = row.get::<_, String>(1)?;
                let declared_not_null = row.get::<_, i64>(3)? != 0;
                let primary_key_position = row.get::<_, i64>(5)?;
                Ok((
                    name,
                    row.get::<_, String>(2)?,
                    !declared_not_null && primary_key_position == 0,
                    (primary_key_position > 0).then_some(primary_key_position as u32),
                ))
            })
            .map_err(|error| storage_error("read SQLite catalog", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage_error("decode SQLite catalog", error))?;

        if columns.is_empty() {
            return Err(CoreError::validation(
                "configured_relation_missing",
                format!("configured SQLite relation {relation} does not exist"),
            ));
        }

        relations.push(ObservedRelation {
            name: relation.clone(),
            columns: columns
                .into_iter()
                .map(
                    |(name, data_type, nullable, primary_key_position)| ObservedColumn {
                        sensitivity: result_policy.column_sensitivity(source_id, relation, &name),
                        name,
                        data_type,
                        nullable,
                        primary_key_position,
                    },
                )
                .collect(),
        });
    }

    relations.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(ObservedSchema {
        source_id: source_id.clone(),
        kind: SchemaKnowledgeKind::Observed,
        relations,
        observed_at: Utc::now(),
    })
}

fn read_sqlite_freshness(
    connection: &Connection,
    source_id: &SourceId,
    relation: &str,
    column: &str,
) -> CoreResult<FreshnessObservation> {
    let sql = format!(
        "SELECT MAX({}) FROM {}",
        quote_identifier(column),
        quote_identifier(relation)
    );
    let value = connection
        .query_row(&sql, [], |row| row.get::<_, Option<String>>(0))
        .map_err(|error| storage_error("read SQLite freshness", error))?;
    let data_as_of = value
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|error| CoreError::validation("invalid_freshness_value", error.to_string()))?
        .map(|value| value.with_timezone(&Utc));
    let observed_at = Utc::now();
    let lag_seconds = data_as_of.map(|value| {
        observed_at
            .signed_duration_since(value)
            .num_seconds()
            .max(0) as u64
    });

    Ok(FreshnessObservation {
        source_id: source_id.clone(),
        relation: relation.to_owned(),
        observed_at,
        data_as_of,
        lag_seconds,
    })
}

#[async_trait]
impl QueryPreflightReader for SqliteConnector {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        validate_request_source(&self.config.source_id, request)?;
        if let Some(state) = &self.managed {
            state.validate_request(request)?;
        }
        let policy = self.sql_policy.evaluate(&request.sql, &request.scope);
        Ok(QueryPreflight {
            sql: request.sql.clone(),
            decision: match policy.disposition {
                super::sql_policy::SqlPolicyDisposition::Allowed => QueryPreflightDecision::Allowed,
                super::sql_policy::SqlPolicyDisposition::Rejected => {
                    QueryPreflightDecision::Rejected
                }
            },
            cost: QueryCostEstimate {
                estimated_cost_units: None,
                scanned_bytes: None,
                estimator_version: None,
            },
            reason_codes: policy
                .reasons
                .into_iter()
                .map(|reason| reason.code)
                .collect(),
            warnings: Vec::new(),
        })
    }
}

#[async_trait]
impl CatalogReader for SqliteConnector {
    async fn observe_schema(&self, source_id: &SourceId) -> CoreResult<ObservedSchema> {
        ensure_source(&self.config.source_id, source_id)?;
        let source_id = source_id.clone();
        let result_policy = self.result_policy.clone();
        self.on_connection(&self.operation_budget(), move |connection| {
            inspect_catalog(connection, &source_id, &result_policy)
        })
        .await
    }
}

#[async_trait]
impl SqlQueryExecutor for SqliteConnector {
    async fn execute_query(&self, request: QueryRequest) -> CoreResult<QueryResult> {
        Ok(self.execute_governed(request, None).await?.model_result)
    }
}

#[async_trait]
impl FreshnessReader for SqliteConnector {
    async fn read_freshness(
        &self,
        source_id: &SourceId,
        relation: &str,
        time_column: &str,
    ) -> CoreResult<FreshnessObservation> {
        ensure_source(&self.config.source_id, source_id)?;
        let scope = self
            .result_policy
            .allowed_scope(ys_agent_core::WorkspaceId::new(), source_id)?;
        ensure_freshness_scope(&scope, relation, time_column)?;
        let source_id = source_id.clone();
        let relation = relation.to_owned();
        let time_column = time_column.to_owned();
        let managed = self.managed.is_some();
        self.on_connection(&self.operation_budget(), move |connection| {
            if managed {
                install_query_authorizer(connection, scope)?;
            }
            read_sqlite_freshness(connection, &source_id, &relation, &time_column)
        })
        .await
    }
}

struct ManagedSqliteState {
    governance: DatasourceGovernanceContext,
    identity: FileIdentity,
    closed: AtomicBool,
    permits: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

fn core_failure(code: &'static str) -> CoreError {
    CoreError::validation(code, "datasource operation rejected")
}

pub(crate) fn ds_failure(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: DsRemediation::Revalidate,
        operation_id: None,
    }
}

fn classify(error: CoreError) -> DsError {
    ds_failure(match error.code() {
        "datasource_timeout" => DsErrorCode::Timeout,
        "datasource_target_missing" => DsErrorCode::TargetMissing,
        "datasource_file_unreadable" => DsErrorCode::FileUnreadable,
        "datasource_closed" => DsErrorCode::Cancelled,
        "datasource_identity_changed" => DsErrorCode::ValidationStale,
        "datasource_policy_denied" => DsErrorCode::PolicyDenied,
        _ => DsErrorCode::ReadOnlyUnproven,
    })
}

/// Every component is checked: a canonical final name alone does not rule out symlink parents.
fn checked_file(path: &Path, roots: &[PathBuf]) -> CoreResult<FileIdentity> {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || !roots
            .iter()
            .any(|root| root.is_absolute() && path.starts_with(root) && path != root)
    {
        return Err(core_failure("datasource_policy_denied"));
    }
    let canonical_path = std::fs::canonicalize(path).map_err(|error| {
        core_failure(if error.kind() == std::io::ErrorKind::NotFound {
            "datasource_target_missing"
        } else {
            "datasource_file_unreadable"
        })
    })?;
    if canonical_path != path
        || !roots.iter().any(|root| {
            std::fs::canonicalize(root)
                .is_ok_and(|canonical_root| canonical_root == *root && path.starts_with(root))
        })
    {
        return Err(core_failure("datasource_policy_denied"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            core_failure(if error.kind() == std::io::ErrorKind::NotFound {
                "datasource_target_missing"
            } else {
                "datasource_file_unreadable"
            })
        })?;
        if metadata.file_type().is_symlink() {
            return Err(core_failure("datasource_policy_denied"));
        }
    }
    let metadata =
        std::fs::metadata(path).map_err(|_| core_failure("datasource_file_unreadable"))?;
    if !metadata.is_file() {
        return Err(core_failure("datasource_file_unreadable"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        // Identity checks must be implemented for a platform before activation is possible.
        Err(core_failure("datasource_file_unreadable"))
    }
}

impl ManagedSqliteState {
    fn validate_request(&self, request: &QueryRequest) -> CoreResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(core_failure("datasource_closed"));
        }
        if request.scope != self.governance.data_scope {
            return Err(core_failure("datasource_policy_denied"));
        }
        Ok(())
    }
}

fn limit_budget(request: &QueryBudget, policy: &QueryBudget) -> QueryBudget {
    QueryBudget {
        max_sql_bytes: request.max_sql_bytes.min(policy.max_sql_bytes),
        statement_timeout_ms: request
            .statement_timeout_ms
            .min(policy.statement_timeout_ms),
        acquire_timeout_ms: request.acquire_timeout_ms.min(policy.acquire_timeout_ms),
        max_rows: request.max_rows.min(policy.max_rows),
        max_result_bytes: request.max_result_bytes.min(policy.max_result_bytes),
        max_concurrency: request.max_concurrency.min(policy.max_concurrency),
        max_estimated_cost_units: policy.max_estimated_cost_units,
        max_scanned_bytes: policy.max_scanned_bytes,
    }
}

impl SqliteConnector {
    /// Only callers holding explicit trusted target/scope authority can construct managed handles.
    pub async fn open_managed(input: ConnectorOpenInput) -> DsResult<Self> {
        let revision = input.revision.input();
        if revision.adapter_id.as_str() != "sqlite"
            || revision.adapter_version.as_str() != "1"
            || revision.config_version != 1
        {
            return Err(ds_failure(DsErrorCode::ConfigIncompatible));
        }
        let DatabaseContext::File { canonical_path } = &revision.context else {
            return Err(ds_failure(DsErrorCode::InvalidField));
        };
        if revision.fields.len() != 1
            || revision
                .fields
                .get(&FieldId::new("database_path").expect("static field"))
                != Some(&FieldValue::Text(
                    canonical_path.to_string_lossy().into_owned(),
                ))
            || input.secret.is_some()
            || revision.credential.is_some()
        {
            return Err(ds_failure(DsErrorCode::InvalidField));
        }
        let source = revision
            .source_id
            .clone()
            .ok_or_else(|| ds_failure(DsErrorCode::PolicyDenied))?;
        if source.as_str() != input.governance.data_scope.source_id
            || revision.workspace_id != input.governance.data_scope.workspace_id
            || input.governance.result_policy != input.governance.data_scope.relations
            || input.governance.data_scope.relations.is_empty()
            || input.governance.budget.max_concurrency == 0
        {
            return Err(ds_failure(DsErrorCode::PolicyDenied));
        }
        let path = canonical_path.clone();
        let roots = input.governance.allowed_roots.clone();
        let identity = blocking(move || checked_file(&path, &roots))
            .await
            .map_err(classify)?;
        let result_policy = ResultPolicy::from_scope(&input.governance.data_scope);
        let connector = Self {
            config: SqliteConnectorConfig {
                source_id: source,
                database_path: canonical_path.clone(),
                max_concurrency: 1,
                freshness_columns: BTreeMap::new(),
            },
            sql_policy: SqlReadOnlyPolicy::new(
                super::SupportedDialect::SQLite,
                input.governance.budget.max_sql_bytes,
            ),
            result_policy,
            managed: Some(Arc::new(ManagedSqliteState {
                governance: input.governance,
                identity,
                closed: AtomicBool::new(false),
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
            })),
        };
        connector.probe().await?;
        Ok(connector)
    }

    fn operation_budget(&self) -> QueryBudget {
        let mut budget = self
            .managed
            .as_ref()
            .map(|state| state.governance.budget.clone())
            .unwrap_or_default();
        budget.statement_timeout_ms = budget.statement_timeout_ms.min(10_000);
        budget
    }

    async fn on_connection<T: Send + 'static>(
        &self,
        budget: &QueryBudget,
        operation: impl FnOnce(&Connection) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<T> {
        let path = self.config.database_path.clone();
        let Some(state) = self.managed.clone() else {
            return blocking(move || operation(&open_read_only(&path)?)).await;
        };
        if state.closed.load(Ordering::Acquire) {
            return Err(core_failure("datasource_closed"));
        }
        if budget.statement_timeout_ms == 0
            || budget.acquire_timeout_ms == 0
            || budget.max_concurrency == 0
        {
            return Err(core_failure("datasource_timeout"));
        }
        let duration = Duration::from_millis(budget.statement_timeout_ms);
        let deadline = Instant::now() + duration;
        let permit = tokio::time::timeout(
            Duration::from_millis(budget.acquire_timeout_ms).min(duration),
            state.permits.clone().acquire_owned(),
        )
        .await
        .map_err(|_| core_failure("datasource_timeout"))?
        .map_err(|_| core_failure("datasource_closed"))?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = CancelOnDrop(cancelled.clone());
        let worker_state = state.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let state = worker_state;
            if state.closed.load(Ordering::Acquire) {
                return Err(core_failure("datasource_closed"));
            }
            if Instant::now() >= deadline || cancelled.load(Ordering::Acquire) {
                return Err(core_failure("datasource_timeout"));
            }
            if checked_file(&path, &state.governance.allowed_roots)? != state.identity {
                return Err(core_failure("datasource_identity_changed"));
            }
            let connection = open_read_only_flags(&path, true)
                .map_err(|_| core_failure("datasource_file_unreadable"))?;
            if checked_file(&path, &state.governance.allowed_roots)? != state.identity {
                return Err(core_failure("datasource_identity_changed"));
            }
            connection
                .busy_timeout(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(100)),
                )
                .map_err(|_| core_failure("datasource_timeout"))?;
            let progress_state = state.clone();
            connection
                .progress_handler(
                    1000,
                    Some(move || {
                        Instant::now() >= deadline
                            || cancelled.load(Ordering::Acquire)
                            || progress_state.closed.load(Ordering::Acquire)
                    }),
                )
                .map_err(|_| core_failure("datasource_readonly_unproven"))?;
            let result = operation(&connection);
            // Connection and permit are dropped before completion is reported, including interruption.
            if Instant::now() >= deadline {
                Err(core_failure("datasource_timeout"))
            } else if state.closed.load(Ordering::Acquire) {
                Err(core_failure("datasource_closed"))
            } else {
                result.map_err(|error| {
                    core_failure(match error.code() {
                        "connector_storage_error" => "datasource_query_failed",
                        "configured_relation_missing" => "datasource_target_missing",
                        "result_byte_budget_exceeded" => "result_byte_budget_exceeded",
                        _ => "datasource_policy_denied",
                    })
                })
            }
        });
        let result = match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(100),
            &mut worker,
        )
        .await
        {
            Ok(result) => result.map_err(|_| core_failure("datasource_query_failed"))?,
            Err(_) => {
                guard.0.store(true, Ordering::Release);
                if tokio::time::timeout(Duration::from_secs(1), &mut worker)
                    .await
                    .is_err()
                {
                    state.closed.store(true, Ordering::Release); // quarantine an unresponsive driver
                }
                Err(core_failure("datasource_timeout"))
            }
        };
        drop(guard);
        result
    }
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn install_query_authorizer(
    connection: &Connection,
    scope: ys_agent_core::AllowedDataScope,
) -> CoreResult<()> {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            let allowed = match context.action {
                AuthAction::Select | AuthAction::Recursive => true,
                // SQLite reports no database name for count(*)'s empty-column READ. Each
                // connection is fresh, has only main, and ATTACH/temp creation are denied.
                AuthAction::Read {
                    table_name,
                    column_name,
                } => {
                    (context.database_name == Some("main")
                        || (context.database_name.is_none() && column_name.is_empty()))
                        && scope.relations.get(table_name).is_some_and(|columns| {
                            column_name.is_empty()
                                || columns.get(column_name).is_some_and(|policy| {
                                    *policy != ys_agent_core::ColumnPolicy::Deny
                                })
                        })
                }
                AuthAction::Function { function_name } => matches!(
                    function_name.to_ascii_lowercase().as_str(),
                    "abs"
                        | "avg"
                        | "count"
                        | "sum"
                        | "total"
                        | "min"
                        | "max"
                        | "coalesce"
                        | "ifnull"
                        | "nullif"
                        | "round"
                        | "length"
                        | "lower"
                        | "upper"
                        | "trim"
                        | "ltrim"
                        | "rtrim"
                        | "substr"
                        | "substring"
                        | "replace"
                        | "date"
                        | "datetime"
                        | "strftime"
                        | "julianday"
                        | "unixepoch"
                        | "like"
                        | "glob"
                        | "typeof"
                ),
                _ => false,
            };
            if allowed {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }))
        .map_err(|_| core_failure("datasource_policy_denied"))
}

#[async_trait]
impl ManagedConnector for SqliteConnector {
    async fn probe(&self) -> DsResult<ProbeEvidence> {
        if self.managed.is_none() {
            return Err(ds_failure(DsErrorCode::PolicyDenied));
        }
        let source = self.config.source_id.clone();
        let policy = self.result_policy.clone();
        self.on_connection(&self.operation_budget(), move |connection| {
            let read_only = connection
                .is_readonly(rusqlite::MAIN_DB)
                .map_err(|_| core_failure("datasource_readonly_unproven"))?;
            let query_only: bool = connection
                .pragma_query_value(None, "query_only", |row| row.get(0))
                .map_err(|_| core_failure("datasource_readonly_unproven"))?;
            if !read_only || !query_only {
                return Err(core_failure("datasource_readonly_unproven"));
            }
            let schema = inspect_catalog(connection, &source, &policy)?;
            for relation in &schema.relations {
                let (kind, sql): (String, String) = connection
                    .query_row(
                        "SELECT type, sql FROM sqlite_schema WHERE name=?1",
                        [&relation.name],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|_| core_failure("datasource_policy_denied"))?;
                if kind != "table" || sql.to_ascii_uppercase().contains("VIRTUAL TABLE") {
                    return Err(core_failure("datasource_policy_denied"));
                }
            }
            Ok(ProbeEvidence {
                authenticated: true,
                target_verified: true,
                read_only_verified: true,
                least_privilege_verified: true,
                capabilities_verified: true,
            })
        })
        .await
        .map_err(classify)
    }

    async fn close(&self) -> DsResult<()> {
        let Some(state) = &self.managed else {
            return Err(ds_failure(DsErrorCode::PolicyDenied));
        };
        state.closed.store(true, Ordering::Release);
        let permit = tokio::time::timeout(Duration::from_secs(1), state.permits.acquire())
            .await
            .map_err(|_| ds_failure(DsErrorCode::Timeout))?
            .map_err(|_| ds_failure(DsErrorCode::Cancelled))?;
        drop(permit);
        Ok(())
    }
}

fn validate_request_source(expected: &SourceId, request: &QueryRequest) -> CoreResult<()> {
    ensure_source(expected, &request.source_id)?;
    if request.scope.source_id != expected.as_str() {
        return Err(CoreError::validation(
            "scope_source_mismatch",
            "request scope belongs to another source",
        ));
    }
    Ok(())
}

fn ensure_source(expected: &SourceId, actual: &SourceId) -> CoreResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(CoreError::validation(
            "source_mismatch",
            "connector was called for another source",
        ))
    }
}

fn ensure_freshness_scope(
    scope: &ys_agent_core::AllowedDataScope,
    relation: &str,
    column: &str,
) -> CoreResult<()> {
    let columns = scope.relations.get(relation).ok_or_else(|| {
        CoreError::validation(
            "relation_not_allowed",
            format!("relation {relation} is not allowed"),
        )
    })?;
    match columns.get(column) {
        Some(ys_agent_core::ColumnPolicy::Allow | ys_agent_core::ColumnPolicy::Redact) => Ok(()),
        _ => Err(CoreError::validation(
            "column_not_allowed",
            format!("freshness column {relation}.{column} is not readable"),
        )),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn storage_error(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::validation("connector_storage_error", format!("{context}: {error}"))
}

async fn blocking<T, F>(operation: F) -> CoreResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CoreResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| CoreError::validation("connector_task_failed", error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::open_read_only;

    #[test]
    fn governed_authorizer_accepts_count_without_a_column_read() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE readings(value); INSERT INTO readings VALUES(1), (2)")
            .unwrap();
        super::install_query_authorizer(
            &db,
            ys_agent_core::AllowedDataScope {
                workspace_id: ys_agent_core::WorkspaceId::new(),
                source_id: "test".into(),
                relations: [(
                    "readings".into(),
                    [("value".into(), ys_agent_core::ColumnPolicy::Allow)].into(),
                )]
                .into(),
            },
        )
        .unwrap();
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM readings a CROSS JOIN readings b",
                [],
                |row| row.get(0),
            )
            .expect("authorized count");
        assert_eq!(count, 4);
    }

    #[test]
    fn physical_read_only_connection_rejects_write_without_ast_policy() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("physical.db");
        let writable = rusqlite::Connection::open(&path).expect("create database");
        writable
            .execute_batch("CREATE TABLE values_table (value INTEGER);")
            .expect("create table");
        drop(writable);

        let read_only = open_read_only(&path).expect("open read only");
        let error = read_only
            .execute("INSERT INTO values_table (value) VALUES (1)", [])
            .expect_err("physical layer must reject write");
        assert!(error.to_string().to_ascii_lowercase().contains("readonly"));
    }
}
