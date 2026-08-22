use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use ys_agent_core::{
    Capability, ContextManifest, CoreError, CoreResult, Principal, RunStatus, ToolSpec,
    WorkflowKind,
};

use super::catalog::ToolCatalog;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryPhase {
    Clarify,
    ClassifyIntent,
    ResolveContext,
    Plan,
    ValidateAndPreflight,
    Execute,
    Verify,
    Package,
    ReadyToComplete,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectorToolAvailability {
    available: BTreeSet<String>,
}

impl ConnectorToolAvailability {
    pub fn all_query_tools() -> Self {
        Self {
            available: ["inspect_schema", "query_data", "read_freshness"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn from_names(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            available: names.into_iter().collect(),
        }
    }

    fn supports(&self, name: &str) -> bool {
        name == "resolve_metric" || self.available.contains(name)
    }
}

#[derive(Debug, Clone)]
pub struct ToolView {
    specs: BTreeMap<String, ToolSpec>,
    content_hash: String,
}

impl ToolView {
    pub fn contains(&self, name: &str) -> bool {
        self.specs.contains_key(name)
    }

    pub fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.specs.get(name)
    }

    pub fn model_tools(&self) -> Vec<ToolSpec> {
        self.specs.values().cloned().collect()
    }

    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    pub fn contains_exact_version(&self, name: &str, version: &str) -> bool {
        self.spec(name).is_some_and(|spec| spec.version == version)
    }

    pub fn apply_to_manifest(&self, manifest: &mut ContextManifest) {
        manifest.tool_view_version = self.content_hash.clone();
    }
}

pub struct ToolViewBuilder<'a> {
    catalog: &'a ToolCatalog,
    workflow: Option<WorkflowKind>,
    query_phase: Option<QueryPhase>,
    principal: Option<&'a Principal>,
    connector_tools: Option<ConnectorToolAvailability>,
    run_status: Option<RunStatus>,
}

impl<'a> ToolViewBuilder<'a> {
    pub fn new(catalog: &'a ToolCatalog) -> Self {
        Self {
            catalog,
            workflow: None,
            query_phase: None,
            principal: None,
            connector_tools: None,
            run_status: None,
        }
    }

    pub fn for_workflow(mut self, workflow: WorkflowKind) -> Self {
        self.workflow = Some(workflow);
        self
    }

    pub fn for_query_phase(mut self, phase: QueryPhase) -> Self {
        self.query_phase = Some(phase);
        self
    }

    pub fn for_principal(mut self, principal: &'a Principal) -> Self {
        self.principal = Some(principal);
        self
    }

    pub fn with_connector_tools(mut self, availability: ConnectorToolAvailability) -> Self {
        self.connector_tools = Some(availability);
        self
    }

    pub fn for_run_status(mut self, status: RunStatus) -> Self {
        self.run_status = Some(status);
        self
    }

    pub fn build(self) -> CoreResult<ToolView> {
        let workflow = self.workflow.ok_or_else(|| {
            CoreError::validation("missing_workflow", "ToolView needs a Workflow")
        })?;
        let phase = self.query_phase.ok_or_else(|| {
            CoreError::validation("missing_query_phase", "ToolView needs a Query phase")
        })?;
        let principal = self.principal.ok_or_else(|| {
            CoreError::validation("missing_principal", "ToolView needs a Principal")
        })?;
        let connector_tools = self.connector_tools.ok_or_else(|| {
            CoreError::validation(
                "missing_connector_capabilities",
                "ToolView needs Connector capability availability",
            )
        })?;
        let run_status = self.run_status.ok_or_else(|| {
            CoreError::validation("missing_run_status", "ToolView needs current Run state")
        })?;

        let mut specs = BTreeMap::new();
        let may_use_tools = workflow == WorkflowKind::Query
            && run_status == RunStatus::Running
            && principal.has_capability(Capability::DataQuery);

        if may_use_tools {
            for name in phase_tool_names(phase) {
                if !connector_tools.supports(name) {
                    continue;
                }

                let tool = self.catalog.get(name).ok_or_else(|| CoreError::NotFound {
                    entity: "tool",
                    id: (*name).to_owned(),
                })?;
                let mut spec = tool.spec();

                if !self.catalog.policy().allows(&spec) {
                    continue;
                }

                narrow_query_data_schema(&mut spec, phase);
                specs.insert(spec.name.clone(), spec);
            }
        }

        let canonical = serde_json::to_vec(&specs).map_err(|_| {
            CoreError::validation(
                "tool_view_serialization",
                "could not serialize the deterministic ToolView",
            )
        })?;
        let content_hash: String = Sha256::digest(canonical)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        Ok(ToolView {
            specs,
            content_hash,
        })
    }
}

fn phase_tool_names(phase: QueryPhase) -> &'static [&'static str] {
    match phase {
        QueryPhase::Clarify | QueryPhase::ClassifyIntent => &[],
        QueryPhase::ResolveContext => &["resolve_metric", "inspect_schema"],
        QueryPhase::Plan => &[],
        QueryPhase::ValidateAndPreflight | QueryPhase::Execute => &["query_data"],
        QueryPhase::Verify => &["read_freshness"],
        QueryPhase::Package | QueryPhase::ReadyToComplete => &[],
    }
}

fn narrow_query_data_schema(spec: &mut ToolSpec, phase: QueryPhase) {
    if spec.name != "query_data" {
        return;
    }

    spec.input_schema = match phase {
        QueryPhase::ValidateAndPreflight => json!({
            "type": "object",
            "properties": {
                "action": { "const": "preflight" },
                "plan_artifact_id": { "type": "string" },
                "plan_hash": { "type": "string" }
            },
            "required": ["action", "plan_artifact_id", "plan_hash"],
            "additionalProperties": false
        }),
        QueryPhase::Execute => json!({
            "type": "object",
            "properties": {
                "action": { "const": "execute" },
                "plan_artifact_id": { "type": "string" },
                "plan_hash": { "type": "string" },
                "preflight_artifact_id": { "type": "string" },
                "preflight_hash": { "type": "string" }
            },
            "required": [
                "action",
                "plan_artifact_id",
                "plan_hash",
                "preflight_artifact_id",
                "preflight_hash"
            ],
            "additionalProperties": false
        }),
        QueryPhase::Clarify
        | QueryPhase::ClassifyIntent
        | QueryPhase::ResolveContext
        | QueryPhase::Plan
        | QueryPhase::Verify
        | QueryPhase::Package
        | QueryPhase::ReadyToComplete => return,
    };
}
