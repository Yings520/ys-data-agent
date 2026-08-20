use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactId, Sensitivity, ToolCallId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Low,
    High,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureCategory {
    Authentication,
    Authorization,
    Policy,
    Budget,
    InvalidArguments,
    NotFound,
    Governance,
    Dialect,
    SchemaChanged,
    Timeout,
    Cancelled,
    Transport,
    ProviderProtocol,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFailure {
    pub code: String,
    pub category: ToolFailureCategory,
    pub user_message: String,
    pub retryable: bool,
    pub parameter_revision_allowed: bool,
    pub remote_query_id: Option<String>,
    pub cost_class: CostClass,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub risk: ToolRisk,
    pub side_effect: SideEffect,
    pub idempotent: bool,
    pub timeout_ms: u64,
    pub required_permissions: Vec<String>,
    pub input_sensitivity: Sensitivity,
    pub output_sensitivity: Sensitivity,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
    pub name: String,
    pub arguments: Value,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutcome {
    Succeeded {
        message: String,
        artifacts: Vec<ArtifactId>,
    },
    Failed {
        failure: ToolFailure,
    },
    Rejected {
        failure: ToolFailure,
    },
    Indeterminate {
        failure: ToolFailure,
    },
}

impl ToolOutcome {
    pub fn indeterminate(message: impl Into<String>) -> Self {
        Self::Indeterminate {
            failure: ToolFailure {
                code: "indeterminate".to_owned(),
                category: ToolFailureCategory::Transport,
                user_message: message.into(),
                retryable: false,
                parameter_revision_allowed: false,
                remote_query_id: None,
                cost_class: CostClass::Unknown,
            },
        }
    }

    pub fn safe_to_retry_same_call(&self) -> bool {
        match self {
            Self::Failed { failure } | Self::Rejected { failure } => {
                failure.retryable
                    && failure.cost_class == CostClass::Low
                    && !failure.parameter_revision_allowed
            }
            Self::Succeeded { .. } | Self::Indeterminate { .. } => false,
        }
    }
}
