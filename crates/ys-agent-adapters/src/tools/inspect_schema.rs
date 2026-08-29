use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use ys_agent_core::{
    ArtifactKind, ArtifactStore, ColumnPolicy, CoreResult, CostClass, ObservedRelation,
    ObservedSchema, PutArtifact, RetentionPolicy, SchemaKnowledgeKind, Sensitivity, SideEffect,
    SourceId, Tool, ToolExecutionContext, ToolFailureCategory, ToolOutcome, ToolRisk, ToolSpec,
};

use super::{ConnectorRegistry, parse_arguments, put_json, rejected, safe_internal_failure};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectSchemaInput {
    source_id: String,
    #[serde(default)]
    relations: Vec<String>,
}

pub struct InspectSchemaTool {
    connectors: ConnectorRegistry,
    artifacts: Arc<dyn ArtifactStore>,
    max_relations: usize,
    max_columns: usize,
    max_inline_bytes: usize,
}

impl InspectSchemaTool {
    pub fn new(
        connectors: ConnectorRegistry,
        artifacts: Arc<dyn ArtifactStore>,
        max_relations: usize,
        max_columns: usize,
        max_inline_bytes: usize,
    ) -> Self {
        Self {
            connectors,
            artifacts,
            max_relations,
            max_columns,
            max_inline_bytes,
        }
    }
}

impl InspectSchemaTool {
    fn authorize(
        &self,
        context: &ToolExecutionContext,
        observed: ObservedSchema,
        requested: &[String],
    ) -> Result<ObservedSchema, ToolOutcome> {
        let requested_count = requested.len();
        let requested = requested.iter().cloned().collect::<BTreeSet<_>>();
        if requested.len() != requested_count {
            return Err(rejected(
                "duplicate_relation",
                ToolFailureCategory::InvalidArguments,
                "Relation filters must be unique",
                true,
                CostClass::Low,
            ));
        }

        for relation in &requested {
            if !context.data_scope.relations.contains_key(relation) {
                return Err(rejected(
                    "relation_not_allowed",
                    ToolFailureCategory::Authorization,
                    format!("Relation {relation} is outside the allowed scope"),
                    true,
                    CostClass::Low,
                ));
            }
        }

        let mut relations = observed
            .relations
            .into_iter()
            .filter_map(|mut relation| {
                let columns = context.data_scope.relations.get(&relation.name)?;
                if !requested.is_empty() && !requested.contains(&relation.name) {
                    return None;
                }
                relation.columns.retain(|column| {
                    matches!(
                        columns.get(&column.name),
                        Some(ColumnPolicy::Allow | ColumnPolicy::Redact)
                    )
                });
                relation
                    .columns
                    .sort_by(|left, right| left.name.cmp(&right.name));
                Some(relation)
            })
            .collect::<Vec<_>>();
        relations.sort_by(|left, right| left.name.cmp(&right.name));

        if !requested.is_empty() {
            let found = relations
                .iter()
                .map(|relation| relation.name.clone())
                .collect::<BTreeSet<_>>();
            if found != requested {
                return Err(rejected(
                    "observed_relation_missing",
                    ToolFailureCategory::SchemaChanged,
                    "One or more requested relations were not observed",
                    true,
                    CostClass::Low,
                ));
            }
        }

        Ok(ObservedSchema {
            source_id: observed.source_id,
            kind: SchemaKnowledgeKind::Observed,
            relations,
            observed_at: observed.observed_at,
        })
    }

    fn inline_schema(&self, full: &ObservedSchema) -> (ObservedSchema, bool) {
        let full_column_count = full
            .relations
            .iter()
            .map(|relation| relation.columns.len())
            .sum::<usize>();
        let must_truncate = full.relations.len() > self.max_relations
            || full_column_count > self.max_columns
            || serde_json::to_vec(full)
                .map(|bytes| bytes.len() > self.max_inline_bytes)
                .unwrap_or(true);
        if !must_truncate {
            return (full.clone(), false);
        }

        let mut remaining_columns = self.max_columns;
        let relations = full
            .relations
            .iter()
            .take(self.max_relations)
            .map(|relation| {
                let take = relation.columns.len().min(remaining_columns);
                remaining_columns -= take;
                ObservedRelation {
                    name: relation.name.clone(),
                    columns: relation.columns.iter().take(take).cloned().collect(),
                }
            })
            .take_while(|relation| !relation.columns.is_empty())
            .collect();

        (
            ObservedSchema {
                source_id: full.source_id.clone(),
                kind: full.kind,
                relations,
                observed_at: full.observed_at,
            },
            true,
        )
    }
}

#[async_trait]
impl Tool for InspectSchemaTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "inspect_schema".to_owned(),
            description: "Observe authorized relation and column metadata for one source."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source_id": { "type": "string" },
                    "relations": { "type": "array" }
                },
                "required": ["source_id"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "knowledge_kind": { "const": "observed" },
                    "source_id": { "type": "string" },
                    "relations": { "type": "array" },
                    "truncated": { "type": "boolean" }
                },
                "required": ["knowledge_kind", "source_id", "relations", "truncated"],
                "additionalProperties": true
            }),
            risk: ToolRisk::Low,
            side_effect: SideEffect::None,
            idempotent: true,
            timeout_ms: 5_000,
            max_output_bytes: 32_768,
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
        let input: InspectSchemaInput = match parse_arguments(arguments) {
            Ok(input) => input,
            Err(outcome) => return Ok(outcome),
        };
        if input.source_id != context.data_scope.source_id {
            return Ok(rejected(
                "source_not_allowed",
                ToolFailureCategory::Authorization,
                "The requested source is outside the allowed scope",
                false,
                CostClass::Low,
            ));
        }

        let source_id = SourceId::new(input.source_id);
        let connector = match self.connectors.get(&source_id) {
            Ok(connector) => connector,
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Low)),
        };
        let observed = match connector.catalog.observe_schema(&source_id).await {
            Ok(observed) => observed,
            Err(error) => return Ok(safe_internal_failure(&error, CostClass::Low)),
        };
        let full = match self.authorize(context, observed, &input.relations) {
            Ok(schema) => schema,
            Err(outcome) => return Ok(outcome),
        };
        let (inline, truncated) = self.inline_schema(&full);

        let mut artifacts = Vec::new();
        let artifact_id = if truncated {
            let metadata = put_json(
                self.artifacts.as_ref(),
                PutArtifact {
                    workspace_id: context.workspace_id,
                    task_id: context.task_id,
                    run_id: context.run_id,
                    kind: ArtifactKind::ContextEvidence,
                    media_type: "application/json".to_owned(),
                    bytes: Vec::new(),
                    sensitivity: Sensitivity::Internal,
                    owner: None,
                    retention_policy: Some(RetentionPolicy::Session),
                    expires_at: None,
                    producer_step_id: None,
                },
                &full,
            )
            .await?;
            let id = metadata.id;
            artifacts.push(metadata);
            Some(id)
        } else {
            None
        };

        Ok(ToolOutcome::Succeeded {
            message: "Observed authorized schema".to_owned(),
            output: json!({
                "knowledge_kind": "observed",
                "source_id": source_id,
                "observed_at": inline.observed_at,
                "relations": inline.relations,
                "truncated": truncated,
                "artifact_id": artifact_id,
            }),
            artifacts,
        })
    }
}
