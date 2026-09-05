use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures::TryStreamExt;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow, PgSslMode};
use sqlx::{AssertSqlSafe, Column, PgPool, Postgres, Row, Transaction, TypeInfo, ValueRef};
use ys_agent_core::{
    CapabilityDescriptor, CatalogReader, CellValue, CoreError, CoreResult, FreshnessObservation,
    FreshnessReader, ObservedColumn, ObservedRelation, ObservedSchema, QueryCostEstimate,
    QueryParameter, QueryPreflight, QueryPreflightDecision, QueryPreflightReader, QueryRequest,
    QueryResult, SchemaKnowledgeKind, SourceId, SqlQueryExecutor,
};
use ys_agent_core::{
    ConnectorFactory, ConnectorOpenInput, DatabaseContext, DatasourceGovernanceContext,
    DatasourceRevision, DsError, DsErrorCode, DsRemediation, DsResult, FieldId, FieldIssue,
    FieldIssueCode, FieldValue, ManagedConnector, ProbeEvidence, QueryBudget,
    validate_datasource_fields,
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
    managed: Option<Arc<ManagedPostgresState>>,
}

struct ManagedPostgresState {
    governance: DatasourceGovernanceContext,
    database: String,
    schema: String,
    username: String,
    closed: AtomicBool,
}

impl PostgresConnector {
    async fn connect_managed(
        config: PostgresConnectorConfig,
        options: PgConnectOptions,
        state: Arc<ManagedPostgresState>,
    ) -> DsResult<Self> {
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
            .map_err(classify_connect_error)?;
        Ok(Self {
            sql_policy: SqlReadOnlyPolicy::new(
                super::SupportedDialect::Postgres,
                state.governance.budget.max_sql_bytes,
            ),
            result_policy: ResultPolicy::from_scope(&state.governance.data_scope),
            config,
            pool,
            managed: Some(state),
        })
    }

    fn managed_state(&self) -> CoreResult<&ManagedPostgresState> {
        let state = self.managed.as_deref().ok_or_else(|| {
            CoreError::validation("datasource_policy_denied", "managed operation required")
        })?;
        if state.closed.load(Ordering::Acquire) || self.pool.is_closed() {
            return Err(CoreError::validation(
                "datasource_closed",
                "datasource operation rejected",
            ));
        }
        Ok(state)
    }

    fn validate_managed_request(&self, request: &QueryRequest) -> CoreResult<QueryBudget> {
        let state = self.managed_state()?;
        if request.scope != state.governance.data_scope {
            return Err(CoreError::validation(
                "datasource_policy_denied",
                "datasource operation rejected",
            ));
        }
        let budget = limit_budget(&request.budget, &state.governance.budget);
        if budget.max_sql_bytes == 0
            || budget.statement_timeout_ms == 0
            || budget.acquire_timeout_ms == 0
            || budget.max_rows == 0
            || budget.max_result_bytes == 0
            || budget.max_concurrency == 0
        {
            return Err(CoreError::validation(
                "datasource_timeout",
                "datasource budget cannot be zero",
            ));
        }
        Ok(budget)
    }

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
            managed: None,
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
            preflight_reader: true,
            read_only_mechanism: Some(ys_agent_core::ReadOnlyMechanism::TransactionReadOnly),
            max_concurrency: self.config.max_connections as usize,
        }
    }

    async fn build_preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        let effective;
        let request = if self.managed.is_some() {
            effective = {
                let mut request = request.clone();
                request.budget = self.validate_managed_request(&request)?;
                request
            };
            &effective
        } else {
            request
        };
        validate_request_source(&self.config.source_id, request)?;
        if request.sql.len() > request.budget.max_sql_bytes {
            return Ok(preflight_rejected(request, "sql_too_large"));
        }
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
        let mut transaction =
            tokio::time::timeout(timeout, self.pool.begin_with("BEGIN READ ONLY"))
                .await
                .map_err(|_| {
                    CoreError::validation(
                        "connection_acquire_timeout",
                        "timed out while acquiring a PostgreSQL connection",
                    )
                })?
                .map_err(|_| safe_database_error("begin read-only PostgreSQL transaction"))?;
        if let Some(state) = &self.managed {
            set_local_context(&mut transaction, &state.schema).await?;
        }
        Ok(transaction)
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
            .map_err(|error| safe_query_error("stream PostgreSQL rows", &error))?
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
        mut request: QueryRequest,
        restricted_context: Option<&RestrictedResultContext>,
    ) -> CoreResult<GovernedQueryResult> {
        if self.managed.is_some() {
            request.budget = self.validate_managed_request(&request)?;
        }
        validate_request_source(&self.config.source_id, &request)?;
        if request.sql.len() > request.budget.max_sql_bytes {
            return Err(CoreError::validation(
                "sql_too_large",
                "SQL exceeds the governed byte budget",
            ));
        }
        let policy_decision = self.sql_policy.evaluate(&request.sql, &request.scope);
        policy_decision.ensure_allowed()?;

        let started = Instant::now();
        let mut transaction = self.begin_read_only(&request).await?;
        let remaining = Duration::from_millis(request.budget.statement_timeout_ms)
            .checked_sub(started.elapsed())
            .ok_or_else(|| {
                CoreError::validation("datasource_timeout", "datasource operation timed out")
            })?;
        set_local_statement_timeout(&mut transaction, duration_millis(remaining)).await?;
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

pub struct PostgresConnectorFactory;

struct ManagedPostgresConfig {
    host: String,
    port: u16,
    database: String,
    schema: String,
    username: String,
    tls: PgSslMode,
    source_id: SourceId,
}

#[async_trait]
impl ConnectorFactory for PostgresConnectorFactory {
    fn validate_config(&self, revision: &DatasourceRevision) -> Vec<FieldIssue> {
        let descriptor =
            super::catalog::builtin_descriptor("postgres").expect("static PostgreSQL descriptor");
        let input = revision.input();
        let mut issues = validate_datasource_fields(
            &descriptor.fields,
            &input.fields,
            input.credential.is_some(),
            true,
        );
        if input.adapter_id != descriptor.adapter_id
            || input.adapter_version != descriptor.adapter_version
            || input.config_version != descriptor.config_version
            || parse_managed_config(revision).is_err()
        {
            issues.push(FieldIssue {
                field: FieldId::new("host").expect("static field"),
                code: FieldIssueCode::Invalid,
            });
        }
        issues
    }

    async fn open(&self, input: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>> {
        if !self.validate_config(&input.revision).is_empty() {
            return Err(ds_failure(DsErrorCode::InvalidField));
        }
        let parsed = parse_managed_config(&input.revision)?;
        let reference = input
            .revision
            .input()
            .credential
            .ok_or_else(|| ds_failure(DsErrorCode::CredentialMissing))?;
        let secret = input
            .secret
            .ok_or_else(|| ds_failure(DsErrorCode::CredentialMissing))?;
        if secret.reference != reference {
            return Err(ds_failure(DsErrorCode::CredentialExpired));
        }
        let revision = input.revision.input();
        if revision.workspace_id != input.governance.data_scope.workspace_id
            || parsed.source_id.as_str() != input.governance.data_scope.source_id
            || input.governance.result_policy != input.governance.data_scope.relations
            || input.governance.data_scope.relations.is_empty()
            || !input.governance.allowed_roots.is_empty()
            || input.governance.budget.max_concurrency == 0
            || input.governance.budget.max_concurrency > 2
            || !scope_matches_schema(&input.governance, &parsed.schema)
        {
            return Err(ds_failure(DsErrorCode::PolicyDenied));
        }
        let options = secret.value.with_exposed(|password| {
            PgConnectOptions::new()
                .host(&parsed.host)
                .port(parsed.port)
                .database(&parsed.database)
                .username(&parsed.username)
                .password(password)
                .ssl_mode(parsed.tls)
                .application_name("ysda")
        });
        let state = Arc::new(ManagedPostgresState {
            governance: input.governance,
            database: parsed.database,
            schema: parsed.schema,
            username: parsed.username,
            closed: AtomicBool::new(false),
        });
        let connector = PostgresConnector::connect_managed(
            PostgresConnectorConfig {
                source_id: parsed.source_id,
                max_connections: state.governance.budget.max_concurrency as u32,
                acquire_timeout: Duration::from_millis(state.governance.budget.acquire_timeout_ms),
                default_statement_timeout: Duration::from_millis(
                    state.governance.budget.statement_timeout_ms,
                ),
                confirmation_cost_units: state
                    .governance
                    .budget
                    .max_estimated_cost_units
                    .unwrap_or(u64::MAX),
                freshness_columns: BTreeMap::new(),
            },
            options,
            state,
        )
        .await?;
        Ok(Arc::new(connector))
    }
}

fn parse_managed_config(revision: &DatasourceRevision) -> DsResult<ManagedPostgresConfig> {
    let input = revision.input();
    let text = |name: &str| match input.fields.get(&FieldId::new(name).expect("static field")) {
        Some(FieldValue::Text(value)) => Some(value.clone()),
        _ => None,
    };
    let host = text("host").ok_or_else(|| ds_failure(DsErrorCode::InvalidField))?;
    let database = text("database").ok_or_else(|| ds_failure(DsErrorCode::InvalidField))?;
    let schema = text("schema").ok_or_else(|| ds_failure(DsErrorCode::InvalidField))?;
    let username = text("username").ok_or_else(|| ds_failure(DsErrorCode::InvalidField))?;
    let port = match input
        .fields
        .get(&FieldId::new("port").expect("static field"))
    {
        Some(FieldValue::Integer(port)) => {
            u16::try_from(*port).map_err(|_| ds_failure(DsErrorCode::InvalidField))?
        }
        _ => return Err(ds_failure(DsErrorCode::InvalidField)),
    };
    let tls = match text("tls").as_deref() {
        Some("disable") => PgSslMode::Disable,
        Some("require") => PgSslMode::Require,
        Some("verify_full") => PgSslMode::VerifyFull,
        _ => return Err(ds_failure(DsErrorCode::InvalidField)),
    };
    if host.is_empty()
        || host.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '/' | '\\' | '@' | '?' | '#')
        })
        || host.contains("://")
        || !safe_identifier(&database)
        || !safe_identifier(&schema)
        || !safe_identifier(&username)
    {
        return Err(ds_failure(DsErrorCode::InvalidField));
    }
    let DatabaseContext::Database {
        catalog,
        database: context_database,
        schema: context_schema,
    } = &input.context
    else {
        return Err(ds_failure(DsErrorCode::InvalidField));
    };
    if catalog.as_deref() != Some(format!("{host}:{port}").as_str())
        || context_database != &database
        || context_schema != &schema
    {
        return Err(ds_failure(DsErrorCode::InvalidField));
    }
    let source_id = input
        .source_id
        .clone()
        .ok_or_else(|| ds_failure(DsErrorCode::PolicyDenied))?;
    Ok(ManagedPostgresConfig {
        host,
        port,
        database,
        schema,
        username,
        tls,
        source_id,
    })
}

fn safe_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn scope_matches_schema(governance: &DatasourceGovernanceContext, schema: &str) -> bool {
    governance.data_scope.relations.keys().all(|relation| {
        relation
            .split_once('.')
            .is_some_and(|(prefix, table)| prefix == schema && safe_identifier(table))
    })
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

fn ds_failure(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::CredentialMissing
            | DsErrorCode::CredentialExpired
            | DsErrorCode::AuthenticationFailed => DsRemediation::ReplaceCredential,
            DsErrorCode::Timeout => DsRemediation::Retry,
            DsErrorCode::Network | DsErrorCode::Protocol => DsRemediation::CheckConnectivity,
            DsErrorCode::PermissionDenied
            | DsErrorCode::PolicyDenied
            | DsErrorCode::ReadOnlyUnproven => DsRemediation::RepairPolicy,
            _ => DsRemediation::EditConfiguration,
        },
        operation_id: None,
    }
}

fn classify_connect_error(error: sqlx::Error) -> DsError {
    let code = match &error {
        sqlx::Error::Database(database) => match database.code().as_deref() {
            Some("28P01" | "28000") => DsErrorCode::AuthenticationFailed,
            Some("3D000") => DsErrorCode::TargetMissing,
            Some("42501") => DsErrorCode::PermissionDenied,
            Some(code) if code.starts_with("08") => DsErrorCode::Network,
            _ => DsErrorCode::Protocol,
        },
        sqlx::Error::Tls(_) => DsErrorCode::Protocol,
        sqlx::Error::Io(_) | sqlx::Error::PoolTimedOut => DsErrorCode::Network,
        sqlx::Error::Configuration(_) | sqlx::Error::InvalidArgument(_) => {
            DsErrorCode::InvalidField
        }
        _ => DsErrorCode::Protocol,
    };
    ds_failure(code)
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
            return Err(CoreError::validation(
                "unsupported_postgres_type",
                "PostgreSQL result type is not supported without loss",
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
        if self.managed.is_some() {
            self.managed_state()?;
        }
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
        if self.managed.is_some() {
            self.managed_state()?;
        }
        ensure_source(&self.config.source_id, source_id)?;
        let scope = self
            .result_policy
            .allowed_scope(ys_agent_core::WorkspaceId::new(), source_id)?;
        ensure_freshness_scope(&scope, relation, time_column)?;
        read_postgres_freshness(&self.pool, source_id, relation, time_column).await
    }
}

#[async_trait]
impl ManagedConnector for PostgresConnector {
    async fn probe(&self) -> DsResult<ProbeEvidence> {
        let state = self.managed_state().map_err(classify_core_error)?;
        let request = QueryRequest {
            source_id: self.config.source_id.clone(),
            sql: "SELECT 1".into(),
            parameters: Vec::new(),
            budget: state.governance.budget.clone(),
            query_tag: "datasource-probe".into(),
            scope: state.governance.data_scope.clone(),
            confirmation_granted: true,
        };
        let mut transaction = self
            .begin_read_only(&request)
            .await
            .map_err(classify_core_error)?;
        set_local_statement_timeout(
            &mut transaction,
            state.governance.budget.statement_timeout_ms.min(10_000),
        )
        .await
        .map_err(classify_core_error)?;

        let (database, username, read_only, schema_exists): (String, String, bool, bool) =
            sqlx::query_as(
                "SELECT current_database(), current_user, current_setting('transaction_read_only') = 'on', to_regnamespace($1) IS NOT NULL",
            )
            .bind(&state.schema)
            .fetch_one(&mut *transaction)
            .await
            .map_err(classify_connect_error)?;
        if database != state.database || username != state.username || !schema_exists {
            return Err(ds_failure(DsErrorCode::TargetMissing));
        }
        if !read_only {
            return Err(ds_failure(DsErrorCode::ReadOnlyUnproven));
        }

        let dangerous_role: bool = sqlx::query_scalar(
            r#"
WITH RECURSIVE inherited AS (
  SELECT oid, rolname, rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls
  FROM pg_roles WHERE rolname = current_user
  UNION
  SELECT parent.oid, parent.rolname, parent.rolsuper, parent.rolcreaterole,
         parent.rolcreatedb, parent.rolreplication, parent.rolbypassrls
  FROM inherited child
  JOIN pg_auth_members membership ON membership.member = child.oid
  JOIN pg_roles parent ON parent.oid = membership.roleid
)
SELECT COALESCE(bool_or(rolsuper OR rolcreaterole OR rolcreatedb OR rolreplication
                       OR rolbypassrls OR rolname LIKE 'pg\_%' ESCAPE '\'), false)
FROM inherited
"#,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(classify_connect_error)?;
        let broad_target_privilege: bool = sqlx::query_scalar(
            "SELECT has_schema_privilege(current_user, $1, 'CREATE') OR has_database_privilege(current_user, current_database(), 'CREATE') OR has_database_privilege(current_user, current_database(), 'TEMP')",
        )
        .bind(&state.schema)
        .fetch_one(&mut *transaction)
        .await
        .map_err(classify_connect_error)?;
        if dangerous_role || broad_target_privilege {
            return Err(ds_failure(DsErrorCode::PermissionDenied));
        }

        for (relation, columns) in &state.governance.data_scope.relations {
            let (schema, table) = relation
                .split_once('.')
                .ok_or_else(|| ds_failure(DsErrorCode::PolicyDenied))?;
            let row: Option<(String, bool, bool, bool)> = sqlx::query_as(
                r#"
SELECT c.relkind::text,
       pg_has_role(current_user, c.relowner, 'MEMBER'),
       has_table_privilege(current_user, c.oid, 'SELECT'),
       has_table_privilege(current_user, c.oid, 'INSERT')
       OR has_table_privilege(current_user, c.oid, 'UPDATE')
       OR has_table_privilege(current_user, c.oid, 'DELETE')
       OR has_table_privilege(current_user, c.oid, 'TRUNCATE')
       OR has_table_privilege(current_user, c.oid, 'REFERENCES')
       OR has_table_privilege(current_user, c.oid, 'TRIGGER')
       OR has_any_column_privilege(current_user, c.oid, 'INSERT')
       OR has_any_column_privilege(current_user, c.oid, 'UPDATE')
       OR has_any_column_privilege(current_user, c.oid, 'REFERENCES')
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relname = $2
"#,
            )
            .bind(schema)
            .bind(table)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(classify_connect_error)?;
            let Some((kind, owns, table_select, writes)) = row else {
                return Err(ds_failure(DsErrorCode::TargetMissing));
            };
            if !matches!(kind.as_str(), "r" | "p") || owns || writes {
                return Err(ds_failure(DsErrorCode::PermissionDenied));
            }
            for (column, action) in columns {
                if *action == ys_agent_core::ColumnPolicy::Deny {
                    continue;
                }
                let column_select: bool = sqlx::query_scalar(
                    "SELECT has_column_privilege(current_user, $1, $2, 'SELECT')",
                )
                .bind(relation)
                .bind(column)
                .fetch_one(&mut *transaction)
                .await
                .map_err(classify_connect_error)?;
                if !table_select && !column_select {
                    return Err(ds_failure(DsErrorCode::PermissionDenied));
                }
            }
        }

        let sequence_write: bool = sqlx::query_scalar(
            r#"
SELECT COALESCE(bool_or(
    pg_has_role(current_user, c.relowner, 'MEMBER')
    OR has_sequence_privilege(current_user, c.oid, 'USAGE')
    OR has_sequence_privilege(current_user, c.oid, 'UPDATE')
), false)
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind = 'S' AND n.nspname = $1
"#,
        )
        .bind(&state.schema)
        .fetch_one(&mut *transaction)
        .await
        .map_err(classify_connect_error)?;
        if sequence_write {
            return Err(ds_failure(DsErrorCode::PermissionDenied));
        }
        transaction
            .rollback()
            .await
            .map_err(classify_connect_error)?;
        Ok(ProbeEvidence {
            authenticated: true,
            target_verified: true,
            read_only_verified: true,
            least_privilege_verified: true,
            capabilities_verified: true,
        })
    }

    async fn close(&self) -> DsResult<()> {
        let state = self
            .managed
            .as_ref()
            .ok_or_else(|| ds_failure(DsErrorCode::PolicyDenied))?;
        state.closed.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(2), self.pool.close())
            .await
            .map_err(|_| ds_failure(DsErrorCode::Timeout))?;
        Ok(())
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

fn preflight_rejected(request: &QueryRequest, code: &str) -> QueryPreflight {
    QueryPreflight {
        sql: request.sql.clone(),
        decision: QueryPreflightDecision::Rejected,
        cost: QueryCostEstimate {
            estimated_cost_units: None,
            scanned_bytes: None,
            estimator_version: None,
        },
        reason_codes: vec![code.to_owned()],
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
        .map_err(|error| safe_query_error("set local PostgreSQL statement timeout", &error))?;
    Ok(())
}

async fn set_local_context(
    transaction: &mut Transaction<'_, Postgres>,
    schema: &str,
) -> CoreResult<()> {
    let search_path = format!("pg_catalog, \"{schema}\"");
    sqlx::query("SELECT set_config('search_path', $1, true)")
        .bind(search_path)
        .execute(&mut **transaction)
        .await
        .map_err(|error| safe_query_error("set PostgreSQL target schema", &error))?;
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

fn safe_query_error(context: &'static str, error: &sqlx::Error) -> CoreError {
    let code = match error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("57014") => {
            "datasource_timeout"
        }
        sqlx::Error::Database(database) if database.code().as_deref() == Some("42501") => {
            "datasource_permission_denied"
        }
        sqlx::Error::PoolTimedOut => "connection_acquire_timeout",
        sqlx::Error::PoolClosed => "datasource_closed",
        _ => "postgres_connector_error",
    };
    CoreError::validation(code, format!("{context} failed"))
}

fn classify_core_error(error: CoreError) -> DsError {
    ds_failure(match error.code() {
        "connection_acquire_timeout" | "datasource_timeout" => DsErrorCode::Timeout,
        "datasource_closed" => DsErrorCode::Cancelled,
        "source_mismatch" | "scope_source_mismatch" | "datasource_policy_denied" => {
            DsErrorCode::PolicyDenied
        }
        _ => DsErrorCode::Protocol,
    })
}
