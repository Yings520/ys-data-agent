use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures::TryStreamExt;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow};
use sqlx::{AssertSqlSafe, Column, PgPool, Postgres, Row, Transaction, TypeInfo, ValueRef};
use ys_agent_core::{
    CapabilityDescriptor, CatalogReader, CellValue, CoreError, CoreResult, FreshnessObservation,
    FreshnessReader, ObservedColumn, ObservedRelation, ObservedSchema, QueryCostEstimate,
    QueryParameter, QueryPreflight, QueryPreflightDecision, QueryPreflightReader, QueryRequest,
    QueryResult, SchemaKnowledgeKind, SourceId, SqlQueryExecutor,
};

use super::result_policy::{
    DecodedQueryResult, GovernedQueryResult, RestrictedResultContext, ResultPolicy,
};
use super::sql_policy::{SqlPolicyDecision, SqlPolicyDisposition, SqlReadOnlyPolicy};

#[derive(Debug, Clone)]
pub struct PostgresConnectorConfig {
    pub source_id: SourceId,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub default_statement_timeout: Duration,
    pub confirmation_cost_units: u64,
    pub freshness_columns: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct PostgresConnector {
    config: PostgresConnectorConfig,
    pool: PgPool,
    sql_policy: SqlReadOnlyPolicy,
    result_policy: ResultPolicy,
}

impl PostgresConnector {
    pub async fn connect(
        config: PostgresConnectorConfig,
        database_url: &str,
        sql_policy: SqlReadOnlyPolicy,
        result_policy: ResultPolicy,
    ) -> CoreResult<Self> {
        if config.max_connections == 0 {
            return Err(CoreError::validation(
                "invalid_connector_config",
                "max_connections must be greater than zero",
            ));
        }
        let options = PgConnectOptions::from_str(database_url)
            .map_err(|_| safe_database_error("parse PostgreSQL connection options"))?
            .application_name("ysda");
        let default_timeout_ms = duration_millis(config.default_statement_timeout);
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(config.acquire_timeout)
            .after_connect(move |connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET default_transaction_read_only = on")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SELECT set_config('statement_timeout', $1, false)")
                        .bind(format!("{default_timeout_ms}ms"))
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .map_err(|_| safe_database_error("connect PostgreSQL pool"))?;

        Ok(Self {
            config,
            pool,
            sql_policy,
            result_policy,
        })
    }

    pub fn capabilities(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            source_id: self.config.source_id.clone(),
            dialect: "postgres".to_owned(),
            catalog_reader: true,
            sql_query_executor: true,
            freshness_reader: true,
            supports_explain: true,
            supports_read_only_tx: true,
            max_concurrency: self.config.max_connections as usize,
        }
    }

    async fn build_preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        validate_request_source(&self.config.source_id, request)?;
        let policy = self.sql_policy.evaluate(&request.sql, &request.scope);
        if policy.disposition == SqlPolicyDisposition::Rejected {
            return Ok(preflight_from_policy(request, &policy));
        }

        let mut warnings = Vec::new();
        let cost = if request.budget.max_estimated_cost_units.is_some() {
            match self.estimate_cost(request).await {
                Ok(cost) => cost,
                Err(_) => {
                    warnings.push("cost_unknown".to_owned());
                    QueryCostEstimate {
                        estimated_cost_units: None,
                        scanned_bytes: None,
                        estimator_version: Some("postgres-explain-json-v1".to_owned()),
                    }
                }
            }
        } else {
            QueryCostEstimate {
                estimated_cost_units: None,
                scanned_bytes: None,
                estimator_version: None,
            }
        };

        let mut decision = QueryPreflightDecision::Allowed;
        let mut reason_codes = Vec::new();
        if let (Some(estimated), Some(hard_limit)) = (
            cost.estimated_cost_units,
            request.budget.max_estimated_cost_units,
        ) {
            if estimated > hard_limit {
                decision = QueryPreflightDecision::Rejected;
                reason_codes.push("estimated_cost_hard_limit".to_owned());
            } else if estimated > self.config.confirmation_cost_units
                && !request.confirmation_granted
            {
                decision = QueryPreflightDecision::ConfirmationRequired;
                reason_codes.push("estimated_cost_confirmation_required".to_owned());
            }
        } else if request.budget.max_estimated_cost_units.is_some() && !request.confirmation_granted
        {
            decision = QueryPreflightDecision::ConfirmationRequired;
            reason_codes.push("cost_unknown".to_owned());
        }
        if request.budget.max_scanned_bytes.is_some() {
            warnings.push("scanned_bytes_unknown".to_owned());
            if !request.confirmation_granted && decision != QueryPreflightDecision::Rejected {
                decision = QueryPreflightDecision::ConfirmationRequired;
                reason_codes.push("scanned_bytes_unknown".to_owned());
            }
        }

        Ok(QueryPreflight {
            sql: request.sql.clone(),
            decision,
            cost,
            reason_codes,
            warnings,
        })
    }

    async fn estimate_cost(&self, request: &QueryRequest) -> CoreResult<QueryCostEstimate> {
        let mut transaction = self.begin_read_only(request).await?;
        set_local_statement_timeout(&mut transaction, request.budget.statement_timeout_ms).await?;
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {}", request.sql);
        let value = sqlx::query_scalar::<_, Value>(AssertSqlSafe(explain_sql))
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| safe_database_error("run PostgreSQL EXPLAIN"))?;
        transaction
            .rollback()
            .await
            .map_err(|_| safe_database_error("rollback PostgreSQL EXPLAIN"))?;

        let estimated_cost_units = value
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item.get("Plan"))
            .and_then(|plan| plan.get("Total Cost"))
            .and_then(Value::as_f64)
            .and_then(cost_to_units);

        Ok(QueryCostEstimate {
            estimated_cost_units,
            scanned_bytes: None,
            estimator_version: Some("postgres-explain-json-v1".to_owned()),
        })
    }

    async fn begin_read_only(
        &self,
        request: &QueryRequest,
    ) -> CoreResult<Transaction<'static, Postgres>> {
        let requested_timeout = Duration::from_millis(request.budget.acquire_timeout_ms);
        let timeout = requested_timeout.min(self.config.acquire_timeout);
        tokio::time::timeout(timeout, self.pool.begin_with("BEGIN READ ONLY"))
            .await
            .map_err(|_| {
                CoreError::validation(
                    "connection_acquire_timeout",
                    "timed out while acquiring a PostgreSQL connection",
                )
            })?
            .map_err(|_| safe_database_error("begin read-only PostgreSQL transaction"))
    }

    pub async fn cancel(&self, external_query_id: i32) -> CoreResult<bool> {
        sqlx::query_scalar::<_, bool>("SELECT pg_cancel_backend($1)")
            .bind(external_query_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| safe_database_error("cancel PostgreSQL backend"))
    }

    async fn execute_postgres_rows(
        transaction: &mut Transaction<'_, Postgres>,
        request: &QueryRequest,
        max_rows: usize,
        max_result_bytes: usize,
        backend_pid: i32,
    ) -> CoreResult<DecodedQueryResult> {
        let mut columns = Vec::new();
        let mut query = sqlx::query(AssertSqlSafe(request.sql.as_str()));
        for parameter in &request.parameters {
            query = match parameter {
                QueryParameter::Timestamp(value) => query.bind(*value),
                QueryParameter::Text(value) => query.bind(value.clone()),
                QueryParameter::Integer(value) => query.bind(*value),
                QueryParameter::Real(value) => query.bind(*value),
                QueryParameter::Boolean(value) => query.bind(*value),
            };
        }
        let mut stream = query.fetch(&mut **transaction);
        let mut rows = Vec::new();
        let mut serialized_bytes = 0usize;
        let mut truncated = false;
        let mut warnings = BTreeMap::<String, ()>::new();

        while let Some(row) = stream
            .try_next()
            .await
            .map_err(|_| safe_database_error("stream PostgreSQL rows"))?
        {
            if columns.is_empty() {
                columns = row
                    .columns()
                    .iter()
                    .map(|column| column.name().to_owned())
                    .collect();
            }
            if rows.len() == max_rows {
                truncated = true;
                break;
            }
            let mut values = Vec::with_capacity(row.len());
            for index in 0..row.len() {
                let (value, warning) = decode_postgres_cell(&row, index)?;
                if let Some(warning) = warning {
                    warnings.insert(warning, ());
                }
                values.push(value);
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
        drop(stream);

        Ok(DecodedQueryResult {
            columns,
            rows,
            truncated,
            remote_query_id: Some(backend_pid.to_string()),
            warning_codes: warnings.into_keys().collect(),
        })
    }

    pub async fn execute_governed(
        &self,
        request: QueryRequest,
        restricted_context: Option<&RestrictedResultContext>,
    ) -> CoreResult<GovernedQueryResult> {
        validate_request_source(&self.config.source_id, &request)?;
        let policy_decision = self.sql_policy.evaluate(&request.sql, &request.scope);
        policy_decision.ensure_allowed()?;

        let mut transaction = self.begin_read_only(&request).await?;
        set_local_statement_timeout(&mut transaction, request.budget.statement_timeout_ms).await?;
        sqlx::query("SELECT set_config('application_name', $1, true)")
            .bind(safe_query_tag(&request.query_tag))
            .execute(&mut *transaction)
            .await
            .map_err(|_| safe_database_error("set PostgreSQL query tag"))?;
        let backend_pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| safe_database_error("read PostgreSQL backend id"))?;
        let decoded = Self::execute_postgres_rows(
            &mut transaction,
            &request,
            request.budget.max_rows,
            request.budget.max_result_bytes,
            backend_pid,
        )
        .await?;
        transaction
            .rollback()
            .await
            .map_err(|_| safe_database_error("rollback PostgreSQL transaction"))?;

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

fn decode_postgres_cell(row: &PgRow, index: usize) -> CoreResult<(CellValue, Option<String>)> {
    let raw = row
        .try_get_raw(index)
        .map_err(|_| safe_database_error("read PostgreSQL value"))?;
    if raw.is_null() {
        return Ok((CellValue::Null, None));
    }

    let type_name = row.column(index).type_info().name().to_ascii_uppercase();
    let value = match type_name.as_str() {
        "BOOL" => CellValue::Boolean(decode::<bool>(row, index)?),
        "INT2" => CellValue::Integer(i64::from(decode::<i16>(row, index)?)),
        "INT4" => CellValue::Integer(i64::from(decode::<i32>(row, index)?)),
        "INT8" => CellValue::Integer(decode::<i64>(row, index)?),
        "FLOAT4" => CellValue::Real(f64::from(decode::<f32>(row, index)?)),
        "FLOAT8" => CellValue::Real(decode::<f64>(row, index)?),
        "NUMERIC" => CellValue::Text(decode::<BigDecimal>(row, index)?.to_string()),
        "TEXT" | "VARCHAR" | "NAME" | "BPCHAR" => CellValue::Text(decode::<String>(row, index)?),
        "UUID" => CellValue::Text(decode::<uuid::Uuid>(row, index)?.to_string()),
        "DATE" => CellValue::Text(decode::<NaiveDate>(row, index)?.to_string()),
        "TIMESTAMP" => CellValue::Text(decode::<NaiveDateTime>(row, index)?.to_string()),
        "TIMESTAMPTZ" => CellValue::Text(decode::<DateTime<Utc>>(row, index)?.to_rfc3339()),
        "BYTEA" => CellValue::BlobSummary {
            bytes: decode::<Vec<u8>>(row, index)?.len(),
        },
        _ => {
            let debug_value = raw
                .as_bytes()
                .map(|bytes| format!("{:?}", &bytes[..bytes.len().min(32)]))
                .unwrap_or_else(|_| "<unavailable>".to_owned());
            return Ok((
                CellValue::Text(format!(
                    "<unsupported postgres type {type_name}: {debug_value}>"
                )),
                Some(format!(
                    "postgres_type_conversion_fallback:{}",
                    type_name.to_ascii_lowercase()
                )),
            ));
        }
    };
    Ok((value, None))
}

fn decode<'row, T>(row: &'row PgRow, index: usize) -> CoreResult<T>
where
    T: sqlx::Decode<'row, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(index)
        .map_err(|_| safe_database_error("decode PostgreSQL cell"))
}

const CATALOG_SQL: &str = r#"
SELECT
    c.table_schema,
    c.table_name,
    c.column_name,
    c.data_type,
    c.is_nullable = 'YES' AS nullable,
    pk.primary_key_position
FROM information_schema.columns AS c
LEFT JOIN (
    SELECT
        kcu.table_schema,
        kcu.table_name,
        kcu.column_name,
        kcu.ordinal_position::int AS primary_key_position
    FROM information_schema.table_constraints AS tc
    JOIN information_schema.key_column_usage AS kcu
      ON tc.constraint_name = kcu.constraint_name
     AND tc.table_schema = kcu.table_schema
     AND tc.table_name = kcu.table_name
    WHERE tc.constraint_type = 'PRIMARY KEY'
) AS pk
  ON pk.table_schema = c.table_schema
 AND pk.table_name = c.table_name
 AND pk.column_name = c.column_name
WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema')
ORDER BY c.table_schema, c.table_name, c.ordinal_position
"#;

async fn inspect_postgres_catalog(
    pool: &PgPool,
    source_id: &SourceId,
    result_policy: &ResultPolicy,
) -> CoreResult<ObservedSchema> {
    let scope = result_policy.allowed_scope(ys_agent_core::WorkspaceId::new(), source_id)?;
    let rows = sqlx::query(CATALOG_SQL)
        .fetch_all(pool)
        .await
        .map_err(|_| safe_database_error("inspect PostgreSQL catalog"))?;
    let mut grouped = BTreeMap::<String, Vec<ObservedColumn>>::new();

    for row in rows {
        let schema = row
            .try_get::<String, _>("table_schema")
            .map_err(|_| safe_database_error("decode catalog schema"))?;
        let table = row
            .try_get::<String, _>("table_name")
            .map_err(|_| safe_database_error("decode catalog table"))?;
        let relation = format!("{schema}.{table}");
        if !scope.relations.contains_key(&relation) {
            continue;
        }
        let name = row
            .try_get::<String, _>("column_name")
            .map_err(|_| safe_database_error("decode catalog column"))?;
        let primary_key_position = row
            .try_get::<Option<i32>, _>("primary_key_position")
            .map_err(|_| safe_database_error("decode catalog primary key"))?
            .and_then(|position| u32::try_from(position).ok());
        grouped
            .entry(relation.clone())
            .or_default()
            .push(ObservedColumn {
                sensitivity: result_policy.column_sensitivity(source_id, &relation, &name),
                name,
                data_type: row
                    .try_get("data_type")
                    .map_err(|_| safe_database_error("decode catalog type"))?,
                nullable: row
                    .try_get("nullable")
                    .map_err(|_| safe_database_error("decode catalog nullability"))?,
                primary_key_position,
            });
    }

    for relation in scope.relations.keys() {
        if !grouped.contains_key(relation) {
            return Err(CoreError::validation(
                "configured_relation_missing",
                format!("configured PostgreSQL relation {relation} does not exist"),
            ));
        }
    }

    Ok(ObservedSchema {
        source_id: source_id.clone(),
        kind: SchemaKnowledgeKind::Observed,
        relations: grouped
            .into_iter()
            .map(|(name, columns)| ObservedRelation { name, columns })
            .collect(),
        observed_at: Utc::now(),
    })
}

async fn read_postgres_freshness(
    pool: &PgPool,
    source_id: &SourceId,
    relation: &str,
    column: &str,
) -> CoreResult<FreshnessObservation> {
    let relation_name = relation.to_owned();
    let quoted_relation = quote_qualified_identifier(relation)?;
    let quoted_column = quote_identifier(column)?;
    let sql = format!("SELECT MAX({quoted_column}) FROM {quoted_relation}");
    let data_as_of = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .map_err(|_| safe_database_error("read PostgreSQL freshness"))?;
    let observed_at = Utc::now();
    let lag_seconds = data_as_of.map(|value| {
        observed_at
            .signed_duration_since(value)
            .num_seconds()
            .max(0) as u64
    });

    Ok(FreshnessObservation {
        source_id: source_id.clone(),
        relation: relation_name,
        observed_at,
        data_as_of,
        lag_seconds,
    })
}

#[async_trait]
impl QueryPreflightReader for PostgresConnector {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        self.build_preflight(request).await
    }
}

#[async_trait]
impl CatalogReader for PostgresConnector {
    async fn observe_schema(&self, source_id: &SourceId) -> CoreResult<ObservedSchema> {
        ensure_source(&self.config.source_id, source_id)?;
        inspect_postgres_catalog(&self.pool, source_id, &self.result_policy).await
    }
}

#[async_trait]
impl SqlQueryExecutor for PostgresConnector {
    async fn execute_query(&self, request: QueryRequest) -> CoreResult<QueryResult> {
        Ok(self.execute_governed(request, None).await?.model_result)
    }
}

#[async_trait]
impl FreshnessReader for PostgresConnector {
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
        read_postgres_freshness(&self.pool, source_id, relation, time_column).await
    }
}

fn preflight_from_policy(request: &QueryRequest, policy: &SqlPolicyDecision) -> QueryPreflight {
    QueryPreflight {
        sql: request.sql.clone(),
        decision: QueryPreflightDecision::Rejected,
        cost: QueryCostEstimate {
            estimated_cost_units: None,
            scanned_bytes: None,
            estimator_version: None,
        },
        reason_codes: policy
            .reasons
            .iter()
            .map(|reason| reason.code.clone())
            .collect(),
        warnings: Vec::new(),
    }
}

async fn set_local_statement_timeout(
    transaction: &mut Transaction<'_, Postgres>,
    timeout_ms: u64,
) -> CoreResult<()> {
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{timeout_ms}ms"))
        .execute(&mut **transaction)
        .await
        .map_err(|_| safe_database_error("set local PostgreSQL statement timeout"))?;
    Ok(())
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

fn quote_qualified_identifier(identifier: &str) -> CoreResult<String> {
    identifier
        .split('.')
        .map(quote_identifier)
        .collect::<CoreResult<Vec<_>>>()
        .map(|parts| parts.join("."))
}

fn quote_identifier(identifier: &str) -> CoreResult<String> {
    let mut characters = identifier.chars();
    let first_is_safe = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    let rest_is_safe =
        characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if !first_is_safe || !rest_is_safe {
        return Err(CoreError::validation(
            "unsafe_identifier",
            format!("identifier {identifier:?} is not allowed"),
        ));
    }
    Ok(format!("\"{}\"", identifier.replace('"', "\"\"")))
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn cost_to_units(cost: f64) -> Option<u64> {
    if !cost.is_finite() || cost < 0.0 {
        None
    } else if cost >= u64::MAX as f64 {
        Some(u64::MAX)
    } else {
        Some(cost.ceil() as u64)
    }
}

fn safe_query_tag(candidate: &str) -> String {
    let candidate = candidate.trim();
    if !candidate.is_empty()
        && candidate.len() <= 64
        && candidate
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        candidate.to_owned()
    } else {
        uuid::Uuid::new_v4().to_string()
    }
}

fn safe_database_error(context: &'static str) -> CoreError {
    CoreError::validation("postgres_connector_error", format!("{context} failed"))
}
