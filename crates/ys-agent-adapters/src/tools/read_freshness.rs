use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use ys_agent_core::{
    ColumnPolicy, CoreResult, CostClass, MetricProvider, MetricStatus, Sensitivity, SideEffect,
    SourceId, Tool, ToolExecutionContext, ToolFailureCategory, ToolOutcome, ToolRisk, ToolSpec,
};

use super::{ConnectorRegistry, parse_arguments, rejected, safe_internal_failure};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFreshnessInput {
    source_id: String,
    relation: String,
    time_column: String,
}

pub struct ReadFreshnessTool {
    connectors: ConnectorRegistry,
    metrics: Arc<dyn MetricProvider>,
}

impl ReadFreshnessTool {
    pub fn new(connectors: ConnectorRegistry, metrics: Arc<dyn MetricProvider>) -> Self {
        Self {
            connectors,
            metrics,
        }
    }
}

fn is_safe_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

impl ReadFreshnessTool {
    async fn approve_column(
        &self,
        context: &ToolExecutionContext,
        source_id: &SourceId,
        relation: &str,
        time_column: &str,
    ) -> Result<Option<u64>, ToolOutcome> {
        if !is_safe_identifier(time_column) {
            return Err(rejected(
                "unsafe_freshness_column",
                ToolFailureCategory::InvalidArguments,
                "Freshness needs one safe column identifier",
                true,
                CostClass::Low,
            ));
        }
        let columns = context.data_scope.relations.get(relation).ok_or_else(|| {
            rejected(
                "relation_not_allowed",
                ToolFailureCategory::Authorization,
                "Freshness relation is outside the allowed scope",
                true,
                CostClass::Low,
            )
        })?;
        if !matches!(
            columns.get(time_column),
            Some(ColumnPolicy::Allow | ColumnPolicy::Redact)
        ) {
            return Err(rejected(
                "freshness_column_not_allowed",
                ToolFailureCategory::Authorization,
                "Freshness column is outside the readable scope",
                true,
                CostClass::Low,
            ));
        }

        let metrics = self
            .metrics
            .list_active_metrics()
            .await
            .map_err(|error| safe_internal_failure(&error, CostClass::Low))?;
        if let Some(metric) = metrics.iter().find(|metric| {
            metric.status == MetricStatus::Active
                && metric.source_relation == relation
                && metric.time_column == time_column
        }) {
            return Ok(metric.freshness_sla_seconds);
        }

        let connector = self
            .connectors
            .get(source_id)
            .map_err(|error| safe_internal_failure(&error, CostClass::Low))?;
        let schema = connector
            .catalog
            .observe_schema(source_id)
            .await
            .map_err(|error| safe_internal_failure(&error, CostClass::Low))?;
        let observed = schema.relations.iter().any(|candidate| {
            candidate.name == relation
                && candidate
                    .columns
                    .iter()
                    .any(|column| column.name == time_column)
        });
        if observed {
            Ok(None)
        } else {
            Err(rejected(
                "freshness_column_unproven",
                ToolFailureCategory::Governance,
                "Freshness column was not found in a metric contract or observed schema",
                true,
                CostClass::Low,
            ))
        }
    }
}

#[async_trait]
impl Tool for ReadFreshnessTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_freshness".to_owned(),
            description: "Read freshness for one approved relation time column.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source_id": { "type": "string" },
                    "relation": { "type": "string" },
                    "time_column": { "type": "string" }
                },
                "required": ["source_id", "relation", "time_column"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "source_id": { "type": "string" },
                    "relation": { "type": "string" },
                    "time_column": { "type": "string" },
                    "observed_at": { "type": "string" },
                    "age_seconds": { "type": "integer" }
                },
                "required": ["source_id", "relation", "time_column", "observed_at"],
                "additionalProperties": true
            }),
            risk: ToolRisk::Low,
            side_effect: SideEffect::None,
            idempotent: true,
            timeout_ms: 5_000,
            max_output_bytes: 8_192,
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
        let input: ReadFreshnessInput = match parse_arguments(arguments) {
            Ok(input) => input,
            Err(outcome) => return Ok(outcome),
        };
        if input.source_id != context.data_scope.source_id {
            return Ok(rejected(
                "source_not_allowed",
                ToolFailureCategory::Authorization,
                "Freshness source is outside the allowed scope",
                false,
                CostClass::Low,
            ));
        }

        let source_id = SourceId::new(input.source_id);
        let sla = match self
            .approve_column(context, &source_id, &input.relation, &input.time_column)
            .await
        {
            Ok(sla) => sla,
            Err(outcome) => return Ok(outcome),
        };
        let connector = match self.connectors.get(&source_id) {
            Ok(connector) => connector,
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Low)),
        };
        let observation = match connector
            .freshness
            .read_freshness(&source_id, &input.relation, &input.time_column)
            .await
        {
            Ok(observation) => observation,
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Low)),
        };
        let is_fresh = match (observation.lag_seconds, sla) {
            (Some(age), Some(limit)) => Some(age <= limit),
            _ => None,
        };

        Ok(ToolOutcome::Succeeded {
            message: "Read approved freshness observation".to_owned(),
            output: json!({
                "source_id": observation.source_id,
                "relation": observation.relation,
                "time_column": input.time_column,
                "observed_at": observation.observed_at,
                "latest_data_at": observation.data_as_of,
                "age_seconds": observation.lag_seconds,
                "sla_seconds": sla,
                "is_fresh": is_fresh,
            }),
            artifacts: Vec::new(),
        })
    }
}
