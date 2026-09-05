use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ys_agent_core::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactId, ArtifactKind, ArtifactMetadata,
    ArtifactStore, CoreError, CoreResult, CostClass, MetricDefinition, MetricProvider, PutArtifact,
    QueryExecutionPlan, QueryParameter, QueryPlan, QueryPreflight, QueryPreflightDecision,
    QueryRequest, QueryResult, RetentionPolicy, SemanticStatus, Sensitivity, SideEffect, SourceId,
    Tool, ToolExecutionContext, ToolFailureCategory, ToolOutcome, ToolRisk, ToolSpec,
};

use super::{
    ArtifactLookup, ConnectorHandle, ConnectorRegistry, MetricSqlDialect, failed, parse_arguments,
    put_json, rejected, safe_internal_failure,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryDataInput {
    Preflight {
        plan_artifact_id: ArtifactId,
        plan_hash: String,
    },
    Execute {
        plan_artifact_id: ArtifactId,
        plan_hash: String,
        preflight_artifact_id: ArtifactId,
        preflight_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledQuery {
    pub source_id: SourceId,
    pub dialect: MetricSqlDialect,
    pub sql: String,
    pub parameters: Vec<QueryParameter>,
    pub source_relations: Vec<String>,
    pub metric_id: Option<String>,
    pub metric_version: Option<String>,
    pub semantic_status: SemanticStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct QueryPreflightEvidence {
    schema_version: u32,
    plan_artifact_id: ArtifactId,
    plan_hash: String,
    budget_hash: String,
    scope_hash: String,
    compiled: CompiledQuery,
    connector_preflight: QueryPreflight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct QueryResultEvidence {
    schema_version: u32,
    plan_artifact_id: ArtifactId,
    preflight_artifact_id: ArtifactId,
    compiled: CompiledQuery,
    result: QueryResult,
}

pub struct QueryDataTool {
    connectors: ConnectorRegistry,
    metrics: Arc<dyn MetricProvider>,
    artifact_lookup: Arc<dyn ArtifactLookup>,
    artifact_store: Arc<dyn ArtifactStore>,
}

impl QueryDataTool {
    pub fn new(
        connectors: ConnectorRegistry,
        metrics: Arc<dyn MetricProvider>,
        artifact_lookup: Arc<dyn ArtifactLookup>,
        artifact_store: Arc<dyn ArtifactStore>,
    ) -> Self {
        Self {
            connectors,
            metrics,
            artifact_lookup,
            artifact_store,
        }
    }
}

impl QueryDataTool {
    async fn compile_plan(
        &self,
        context: &ToolExecutionContext,
        plan: &QueryPlan,
        connector: &ConnectorHandle,
    ) -> Result<CompiledQuery, ToolOutcome> {
        if plan.source_id.as_str() != context.data_scope.source_id {
            return Err(rejected(
                "plan_source_not_allowed",
                ToolFailureCategory::Authorization,
                "QueryPlan source is outside the current allowed scope",
                false,
                CostClass::High,
            ));
        }

        match &plan.execution {
            QueryExecutionPlan::Metric {
                metric_id,
                start,
                end,
                dimensions,
            } => {
                let metric = self
                    .metrics
                    .get_metric(metric_id)
                    .await
                    .map_err(|error| safe_internal_failure(&error, CostClass::Low))?
                    .ok_or_else(|| {
                        rejected(
                            "metric_not_found_or_inactive",
                            ToolFailureCategory::Governance,
                            "Metric plan does not reference an Active contract",
                            true,
                            CostClass::Low,
                        )
                    })?;
                if !context
                    .data_scope
                    .relations
                    .contains_key(&metric.source_relation)
                {
                    return Err(rejected(
                        "metric_relation_not_allowed",
                        ToolFailureCategory::Authorization,
                        "Metric relation is outside the allowed scope",
                        false,
                        CostClass::High,
                    ));
                }
                MetricSqlCompiler::new(connector.dialect)
                    .compile(plan.source_id.clone(), &metric, *start, *end, dimensions)
                    .map_err(|error| {
                        rejected(
                            error.code(),
                            ToolFailureCategory::Governance,
                            error.to_string(),
                            true,
                            CostClass::Low,
                        )
                    })
            }
            QueryExecutionPlan::AdHoc {
                sql,
                assumption_refs,
            } => {
                if assumption_refs.is_empty() {
                    return Err(rejected(
                        "adhoc_assumptions_required",
                        ToolFailureCategory::Governance,
                        "AdHoc QueryPlan needs at least one assumption Artifact",
                        true,
                        CostClass::Low,
                    ));
                }
                if sql.trim().is_empty() || sql.len() > context.query_budget.max_sql_bytes {
                    return Err(rejected(
                        "adhoc_sql_budget_invalid",
                        ToolFailureCategory::Budget,
                        "AdHoc SQL is empty or exceeds the SQL byte budget",
                        true,
                        CostClass::Low,
                    ));
                }
                self.validate_assumptions(context, assumption_refs).await?;
                Ok(CompiledQuery {
                    source_id: plan.source_id.clone(),
                    dialect: connector.dialect,
                    sql: sql.clone(),
                    parameters: Vec::new(),
                    source_relations: Vec::new(),
                    metric_id: None,
                    metric_version: None,
                    semantic_status: SemanticStatus::Inferred,
                })
            }
        }
    }

    async fn validate_assumptions(
        &self,
        context: &ToolExecutionContext,
        assumption_refs: &[ArtifactId],
    ) -> Result<(), ToolOutcome> {
        for artifact_id in assumption_refs {
            let record = self
                .artifact_lookup
                .load(artifact_id, &access_context(context))
                .await
                .map_err(|error| artifact_rejection(&error))?;
            if record.metadata.workspace_id != context.workspace_id
                || record.metadata.task_id != context.task_id
                || record.metadata.kind != ArtifactKind::ContextEvidence
            {
                return Err(rejected(
                    "invalid_assumption_artifact",
                    ToolFailureCategory::Governance,
                    "AdHoc assumption reference is not authorized Context Evidence",
                    true,
                    CostClass::Low,
                ));
            }
        }
        Ok(())
    }
}

fn artifact_rejection(error: &CoreError) -> ToolOutcome {
    rejected(
        error.code(),
        ToolFailureCategory::Policy,
        "Immutable query Artifact could not be validated",
        false,
        CostClass::Low,
    )
}

fn query_request(context: &ToolExecutionContext, compiled: &CompiledQuery) -> QueryRequest {
    QueryRequest {
        source_id: compiled.source_id.clone(),
        sql: compiled.sql.clone(),
        parameters: compiled.parameters.clone(),
        budget: context.query_budget.clone(),
        query_tag: format!("ysda:{}:{}", context.run_id, context.call_id),
        scope: context.data_scope.clone(),
        confirmation_granted: context.confirmation_granted,
    }
}

fn preflight_rejection(preflight: &QueryPreflight) -> ToolOutcome {
    rejected(
        "query_preflight_rejected",
        ToolFailureCategory::Policy,
        if preflight.reason_codes.is_empty() {
            "Connector policy rejected the query".to_owned()
        } else {
            format!(
                "Connector policy rejected the query: {}",
                preflight.reason_codes.join(",")
            )
        },
        true,
        CostClass::Low,
    )
}

impl QueryDataTool {
    async fn preflight_action(
        &self,
        context: &ToolExecutionContext,
        plan_artifact_id: ArtifactId,
        plan_hash: String,
    ) -> CoreResult<ToolOutcome> {
        let plan: QueryPlan = match self
            .load_json(
                context,
                &plan_artifact_id,
                ArtifactKind::QueryPlan,
                &plan_hash,
            )
            .await
        {
            Ok(plan) => plan,
            Err(error) => return Ok(artifact_rejection(&error)),
        };
        let connector = match self.connectors.get(&plan.source_id) {
            Ok(connector) => connector,
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Low)),
        };
        let compiled = match self.compile_plan(context, &plan, &connector).await {
            Ok(compiled) => compiled,
            Err(outcome) => return Ok(outcome),
        };
        let request = query_request(context, &compiled);
        let connector_preflight = match connector.preflight.preflight(&request).await {
            Ok(preflight) => preflight,
            Err(error) => {
                return Ok(failed(
                    error.code(),
                    ToolFailureCategory::Transport,
                    "Connector preflight failed",
                    false,
                    CostClass::Unknown,
                ));
            }
        };
        if connector_preflight.decision == QueryPreflightDecision::Rejected {
            return Ok(preflight_rejection(&connector_preflight));
        }

        let evidence = QueryPreflightEvidence {
            schema_version: 1,
            plan_artifact_id,
            plan_hash,
            budget_hash: stable_hash(&context.query_budget)?,
            scope_hash: stable_hash(&context.data_scope)?,
            compiled,
            connector_preflight,
        };
        let metadata = put_json(
            self.artifact_store.as_ref(),
            PutArtifact {
                workspace_id: context.workspace_id,
                task_id: context.task_id,
                run_id: context.run_id,
                kind: ArtifactKind::QueryPreflight,
                media_type: "application/json".to_owned(),
                bytes: Vec::new(),
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            },
            &evidence,
        )
        .await?;

        Ok(ToolOutcome::Succeeded {
            message: "Created immutable query preflight".to_owned(),
            output: json!({
                "artifact_id": metadata.id,
                "artifact_hash": metadata.content_hash.clone(),
                "decision": evidence.connector_preflight.decision,
                "estimated_cost_units": evidence.connector_preflight.cost.estimated_cost_units,
                "scanned_bytes": evidence.connector_preflight.cost.scanned_bytes,
                "compiled_sql": evidence.compiled.sql,
                "bound_parameters": evidence.compiled.parameters,
                "semantic_status": evidence.compiled.semantic_status,
            }),
            artifacts: vec![metadata],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MetricSqlCompiler {
    dialect: MetricSqlDialect,
}

impl MetricSqlCompiler {
    pub fn new(dialect: MetricSqlDialect) -> Self {
        Self { dialect }
    }

    pub fn compile(
        &self,
        source_id: SourceId,
        metric: &MetricDefinition,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        dimensions: &[String],
    ) -> CoreResult<CompiledQuery> {
        if metric.status != ys_agent_core::MetricStatus::Active {
            return Err(CoreError::validation(
                "metric_not_active",
                "Metric SQL requires an Active contract",
            ));
        }
        if start >= end {
            return Err(CoreError::validation(
                "invalid_time_range",
                "Metric time range must be a non-empty half-open interval",
            ));
        }
        if metric.expression.trim().is_empty() || metric.expression.contains(';') {
            return Err(CoreError::validation(
                "unsafe_metric_expression",
                "Metric expression is empty or contains a statement separator",
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        for dimension in dimensions {
            if !seen.insert(dimension.as_str()) {
                return Err(CoreError::validation(
                    "duplicate_metric_dimension",
                    format!("Metric dimension {dimension} is repeated"),
                ));
            }
            if !metric.allowed_dimensions.contains(dimension) {
                return Err(CoreError::validation(
                    "metric_dimension_not_allowed",
                    format!("Metric dimension {dimension} is not allowed by the contract"),
                ));
            }
        }

        let relation = quote_qualified_identifier(&metric.source_relation)?;
        let time_column = quote_identifier(&metric.time_column)?;
        let quoted_dimensions = dimensions
            .iter()
            .map(|dimension| quote_identifier(dimension))
            .collect::<CoreResult<Vec<_>>>()?;
        let (start_placeholder, end_placeholder) = match self.dialect {
            MetricSqlDialect::Sqlite | MetricSqlDialect::DuckDb => ("?", "?"),
            MetricSqlDialect::Postgres => ("$1", "$2"),
        };
        let sql = metric_sql(
            &quoted_dimensions,
            &metric.expression,
            &relation,
            &time_column,
            start_placeholder,
            end_placeholder,
        );

        Ok(CompiledQuery {
            source_id,
            dialect: self.dialect,
            sql,
            parameters: vec![
                QueryParameter::Timestamp(start),
                QueryParameter::Timestamp(end),
            ],
            source_relations: vec![metric.source_relation.clone()],
            metric_id: Some(metric.id.clone()),
            metric_version: Some(metric.version.clone()),
            semantic_status: SemanticStatus::Confirmed,
        })
    }
}

fn metric_sql(
    dimensions: &[String],
    expression: &str,
    relation: &str,
    time_column: &str,
    start_placeholder: &str,
    end_placeholder: &str,
) -> String {
    let mut select_items = dimensions.to_vec();
    select_items.push(format!("{expression} AS metric_value"));
    let mut sql = format!(
        "SELECT\n    {}\nFROM {relation}\nWHERE {time_column} >= {start_placeholder}\n  AND {time_column} < {end_placeholder}",
        select_items.join(",\n    ")
    );
    if !dimensions.is_empty() {
        let dimensions = dimensions.join(", ");
        sql.push_str(&format!("\nGROUP BY {dimensions}\nORDER BY {dimensions}"));
    }
    sql
}

fn quote_qualified_identifier(value: &str) -> CoreResult<String> {
    value
        .split('.')
        .map(quote_identifier)
        .collect::<CoreResult<Vec<_>>>()
        .map(|parts| parts.join("."))
}

fn quote_identifier(value: &str) -> CoreResult<String> {
    let mut characters = value.chars();
    let first_is_safe = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    let rest_is_safe =
        characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if !first_is_safe || !rest_is_safe {
        return Err(CoreError::validation(
            "unsafe_metric_identifier",
            format!("Metric identifier {value:?} is outside the v0.2 safe subset"),
        ));
    }
    Ok(format!("\"{value}\""))
}

fn stable_hash<T>(value: &T) -> CoreResult<String>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|error| CoreError::validation("hash_serialization_failed", error.to_string()))?;
    let digest = Sha256::digest(bytes);
    Ok(format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn access_context(context: &ToolExecutionContext) -> ArtifactAccessContext {
    ArtifactAccessContext {
        workspace_id: context.workspace_id,
        principal_id: context.principal.id,
        purpose: ArtifactAccessPurpose::RuntimeVerification,
        max_sensitivity: Sensitivity::Restricted,
    }
}

fn verify_artifact_identity(
    metadata: &ArtifactMetadata,
    context: &ToolExecutionContext,
    expected_kind: ArtifactKind,
    expected_hash: &str,
) -> CoreResult<()> {
    if metadata.workspace_id != context.workspace_id
        || metadata.task_id != context.task_id
        || metadata.run_id != context.run_id
    {
        return Err(CoreError::validation(
            "artifact_scope_mismatch",
            "Artifact does not belong to the current Run",
        ));
    }
    if metadata.kind != expected_kind {
        return Err(CoreError::validation(
            "artifact_kind_mismatch",
            "Artifact kind does not match this query_data action",
        ));
    }
    if metadata.content_hash != expected_hash {
        return Err(CoreError::validation(
            "artifact_hash_mismatch",
            "Artifact content hash does not match the supplied immutable reference",
        ));
    }
    Ok(())
}

impl QueryDataTool {
    async fn load_json<T>(
        &self,
        context: &ToolExecutionContext,
        artifact_id: &ArtifactId,
        expected_kind: ArtifactKind,
        expected_hash: &str,
    ) -> CoreResult<T>
    where
        T: DeserializeOwned,
    {
        let record = self
            .artifact_lookup
            .load(artifact_id, &access_context(context))
            .await?;
        verify_artifact_identity(&record.metadata, context, expected_kind, expected_hash)?;
        serde_json::from_slice(&record.bytes)
            .map_err(|error| CoreError::validation("artifact_json_invalid", error.to_string()))
    }
}

impl QueryDataTool {
    async fn execute_action(
        &self,
        context: &ToolExecutionContext,
        plan_artifact_id: ArtifactId,
        plan_hash: String,
        preflight_artifact_id: ArtifactId,
        preflight_hash: String,
    ) -> CoreResult<ToolOutcome> {
        let plan: QueryPlan = match self
            .load_json(
                context,
                &plan_artifact_id,
                ArtifactKind::QueryPlan,
                &plan_hash,
            )
            .await
        {
            Ok(plan) => plan,
            Err(error) => return Ok(artifact_rejection(&error)),
        };
        let evidence: QueryPreflightEvidence = match self
            .load_json(
                context,
                &preflight_artifact_id,
                ArtifactKind::QueryPreflight,
                &preflight_hash,
            )
            .await
        {
            Ok(evidence) => evidence,
            Err(error) => return Ok(artifact_rejection(&error)),
        };
        if evidence.schema_version != 1
            || evidence.plan_artifact_id != plan_artifact_id
            || evidence.plan_hash != plan_hash
            || evidence.budget_hash != stable_hash(&context.query_budget)?
            || evidence.scope_hash != stable_hash(&context.data_scope)?
        {
            return Ok(rejected(
                "preflight_invalidated",
                ToolFailureCategory::Policy,
                "Plan, budget, scope, or preflight identity changed",
                false,
                CostClass::High,
            ));
        }

        let connector = match self.connectors.get(&plan.source_id) {
            Ok(connector) => connector,
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Unknown)),
        };
        let recompiled = match self.compile_plan(context, &plan, &connector).await {
            Ok(compiled) => compiled,
            Err(outcome) => return Ok(outcome),
        };
        if recompiled != evidence.compiled {
            return Ok(rejected(
                "compiled_plan_changed",
                ToolFailureCategory::Governance,
                "Active contract or compiled plan changed after preflight",
                false,
                CostClass::High,
            ));
        }
        match evidence.connector_preflight.decision {
            QueryPreflightDecision::Allowed => {}
            QueryPreflightDecision::ConfirmationRequired if context.confirmation_granted => {}
            QueryPreflightDecision::ConfirmationRequired => {
                return Ok(rejected(
                    "query_confirmation_required",
                    ToolFailureCategory::Budget,
                    "This preflight requires explicit cost confirmation",
                    false,
                    CostClass::High,
                ));
            }
            QueryPreflightDecision::Rejected => {
                return Ok(preflight_rejection(&evidence.connector_preflight));
            }
        }

        let result = match connector
            .query
            .execute_query(query_request(context, &recompiled))
            .await
        {
            Ok(result) => result,
            Err(error) => {
                return Ok(failed(
                    error.code(),
                    ToolFailureCategory::Internal,
                    "Connector query execution failed",
                    false,
                    CostClass::Unknown,
                ));
            }
        };
        let result_evidence = QueryResultEvidence {
            schema_version: 1,
            plan_artifact_id,
            preflight_artifact_id,
            compiled: recompiled.clone(),
            result: result.clone(),
        };
        let metadata = put_json(
            self.artifact_store.as_ref(),
            PutArtifact {
                workspace_id: context.workspace_id,
                task_id: context.task_id,
                run_id: context.run_id,
                kind: ArtifactKind::QueryResult,
                media_type: "application/json".to_owned(),
                bytes: Vec::new(),
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            },
            &result_evidence,
        )
        .await?;

        Ok(ToolOutcome::Succeeded {
            message: "Executed the exact preflighted query".to_owned(),
            output: json!({
                "artifact_id": metadata.id,
                "artifact_hash": metadata.content_hash.clone(),
                "semantic_status": recompiled.semantic_status,
                "metric_id": recompiled.metric_id,
                "metric_version": recompiled.metric_version,
                "source_id": recompiled.source_id,
                "source_relations": recompiled.source_relations,
                "executed_sql": recompiled.sql,
                "bound_parameters": recompiled.parameters,
                "columns": result.columns,
                "row_count": result.row_count,
                "truncated": result.truncated,
                "warning_codes": result.warning_codes,
                "model_preview": result.model_preview,
            }),
            artifacts: vec![metadata],
        })
    }
}

#[async_trait]
impl Tool for QueryDataTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "query_data".to_owned(),
            description: "Preflight or execute one immutable QueryPlan Artifact.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string" },
                    "plan_artifact_id": { "type": "string" },
                    "plan_hash": { "type": "string" },
                    "preflight_artifact_id": { "type": "string" },
                    "preflight_hash": { "type": "string" }
                },
                "required": ["action", "plan_artifact_id", "plan_hash"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "artifact_id": { "type": "string" },
                    "artifact_hash": { "type": "string" }
                },
                "required": ["artifact_id", "artifact_hash"],
                "additionalProperties": true
            }),
            risk: ToolRisk::Medium,
            side_effect: SideEffect::None,
            idempotent: true,
            timeout_ms: 60_000,
            max_output_bytes: 65_536,
            required_permissions: vec!["data_query".to_owned()],
            input_sensitivity: Sensitivity::Internal,
            output_sensitivity: Sensitivity::Internal,
            version: "1.0.0".to_owned(),
        }
    }

    async fn execute(
        &self,
        context: &ToolExecutionContext,
        arguments: Value,
    ) -> CoreResult<ToolOutcome> {
        let input: QueryDataInput = match parse_arguments(arguments) {
            Ok(input) => input,
            Err(outcome) => return Ok(outcome),
        };
        match input {
            QueryDataInput::Preflight {
                plan_artifact_id,
                plan_hash,
            } => {
                self.preflight_action(context, plan_artifact_id, plan_hash)
                    .await
            }
            QueryDataInput::Execute {
                plan_artifact_id,
                plan_hash,
                preflight_artifact_id,
                preflight_hash,
            } => {
                self.execute_action(
                    context,
                    plan_artifact_id,
                    plan_hash,
                    preflight_artifact_id,
                    preflight_hash,
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod compiler_tests {
    use chrono::{TimeZone, Utc};
    use ys_agent_core::{MetricDefinition, MetricStatus, QueryParameter, SourceId};

    use super::{MetricSqlCompiler, MetricSqlDialect};

    fn metric() -> MetricDefinition {
        MetricDefinition {
            id: "commerce.gmv".to_owned(),
            version: "1".to_owned(),
            status: MetricStatus::Active,
            description: "Paid order value".to_owned(),
            source_relation: "mart_orders".to_owned(),
            expression: "SUM(paid_amount)".to_owned(),
            time_column: "paid_at".to_owned(),
            allowed_dimensions: vec!["channel".to_owned()],
            owner: "data-team".to_owned(),
            freshness_sla_seconds: Some(3_600),
        }
    }

    #[test]
    fn sqlite_uses_bound_question_mark_parameters() {
        let start = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 8, 8, 0, 0, 0).unwrap();
        let compiled = MetricSqlCompiler::new(MetricSqlDialect::Sqlite)
            .compile(
                SourceId::new("sqlite-demo"),
                &metric(),
                start,
                end,
                &["channel".to_owned()],
            )
            .unwrap();

        assert!(compiled.sql.contains("SUM(paid_amount) AS metric_value"));
        assert!(compiled.sql.contains("\"paid_at\" >= ?"));
        assert!(!compiled.sql.contains("2026-08-01"));
        assert_eq!(
            compiled.parameters,
            vec![
                QueryParameter::Timestamp(start),
                QueryParameter::Timestamp(end),
            ]
        );
    }

    #[test]
    fn postgres_numbers_bound_parameters() {
        let compiled = MetricSqlCompiler::new(MetricSqlDialect::Postgres)
            .compile(
                SourceId::new("warehouse"),
                &metric(),
                Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 8, 8, 0, 0, 0).unwrap(),
                &[],
            )
            .unwrap();

        assert!(compiled.sql.contains("\"paid_at\" >= $1"));
        assert!(compiled.sql.contains("\"paid_at\" < $2"));
        assert!(!compiled.sql.contains("GROUP BY"));
    }

    #[test]
    fn unapproved_dimension_fails_before_sql_exists() {
        let error = MetricSqlCompiler::new(MetricSqlDialect::Sqlite)
            .compile(
                SourceId::new("sqlite-demo"),
                &metric(),
                Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 8, 8, 0, 0, 0).unwrap(),
                &["card_number".to_owned()],
            )
            .unwrap_err();

        assert_eq!(error.code(), "metric_dimension_not_allowed");
    }
}
