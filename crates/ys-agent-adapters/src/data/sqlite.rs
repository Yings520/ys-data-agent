use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
            max_concurrency: self.config.max_concurrency,
        }
    }

    pub async fn execute_governed(
        &self,
        request: QueryRequest,
        restricted_context: Option<&RestrictedResultContext>,
    ) -> CoreResult<GovernedQueryResult> {
        validate_request_source(&self.config.source_id, &request)?;
        let policy_decision = self.sql_policy.evaluate(&request.sql, &request.scope);
        policy_decision.ensure_allowed()?;

        let path = self.config.database_path.clone();
        let sql = request.sql.clone();
        let parameters = request.parameters.clone();
        let budget = request.budget.clone();
        let decoded = blocking(move || {
            execute_read(
                &path,
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
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| storage_error("open SQLite database", error))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| storage_error("set SQLite query_only", error))?;
    Ok(connection)
}

fn execute_read(
    path: &Path,
    sql: &str,
    parameters: Vec<QueryParameter>,
    max_rows: usize,
    max_result_bytes: usize,
) -> CoreResult<DecodedQueryResult> {
    let connection = open_read_only(path)?;
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
    path: &Path,
    source_id: &SourceId,
    result_policy: &ResultPolicy,
) -> CoreResult<ObservedSchema> {
    let connection = open_read_only(path)?;
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
    path: &Path,
    source_id: &SourceId,
    relation: &str,
    column: &str,
) -> CoreResult<FreshnessObservation> {
    let connection = open_read_only(path)?;
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
        let path = self.config.database_path.clone();
        let source_id = source_id.clone();
        let result_policy = self.result_policy.clone();
        blocking(move || inspect_catalog(&path, &source_id, &result_policy)).await
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
        let path = self.config.database_path.clone();
        let source_id = source_id.clone();
        let relation = relation.to_owned();
        let time_column = time_column.to_owned();
        blocking(move || read_sqlite_freshness(&path, &source_id, &relation, &time_column)).await
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
