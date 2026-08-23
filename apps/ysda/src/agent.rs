use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use ys_agent_adapters::{
    ResultPolicy, SqlReadOnlyPolicy, SqliteConnector, SqliteConnectorConfig, SupportedDialect,
};
use ys_agent_core::{
    CatalogReader, CellValue as CoreCellValue, QueryBudget, QueryRequest, SourceId,
    SqlQueryExecutor, WorkspaceId,
};

use crate::domain::{
    AgentRun, CellValue, ColumnSchema, PolicyDecision, QueryResult, RunErrorRecord, RunEvent,
    SchemaSnapshot, TableSchema, UserQuestion,
};
use crate::error::{AppError, AppResult};
use crate::llm::LlmClient;
use crate::trace::TraceRecorder;

pub struct QueryAgent {
    llm: LlmClient,
    traces: TraceRecorder,
    max_rows: usize,
    source_id: SourceId,
    result_policy: ResultPolicy,
}

impl QueryAgent {
    pub fn new(
        llm: LlmClient,
        traces: TraceRecorder,
        max_rows: usize,
        source_id: SourceId,
        result_policy: ResultPolicy,
    ) -> Self {
        Self {
            llm,
            traces,
            max_rows,
            source_id,
            result_policy,
        }
    }

    fn connector(&self, database: &Path) -> SqliteConnector {
        SqliteConnector::new(
            SqliteConnectorConfig {
                source_id: self.source_id.clone(),
                database_path: database.to_path_buf(),
                max_concurrency: 1,
                freshness_columns: BTreeMap::new(),
            },
            SqlReadOnlyPolicy::new(
                SupportedDialect::SQLite,
                QueryBudget::default().max_sql_bytes,
            ),
            self.result_policy.clone(),
        )
    }

    pub async fn inspect(
        database: &Path,
        source_id: SourceId,
        result_policy: ResultPolicy,
    ) -> AppResult<SchemaSnapshot> {
        let connector = SqliteConnector::new(
            SqliteConnectorConfig {
                source_id: source_id.clone(),
                database_path: database.to_path_buf(),
                max_concurrency: 1,
                freshness_columns: BTreeMap::new(),
            },
            SqlReadOnlyPolicy::new(
                SupportedDialect::SQLite,
                QueryBudget::default().max_sql_bytes,
            ),
            result_policy,
        );
        let observed = connector
            .observe_schema(&source_id)
            .await
            .map_err(adapter_error)?;
        Ok(to_legacy_schema(observed))
    }

    pub async fn run(&self, database: &Path, question: UserQuestion) -> AppResult<AgentRun> {
        let started = Instant::now();
        let mut run = AgentRun::new(question);
        let connector = self.connector(database);

        let observed = match connector.observe_schema(&self.source_id).await {
            Ok(schema) => schema,
            Err(error) => {
                return self.finish_failure(run, "schema", started, adapter_error(error));
            }
        };
        let schema = to_legacy_schema(observed);
        run.events.push(event(
            "schema",
            started,
            format!("inspected {} table(s)", schema.tables.len()),
        ));
        run.schema = Some(schema.clone());

        let generated = match self.llm.generate(&run.question, &schema).await {
            Ok(generated) => generated,
            Err(error) => return self.finish_failure(run, "llm", started, error),
        };
        run.events.push(event(
            "llm",
            started,
            "received structured query".to_owned(),
        ));
        run.generated_query = Some(generated.clone());

        let scope = match self
            .result_policy
            .allowed_scope(WorkspaceId::new(), &self.source_id)
        {
            Ok(scope) => scope,
            Err(error) => {
                return self.finish_failure(run, "policy", started, adapter_error(error));
            }
        };
        let request = QueryRequest {
            source_id: self.source_id.clone(),
            sql: generated.sql,
            budget: QueryBudget {
                max_rows: self.max_rows,
                ..QueryBudget::default()
            },
            query_tag: run.run_id.to_string(),
            scope,
            confirmation_granted: false,
        };

        let result = match connector.execute_query(request).await {
            Ok(result) => result,
            Err(error) if is_policy_rejection(error.code()) => {
                run.policy_decision = Some(PolicyDecision::deny(error.to_string()));
                return self.finish_failure(
                    run,
                    "policy",
                    started,
                    AppError::UnsafeSql(error.to_string()),
                );
            }
            Err(error) => {
                return self.finish_failure(run, "execute", started, adapter_error(error));
            }
        };

        run.policy_decision = Some(PolicyDecision::allow());
        run.events
            .push(event("policy", started, "read-only SQL allowed".to_owned()));
        run.events.push(event(
            "execute",
            started,
            format!("returned {} row(s)", result.row_count),
        ));
        run.result = Some(to_legacy_result(result));
        self.traces.save(&run)?;
        Ok(run)
    }

    fn finish_failure(
        &self,
        mut run: AgentRun,
        stage: &str,
        started: Instant,
        error: AppError,
    ) -> AppResult<AgentRun> {
        run.events
            .push(event(stage, started, "stage failed".to_owned()));
        run.error = Some(RunErrorRecord {
            category: error.category().to_owned(),
            message: error.to_string(),
        });
        self.traces.save(&run)?;
        Ok(run)
    }
}

fn to_legacy_schema(schema: ys_agent_core::ObservedSchema) -> SchemaSnapshot {
    SchemaSnapshot {
        tables: schema
            .relations
            .into_iter()
            .map(|relation| TableSchema {
                name: relation.name,
                columns: relation
                    .columns
                    .into_iter()
                    .map(|column| ColumnSchema {
                        name: column.name,
                        data_type: column.data_type,
                        not_null: !column.nullable,
                        primary_key_position: column.primary_key_position.unwrap_or(0),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn to_legacy_result(result: ys_agent_core::QueryResult) -> QueryResult {
    QueryResult {
        columns: result.columns,
        rows: result
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(to_legacy_cell).collect())
            .collect(),
        row_count: result.row_count,
        truncated: result.truncated,
    }
}

fn to_legacy_cell(value: CoreCellValue) -> CellValue {
    match value {
        CoreCellValue::Null => CellValue::Null,
        CoreCellValue::Boolean(value) => CellValue::Text(value.to_string()),
        CoreCellValue::Integer(value) => CellValue::Integer(value),
        CoreCellValue::Real(value) => CellValue::Real(value),
        CoreCellValue::Text(value) => CellValue::Text(value),
        CoreCellValue::BlobSummary { bytes } => CellValue::Blob(format!("<{bytes} bytes>")),
    }
}

fn is_policy_rejection(code: &str) -> bool {
    matches!(
        code,
        "sql_too_large"
            | "sql_parse_error"
            | "statement_count_invalid"
            | "statement_not_read_only"
            | "locking_query_rejected"
            | "mutating_subquery_rejected"
            | "select_into_rejected"
            | "dynamic_relation_rejected"
            | "function_not_allowed"
            | "relation_not_allowed"
            | "column_not_allowed"
            | "column_denied"
            | "wildcard_includes_denied_column"
    )
}

fn adapter_error(error: ys_agent_core::CoreError) -> AppError {
    AppError::DataAdapter(error.to_string())
}

fn event(stage: &str, started: Instant, message: String) -> RunEvent {
    RunEvent {
        stage: stage.to_owned(),
        elapsed_ms: started.elapsed().as_millis(),
        message,
    }
}
