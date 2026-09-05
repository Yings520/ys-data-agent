use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use duckdb::types::{TimeUnit, Value, ValueRef};
use duckdb::{AccessMode, Config, Connection, params_from_iter};
use ys_agent_core::{
    CapabilityDescriptor, CatalogReader, CellValue, ConnectorFactory, ConnectorOpenInput,
    CoreError, CoreResult, DatabaseContext, DatasourceGovernanceContext, DatasourceRevision,
    DsError, DsErrorCode, DsRemediation, DsResult, FieldId, FieldIssue, FieldIssueCode, FieldValue,
    FreshnessObservation, FreshnessReader, ManagedConnector, ObservedColumn, ObservedRelation,
    ObservedSchema, ProbeEvidence, QueryBudget, QueryCostEstimate, QueryParameter, QueryPreflight,
    QueryPreflightDecision, QueryPreflightReader, QueryRequest, QueryResult, SchemaKnowledgeKind,
    SourceId, SqlQueryExecutor, validate_datasource_fields,
};

use super::result_policy::{DecodedQueryResult, ResultPolicy};
use super::sql_policy::{SqlPolicyDisposition, SqlReadOnlyPolicy};

#[derive(Clone)]
pub struct DuckDbConnector {
    source_id: SourceId,
    sql_policy: SqlReadOnlyPolicy,
    result_policy: ResultPolicy,
    state: Arc<ManagedDuckDbState>,
}

struct ManagedDuckDbState {
    governance: DatasourceGovernanceContext,
    path: PathBuf,
    identity: FileIdentity,
    connection: Mutex<Option<Connection>>,
    interrupt: Arc<duckdb::InterruptHandle>,
    permits: Arc<tokio::sync::Semaphore>,
    closed: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

pub struct DuckDbConnectorFactory;

#[async_trait]
impl ConnectorFactory for DuckDbConnectorFactory {
    fn validate_config(&self, revision: &DatasourceRevision) -> Vec<FieldIssue> {
        let descriptor =
            super::catalog::builtin_descriptor("duckdb").expect("static DuckDB descriptor");
        let input = revision.input();
        let path_field = FieldId::new("database_path").expect("static field");
        let mut issues = validate_datasource_fields(
            &descriptor.fields,
            &input.fields,
            input.credential.is_some(),
            true,
        );
        if input.adapter_id != descriptor.adapter_id
            || input.adapter_version != descriptor.adapter_version
            || input.config_version != descriptor.config_version
            || input.credential.is_some()
            || !matches!(&input.context, DatabaseContext::File { canonical_path }
                if input.fields.get(&path_field)
                    == Some(&FieldValue::Text(canonical_path.to_string_lossy().into_owned())))
        {
            issues.push(FieldIssue {
                field: path_field,
                code: FieldIssueCode::Invalid,
            });
        }
        issues
    }

    async fn open(&self, input: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>> {
        if !self.validate_config(&input.revision).is_empty() {
            return Err(ds_failure(DsErrorCode::InvalidField));
        }
        Ok(Arc::new(DuckDbConnector::open_managed(input).await?))
    }
}

impl DuckDbConnector {
    pub async fn open_managed(input: ConnectorOpenInput) -> DsResult<Self> {
        let revision = input.revision.input();
        let DatabaseContext::File { canonical_path } = &revision.context else {
            return Err(ds_failure(DsErrorCode::InvalidField));
        };
        if input.secret.is_some()
            || revision.credential.is_some()
            || revision.fields.len() != 1
            || input.governance.allowed_roots.is_empty()
            || input.governance.data_scope.relations.is_empty()
            || input.governance.result_policy != input.governance.data_scope.relations
            || revision.workspace_id != input.governance.data_scope.workspace_id
        {
            return Err(ds_failure(DsErrorCode::PolicyDenied));
        }
        let source_id = revision
            .source_id
            .clone()
            .ok_or_else(|| ds_failure(DsErrorCode::PolicyDenied))?;
        if source_id.as_str() != input.governance.data_scope.source_id
            || !input
                .governance
                .data_scope
                .relations
                .keys()
                .all(|relation| safe_identifier(relation))
        {
            return Err(ds_failure(DsErrorCode::PolicyDenied));
        }

        let path = canonical_path.clone();
        let roots = input.governance.allowed_roots.clone();
        let (identity, connection, interrupt) = tokio::task::spawn_blocking(move || {
            let identity = checked_file(&path, &roots)?;
            let connection = open_read_only(&path)?;
            if checked_file(&path, &roots)? != identity {
                return Err(core_failure("datasource_identity_changed"));
            }
            let interrupt = connection.interrupt_handle();
            Ok((identity, connection, interrupt))
        })
        .await
        .map_err(|_| ds_failure(DsErrorCode::Protocol))?
        .map_err(classify)?;

        let result_policy = ResultPolicy::from_scope(&input.governance.data_scope);
        let connector = Self {
            source_id,
            sql_policy: SqlReadOnlyPolicy::new(
                super::SupportedDialect::DuckDB,
                input.governance.budget.max_sql_bytes,
            ),
            result_policy,
            state: Arc::new(ManagedDuckDbState {
                governance: input.governance,
                path: canonical_path.clone(),
                identity,
                connection: Mutex::new(Some(connection)),
                interrupt,
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
                closed: AtomicBool::new(false),
            }),
        };
        connector.probe().await?;
        Ok(connector)
    }

    pub fn capabilities(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            source_id: self.source_id.clone(),
            dialect: "duckdb".into(),
            catalog_reader: true,
            preflight_reader: true,
            sql_query_executor: true,
            freshness_reader: true,
            supports_explain: false,
            supports_read_only_tx: false,
            read_only_mechanism: Some(ys_agent_core::ReadOnlyMechanism::FileReadOnly),
            max_concurrency: 1,
        }
    }

    fn operation_budget(&self) -> QueryBudget {
        let mut budget = self.state.governance.budget.clone();
        budget.statement_timeout_ms = budget.statement_timeout_ms.min(10_000);
        budget
    }

    fn validate_request(&self, request: &QueryRequest) -> CoreResult<QueryBudget> {
        if self.state.closed.load(Ordering::Acquire) {
            return Err(core_failure("datasource_closed"));
        }
        ensure_source(&self.source_id, &request.source_id)?;
        if request.scope != self.state.governance.data_scope {
            return Err(core_failure("datasource_policy_denied"));
        }
        let budget = limit_budget(&request.budget, &self.state.governance.budget);
        if budget.statement_timeout_ms == 0
            || budget.acquire_timeout_ms == 0
            || budget.max_concurrency == 0
            || budget.max_rows == 0
            || budget.max_result_bytes == 0
        {
            return Err(core_failure("datasource_timeout"));
        }
        Ok(budget)
    }

    async fn on_connection<T: Send + 'static>(
        &self,
        budget: &QueryBudget,
        operation: impl FnOnce(&Connection) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<T> {
        if self.state.closed.load(Ordering::Acquire) {
            return Err(core_failure("datasource_closed"));
        }
        if budget.statement_timeout_ms == 0 || budget.acquire_timeout_ms == 0 {
            return Err(core_failure("datasource_timeout"));
        }
        let timeout = Duration::from_millis(budget.statement_timeout_ms);
        let deadline = Instant::now() + timeout;
        let permit = tokio::time::timeout(
            Duration::from_millis(budget.acquire_timeout_ms).min(timeout),
            self.state.permits.clone().acquire_owned(),
        )
        .await
        .map_err(|_| core_failure("datasource_timeout"))?
        .map_err(|_| core_failure("datasource_closed"))?;
        let state = self.state.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if state.closed.load(Ordering::Acquire) {
                return Err(core_failure("datasource_closed"));
            }
            if checked_file(&state.path, &state.governance.allowed_roots)? != state.identity {
                state.closed.store(true, Ordering::Release);
                return Err(core_failure("datasource_identity_changed"));
            }
            let guard = state
                .connection
                .lock()
                .map_err(|_| core_failure("datasource_closed"))?;
            let connection = guard
                .as_ref()
                .ok_or_else(|| core_failure("datasource_closed"))?;
            let result = operation(connection);
            if Instant::now() >= deadline {
                Err(core_failure("datasource_timeout"))
            } else if state.closed.load(Ordering::Acquire) {
                Err(core_failure("datasource_closed"))
            } else {
                result
            }
        });
        let mut interrupt_guard = InterruptOnDrop(Some(self.state.interrupt.clone()));
        let result = match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            &mut worker,
        )
        .await
        {
            Ok(joined) => joined.map_err(|_| core_failure("datasource_query_failed"))?,
            Err(_) => {
                self.state.interrupt.interrupt();
                if tokio::time::timeout(Duration::from_secs(1), &mut worker)
                    .await
                    .is_err()
                {
                    self.state.closed.store(true, Ordering::Release);
                }
                Err(core_failure("datasource_timeout"))
            }
        };
        interrupt_guard.0 = None;
        result
    }

    async fn execute_governed(&self, mut request: QueryRequest) -> CoreResult<QueryResult> {
        request.budget = self.validate_request(&request)?;
        if request.sql.len() > request.budget.max_sql_bytes {
            return Err(core_failure("sql_too_large"));
        }
        let decision = self.sql_policy.evaluate(&request.sql, &request.scope);
        decision.ensure_allowed()?;
        let sql = request.sql.clone();
        let parameters = request.parameters.clone();
        let budget = request.budget.clone();
        let decoded = self
            .on_connection(&request.budget, move |connection| {
                execute_read(
                    connection,
                    &sql,
                    parameters,
                    budget.max_rows,
                    budget.max_result_bytes,
                )
            })
            .await?;
        Ok(self
            .result_policy
            .apply(
                &request.source_id,
                &decision.referenced_relations,
                &decision.referenced_columns,
                decoded,
                request.budget.max_result_bytes,
                None,
            )?
            .model_result)
    }
}

struct InterruptOnDrop(Option<Arc<duckdb::InterruptHandle>>);

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.interrupt();
        }
    }
}

#[async_trait]
impl QueryPreflightReader for DuckDbConnector {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        self.validate_request(request)?;
        let policy = self.sql_policy.evaluate(&request.sql, &request.scope);
        Ok(QueryPreflight {
            sql: request.sql.clone(),
            decision: match policy.disposition {
                SqlPolicyDisposition::Allowed => QueryPreflightDecision::Allowed,
                SqlPolicyDisposition::Rejected => QueryPreflightDecision::Rejected,
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
impl CatalogReader for DuckDbConnector {
    async fn observe_schema(&self, source_id: &SourceId) -> CoreResult<ObservedSchema> {
        ensure_source(&self.source_id, source_id)?;
        let source_id = source_id.clone();
        let policy = self.result_policy.clone();
        self.on_connection(&self.operation_budget(), move |connection| {
            inspect_catalog(connection, &source_id, &policy)
        })
        .await
    }
}

#[async_trait]
impl SqlQueryExecutor for DuckDbConnector {
    async fn execute_query(&self, request: QueryRequest) -> CoreResult<QueryResult> {
        self.execute_governed(request).await
    }
}

#[async_trait]
impl FreshnessReader for DuckDbConnector {
    async fn read_freshness(
        &self,
        source_id: &SourceId,
        relation: &str,
        time_column: &str,
    ) -> CoreResult<FreshnessObservation> {
        ensure_source(&self.source_id, source_id)?;
        ensure_freshness_scope(&self.state.governance.data_scope, relation, time_column)?;
        let source_id = source_id.clone();
        let relation = relation.to_owned();
        let time_column = time_column.to_owned();
        self.on_connection(&self.operation_budget(), move |connection| {
            read_freshness(connection, &source_id, &relation, &time_column)
        })
        .await
    }
}

#[async_trait]
impl ManagedConnector for DuckDbConnector {
    async fn probe(&self) -> DsResult<ProbeEvidence> {
        let source = self.source_id.clone();
        let policy = self.result_policy.clone();
        let scope = self.state.governance.data_scope.clone();
        self.on_connection(&self.operation_budget(), move |connection| {
            let settings = [
                ("access_mode", "read_only"),
                ("enable_external_access", "false"),
                ("autoload_known_extensions", "false"),
                ("autoinstall_known_extensions", "false"),
                ("lock_configuration", "true"),
            ];
            for (name, expected) in settings {
                let sql = format!("SELECT current_setting('{name}')::VARCHAR");
                let value: String = connection
                    .query_row(&sql, [], |row| row.get(0))
                    .map_err(|_| core_failure("datasource_readonly_unproven"))?;
                if !value.eq_ignore_ascii_case(expected) {
                    return Err(core_failure("datasource_readonly_unproven"));
                }
            }
            let temp_directory: String = connection
                .query_row("SELECT current_setting('temp_directory')", [], |row| row.get(0))
                .map_err(|_| core_failure("datasource_readonly_unproven"))?;
            if !temp_directory.is_empty() {
                return Err(core_failure("datasource_readonly_unproven"));
            }
            let observed = inspect_catalog(connection, &source, &policy)?;
            for relation in scope.relations.keys() {
                let table_type: Option<String> = optional_row(connection.query_row(
                    "SELECT table_type FROM information_schema.tables WHERE table_schema='main' AND table_name=?",
                    [relation],
                    |row| row.get(0),
                ))
                .map_err(|_| core_failure("datasource_target_missing"))?;
                if table_type.as_deref() != Some("BASE TABLE") {
                    return Err(core_failure("datasource_policy_denied"));
                }
            }
            if observed.relations.len() != scope.relations.len() {
                return Err(core_failure("datasource_target_missing"));
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
        if self.state.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.state.interrupt.interrupt();
        let permit = tokio::time::timeout(Duration::from_secs(2), self.state.permits.acquire())
            .await
            .map_err(|_| ds_failure(DsErrorCode::Timeout))?
            .map_err(|_| ds_failure(DsErrorCode::Cancelled))?;
        let state = self.state.clone();
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::task::spawn_blocking(move || {
                let connection = state
                    .connection
                    .lock()
                    .map_err(|_| ds_failure(DsErrorCode::Protocol))?
                    .take();
                if let Some(connection) = connection {
                    connection
                        .close()
                        .map_err(|_| ds_failure(DsErrorCode::Protocol))?;
                }
                Ok::<_, DsError>(())
            }),
        )
        .await
        .map_err(|_| ds_failure(DsErrorCode::Timeout))?
        .map_err(|_| ds_failure(DsErrorCode::Protocol))??;
        drop(permit);
        Ok(())
    }
}

fn open_read_only(path: &Path) -> CoreResult<Connection> {
    let config = Config::default()
        .access_mode(AccessMode::ReadOnly)
        .map_err(|_| core_failure("datasource_readonly_unproven"))?
        .enable_autoload_extension(false)
        .map_err(|_| core_failure("datasource_readonly_unproven"))?
        .threads(1)
        .map_err(|_| core_failure("datasource_readonly_unproven"))?
        .max_memory("256MB")
        .map_err(|_| core_failure("datasource_readonly_unproven"))?;
    let connection = Connection::open_with_flags(path, config)
        .map_err(|_| core_failure("datasource_file_unreadable"))?;
    connection
        .execute_batch(
            "SET temp_directory=''; SET max_temp_directory_size='0B'; SET enable_external_access=false; SET autoload_known_extensions=false; SET autoinstall_known_extensions=false; SET lock_configuration=true;",
        )
        .map_err(|_| core_failure("datasource_readonly_unproven"))?;
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
        .map_err(|_| core_failure("datasource_query_failed"))?;
    let values = parameters
        .into_iter()
        .map(duckdb_parameter)
        .collect::<Vec<_>>();
    let mut cursor = statement
        .query(params_from_iter(values.iter()))
        .map_err(|_| core_failure("datasource_query_failed"))?;
    let columns = cursor
        .as_ref()
        .ok_or_else(|| core_failure("datasource_query_failed"))?
        .column_names();
    let column_count = columns.len();
    let mut rows = Vec::new();
    let mut serialized_bytes = 0usize;
    let mut truncated = false;
    while let Some(row) = cursor
        .next()
        .map_err(|error| classify_query_error(&error))?
    {
        if rows.len() == max_rows {
            truncated = true;
            break;
        }
        let mut result_row = Vec::with_capacity(column_count);
        for index in 0..column_count {
            result_row.push(decode_value(
                row.get_ref(index)
                    .map_err(|_| core_failure("unsupported_duckdb_type"))?,
            )?);
        }
        let row_bytes = serde_json::to_vec(&result_row)
            .map_err(|_| core_failure("result_serialization_failed"))?
            .len();
        if serialized_bytes.saturating_add(row_bytes) > max_result_bytes {
            truncated = true;
            break;
        }
        serialized_bytes += row_bytes;
        rows.push(result_row);
    }
    Ok(DecodedQueryResult {
        columns,
        rows,
        truncated,
        remote_query_id: None,
        warning_codes: Vec::new(),
    })
}

fn duckdb_parameter(parameter: QueryParameter) -> Value {
    match parameter {
        QueryParameter::Timestamp(value) => {
            Value::Timestamp(TimeUnit::Microsecond, value.timestamp_micros())
        }
        QueryParameter::Text(value) => Value::Text(value),
        QueryParameter::Integer(value) => Value::BigInt(value),
        QueryParameter::Real(value) => Value::Double(value),
        QueryParameter::Boolean(value) => Value::Boolean(value),
    }
}

fn decode_value(value: ValueRef<'_>) -> CoreResult<CellValue> {
    match value {
        ValueRef::Null => Ok(CellValue::Null),
        ValueRef::Boolean(value) => Ok(CellValue::Boolean(value)),
        ValueRef::TinyInt(value) => Ok(CellValue::Integer(i64::from(value))),
        ValueRef::SmallInt(value) => Ok(CellValue::Integer(i64::from(value))),
        ValueRef::Int(value) => Ok(CellValue::Integer(i64::from(value))),
        ValueRef::BigInt(value) => Ok(CellValue::Integer(value)),
        ValueRef::HugeInt(value) => integer(value),
        ValueRef::UHugeInt(value) => integer(value),
        ValueRef::UTinyInt(value) => Ok(CellValue::Integer(i64::from(value))),
        ValueRef::USmallInt(value) => Ok(CellValue::Integer(i64::from(value))),
        ValueRef::UInt(value) => Ok(CellValue::Integer(i64::from(value))),
        ValueRef::UBigInt(value) => integer(value),
        ValueRef::Float(value) => Ok(CellValue::Real(f64::from(value))),
        ValueRef::Double(value) => Ok(CellValue::Real(value)),
        ValueRef::Decimal(value) => Ok(CellValue::Text(value.to_string())),
        ValueRef::Timestamp(unit, value) => {
            DateTime::<Utc>::from_timestamp_micros(unit.to_micros(value))
                .map(|value| CellValue::Text(value.to_rfc3339()))
                .ok_or_else(|| core_failure("unsupported_duckdb_type"))
        }
        ValueRef::Text(value) => std::str::from_utf8(value)
            .map(|value| CellValue::Text(value.to_owned()))
            .map_err(|_| core_failure("unsupported_duckdb_type")),
        ValueRef::Blob(value) => Ok(CellValue::BlobSummary { bytes: value.len() }),
        ValueRef::Date32(days) => NaiveDate::from_ymd_opt(1970, 1, 1)
            .and_then(|epoch| epoch.checked_add_signed(chrono::Duration::days(i64::from(days))))
            .map(|value| CellValue::Text(value.to_string()))
            .ok_or_else(|| core_failure("unsupported_duckdb_type")),
        _ => Err(core_failure("unsupported_duckdb_type")),
    }
}

fn integer<T: TryInto<i64>>(value: T) -> CoreResult<CellValue> {
    value
        .try_into()
        .map(CellValue::Integer)
        .map_err(|_| core_failure("unsupported_duckdb_type"))
}

fn inspect_catalog(
    connection: &Connection,
    source_id: &SourceId,
    result_policy: &ResultPolicy,
) -> CoreResult<ObservedSchema> {
    let scope = result_policy.allowed_scope(ys_agent_core::WorkspaceId::new(), source_id)?;
    let mut relations = Vec::new();
    for relation in scope.relations.keys() {
        let mut statement = connection
            .prepare(
                "SELECT column_name, data_type, is_nullable = 'YES' FROM information_schema.columns WHERE table_schema='main' AND table_name=? ORDER BY ordinal_position",
            )
            .map_err(|_| core_failure("datasource_target_missing"))?;
        let columns = statement
            .query_map([relation], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(|_| core_failure("datasource_target_missing"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| core_failure("datasource_target_missing"))?;
        if columns.is_empty() {
            return Err(core_failure("datasource_target_missing"));
        }
        relations.push(ObservedRelation {
            name: relation.clone(),
            columns: columns
                .into_iter()
                .map(|(name, data_type, nullable)| ObservedColumn {
                    sensitivity: result_policy.column_sensitivity(source_id, relation, &name),
                    name,
                    data_type,
                    nullable,
                    primary_key_position: None,
                })
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

fn read_freshness(
    connection: &Connection,
    source_id: &SourceId,
    relation: &str,
    time_column: &str,
) -> CoreResult<FreshnessObservation> {
    let sql = format!(
        "SELECT epoch_us(MAX({})) FROM {}",
        quote_identifier(time_column)?,
        quote_identifier(relation)?
    );
    let value: Option<i64> = connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|_| core_failure("datasource_query_failed"))?;
    let data_as_of = match value {
        Some(micros) => Some(
            DateTime::<Utc>::from_timestamp_micros(micros)
                .ok_or_else(|| core_failure("invalid_freshness_value"))?,
        ),
        None => None,
    };
    let observed_at = Utc::now();
    Ok(FreshnessObservation {
        source_id: source_id.clone(),
        relation: relation.into(),
        observed_at,
        data_as_of,
        lag_seconds: data_as_of.map(|value| {
            observed_at
                .signed_duration_since(value)
                .num_seconds()
                .max(0) as u64
        }),
    })
}

fn checked_file(path: &Path, roots: &[PathBuf]) -> CoreResult<FileIdentity> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || !roots
            .iter()
            .any(|root| root.is_absolute() && path.starts_with(root) && path != root)
    {
        return Err(core_failure("datasource_policy_denied"));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        core_failure(if error.kind() == std::io::ErrorKind::NotFound {
            "datasource_target_missing"
        } else {
            "datasource_file_unreadable"
        })
    })?;
    if canonical != path
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
        if std::fs::symlink_metadata(&current)
            .map_err(|_| core_failure("datasource_file_unreadable"))?
            .file_type()
            .is_symlink()
        {
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
        Err(core_failure("datasource_file_unreadable"))
    }
}

fn ensure_source(expected: &SourceId, actual: &SourceId) -> CoreResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(core_failure("source_mismatch"))
    }
}

fn ensure_freshness_scope(
    scope: &ys_agent_core::AllowedDataScope,
    relation: &str,
    column: &str,
) -> CoreResult<()> {
    match scope
        .relations
        .get(relation)
        .and_then(|columns| columns.get(column))
    {
        Some(ys_agent_core::ColumnPolicy::Allow | ys_agent_core::ColumnPolicy::Redact) => Ok(()),
        _ => Err(core_failure("datasource_policy_denied")),
    }
}

fn safe_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn quote_identifier(value: &str) -> CoreResult<String> {
    if !safe_identifier(value) {
        return Err(core_failure("datasource_policy_denied"));
    }
    Ok(format!("\"{value}\""))
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

fn classify_query_error(error: &duckdb::Error) -> CoreError {
    if error.to_string().to_ascii_lowercase().contains("interrupt") {
        core_failure("datasource_timeout")
    } else {
        core_failure("datasource_query_failed")
    }
}

fn core_failure(code: &'static str) -> CoreError {
    CoreError::validation(code, "datasource operation rejected")
}

fn ds_failure(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::Timeout => DsRemediation::Retry,
            DsErrorCode::PolicyDenied | DsErrorCode::ReadOnlyUnproven => {
                DsRemediation::RepairPolicy
            }
            _ => DsRemediation::EditConfiguration,
        },
        operation_id: None,
    }
}

fn classify(error: CoreError) -> DsError {
    ds_failure(match error.code() {
        "datasource_timeout" => DsErrorCode::Timeout,
        "datasource_target_missing" => DsErrorCode::TargetMissing,
        "datasource_file_unreadable" => DsErrorCode::FileUnreadable,
        "datasource_identity_changed" => DsErrorCode::ValidationStale,
        "datasource_closed" => DsErrorCode::Cancelled,
        "datasource_policy_denied" => DsErrorCode::PolicyDenied,
        _ => DsErrorCode::ReadOnlyUnproven,
    })
}

fn optional_row<T>(result: duckdb::Result<T>) -> duckdb::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error),
    }
}
