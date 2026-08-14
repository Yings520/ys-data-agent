use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::WorkspaceId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryIntent {
    GovernedMetric {
        metric_id: String,
        metric_version: String,
        dimensions: Vec<String>,
        time_range: String,
    },
    AdHocRead {
        question: String,
        assumed_relations: Vec<String>,
    },
    Metadata {
        subject: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryBudget {
    pub max_sql_bytes: usize,
    pub statement_timeout_ms: u64,
    pub acquire_timeout_ms: u64,
    pub max_rows: usize,
    pub max_result_bytes: usize,
    pub max_concurrency: usize,
    pub max_estimated_cost_units: Option<u64>,
    pub max_scanned_bytes: Option<u64>,
}

impl Default for QueryBudget {
    fn default() -> Self {
        Self {
            max_sql_bytes: 16_384,
            statement_timeout_ms: 30_000,
            acquire_timeout_ms: 5_000,
            max_rows: 10_000,
            max_result_bytes: 2 * 1024 * 1024,
            max_concurrency: 2,
            max_estimated_cost_units: None,
            max_scanned_bytes: None,
        }
    }
}

/// v0.2: exact relation → allowed columns. No wildcards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedDataScope {
    pub workspace_id: WorkspaceId,
    pub source_id: String,
    pub relations: BTreeMap<String, BTreeMap<String, ColumnPolicy>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnPolicy {
    Allow,
    Redact,
    LocalArtifactOnly,
    Deny,
}
