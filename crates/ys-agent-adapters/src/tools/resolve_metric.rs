use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use ys_agent_core::{
    CoreResult, CostClass, MetricProvider, MetricStatus, Sensitivity, SideEffect, Tool,
    ToolExecutionContext, ToolFailureCategory, ToolOutcome, ToolRisk, ToolSpec,
};

use super::{parse_arguments, rejected, safe_internal_failure};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveMetricInput {
    metric: String,
}

pub struct ResolveMetricTool {
    metrics: Arc<dyn MetricProvider>,
}

impl ResolveMetricTool {
    pub fn new(metrics: Arc<dyn MetricProvider>) -> Self {
        Self { metrics }
    }
}

#[async_trait]
impl Tool for ResolveMetricTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "resolve_metric".to_owned(),
            description: "Resolve one exact Active governed metric contract.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": { "metric": { "type": "string" } },
                "required": ["metric"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "version": { "type": "string" },
                    "status": { "const": "active" },
                    "description": { "type": "string" },
                    "source_relation": { "type": "string" },
                    "expression": { "type": "string" },
                    "time_column": { "type": "string" },
                    "allowed_dimensions": { "type": "array" },
                    "owner": { "type": "string" }
                },
                "required": [
                    "id", "version", "status", "description", "source_relation",
                    "expression", "time_column", "allowed_dimensions", "owner"
                ],
                "additionalProperties": true
            }),
            risk: ToolRisk::Low,
            side_effect: SideEffect::None,
            idempotent: true,
            timeout_ms: 2_000,
            max_output_bytes: 16_384,
            required_permissions: vec!["data_query".to_owned()],
            input_sensitivity: Sensitivity::Internal,
            output_sensitivity: Sensitivity::Internal,
            version: "1.0.0".to_owned(),
        }
    }

    async fn execute(
        &self,
        _context: &ToolExecutionContext,
        arguments: Value,
    ) -> CoreResult<ToolOutcome> {
        let input: ResolveMetricInput = match parse_arguments(arguments) {
            Ok(input) => input,
            Err(outcome) => return Ok(outcome),
        };
        let metric = match self.metrics.get_metric(input.metric.trim()).await {
            Ok(Some(metric)) if metric.status == MetricStatus::Active => metric,
            Ok(_) => {
                return Ok(rejected(
                    "metric_not_found_or_inactive",
                    ToolFailureCategory::Governance,
                    "The requested metric does not exist or is not Active",
                    true,
                    CostClass::Low,
                ));
            }
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Low)),
        };

        Ok(ToolOutcome::Succeeded {
            message: format!("Resolved Active metric {}", metric.id),
            output: json!({
                "id": metric.id,
                "version": metric.version,
                "status": metric.status,
                "description": metric.description,
                "source_relation": metric.source_relation,
                "expression": metric.expression,
                "time_column": metric.time_column,
                "allowed_dimensions": metric.allowed_dimensions,
                "owner": metric.owner,
                "freshness_sla_seconds": metric.freshness_sla_seconds,
            }),
            artifacts: Vec::new(),
        })
    }
}
