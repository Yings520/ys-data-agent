use std::{fmt, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ys_agent_core::CoreResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryCapability {
    GovernedMetric,
    AdHocRead,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReadiness {
    pub reachable: bool,
    pub supports_tool_calls: bool,
    pub supports_tool_call_ids: bool,
    pub supports_multi_turn_tool_results: bool,
    pub context_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReadiness {
    pub reachable: bool,
    pub query_capability: bool,
    pub catalog_capability: bool,
    pub freshness_capability: bool,
    pub database_read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorInputs {
    pub missing_config_keys: Vec<String>,
    pub model: ModelReadiness,
    pub source: SourceReadiness,
    pub query_policy_valid: bool,
    pub metric_registry_valid: bool,
    pub dbt_manifest_valid: Option<bool>,
    pub timezone_explicit: bool,
    pub freshness_rules_explicit: bool,
    pub query_budget_explicit: bool,
    pub artifact_directory_private_and_writable: bool,
    pub export_directory_private_and_writable: bool,
}

#[async_trait]
pub trait DoctorProbe: Send + Sync {
    async fn inspect(&self) -> CoreResult<DoctorInputs>;
}

#[async_trait]
pub trait DoctorRunner: Send + Sync {
    async fn run(&self) -> CoreResult<DoctorReport>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub blocker_codes: Vec<String>,
    pub warning_codes: Vec<String>,
    pub ready_capabilities: Vec<QueryCapability>,
    pub repairs: Vec<String>,
}

impl DoctorReport {
    pub fn has_blocker(&self, code: &str) -> bool {
        self.blocker_codes.iter().any(|item| item == code)
    }

    pub fn has_warning(&self, code: &str) -> bool {
        self.warning_codes.iter().any(|item| item == code)
    }

    pub fn allows_query_submission(&self) -> bool {
        self.blocker_codes.is_empty() && !self.ready_capabilities.is_empty()
    }
}

impl fmt::Display for DoctorReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "Workspace Doctor")?;
        writeln!(formatter, "blockers: {}", self.blocker_codes.join(", "))?;
        writeln!(formatter, "warnings: {}", self.warning_codes.join(", "))?;
        for repair in &self.repairs {
            writeln!(formatter, "repair: {repair}")?;
        }
        Ok(())
    }
}

pub struct WorkspaceDoctor {
    probe: Arc<dyn DoctorProbe>,
}

impl WorkspaceDoctor {
    pub fn new(probe: Arc<dyn DoctorProbe>) -> Self {
        Self { probe }
    }

    fn evaluate(inputs: DoctorInputs) -> DoctorReport {
        let mut blockers = Vec::new();
        let mut warnings = Vec::new();
        let mut repairs = Vec::new();
        let mut capabilities = vec![
            QueryCapability::GovernedMetric,
            QueryCapability::AdHocRead,
            QueryCapability::Metadata,
        ];

        if !inputs.missing_config_keys.is_empty() {
            blockers.push("required_config_missing".to_owned());
            repairs.push(format!(
                "Define required configuration keys (using CredentialReference for secrets) for: {}",
                inputs.missing_config_keys.join(", ")
            ));
        }

        if !inputs.model.reachable
            || !inputs.model.supports_tool_calls
            || !inputs.model.supports_tool_call_ids
            || !inputs.model.supports_multi_turn_tool_results
            || inputs.model.context_limit.is_none()
        {
            blockers.push("model_protocol_incompatible".to_owned());
            repairs.push(
                "Configure a reachable OpenAI-compatible model with tool calls, tool call IDs, multi-turn tool results, and a known context limit"
                    .to_owned(),
            );
        }

        if !inputs.source.reachable || !inputs.source.query_capability {
            blockers.push("connector_unavailable".to_owned());
            repairs.push(
                "Repair the configured Query connector and verify its capabilities".to_owned(),
            );
        }

        if !inputs.source.database_read_only {
            blockers.push("database_not_read_only".to_owned());
            repairs.push(
                "Use a database-enforced read-only role or SQLite read-only flags plus PRAGMA query_only = 1"
                    .to_owned(),
            );
        }

        if !inputs.query_policy_valid {
            blockers.push("query_policy_invalid".to_owned());
            repairs.push("Repair the Query Policy before submitting a query".to_owned());
        }

        if !inputs.metric_registry_valid {
            warnings.push("metric_registry_missing".to_owned());
            repairs.push("Provide a valid Metric Registry to enable GovernedMetric".to_owned());
            capabilities.retain(|item| *item != QueryCapability::GovernedMetric);
        }

        match inputs.dbt_manifest_valid {
            None => {
                warnings.push("dbt_manifest_missing".to_owned());
                repairs.push(
                    "Configure the optional dbt manifest to improve engineering Context".to_owned(),
                );
            }
            Some(false) => {
                warnings.push("dbt_manifest_invalid".to_owned());
                repairs.push("Repair or remove the optional dbt manifest path".to_owned());
            }
            Some(true) => {}
        }

        if !inputs.source.freshness_capability {
            warnings.push("freshness_capability_missing".to_owned());
            repairs
                .push("Configure a Freshness reader for current/latest/SLA questions".to_owned());
        }

        if !inputs.timezone_explicit {
            blockers.push("timezone_missing".to_owned());
            repairs.push("Set an explicit Workspace timezone".to_owned());
        }

        if !inputs.freshness_rules_explicit {
            warnings.push("freshness_rules_missing".to_owned());
            repairs.push("Define Freshness rules for time-sensitive answers".to_owned());
        }

        if !inputs.query_budget_explicit {
            blockers.push("query_budget_missing".to_owned());
            repairs.push("Set timeout, row, byte, and optional cost limits".to_owned());
        }

        if !inputs.artifact_directory_private_and_writable {
            blockers.push("artifact_directory_unsafe".to_owned());
            repairs.push("Make .ysda/artifacts owner-only and writable".to_owned());
        }

        if !inputs.export_directory_private_and_writable {
            warnings.push("export_directory_unsafe".to_owned());
            repairs.push("Make .ysda/exports owner-only and writable before exporting".to_owned());
        }

        if !inputs.source.catalog_capability {
            capabilities.retain(|item| *item != QueryCapability::Metadata);
        }
        if !blockers.is_empty() {
            capabilities.clear();
        }

        DoctorReport {
            blocker_codes: blockers,
            warning_codes: warnings,
            ready_capabilities: capabilities,
            repairs,
        }
    }
}

#[async_trait]
impl DoctorRunner for WorkspaceDoctor {
    async fn run(&self) -> CoreResult<DoctorReport> {
        let inputs = self.probe.inspect().await?;
        Ok(Self::evaluate(inputs))
    }
}
