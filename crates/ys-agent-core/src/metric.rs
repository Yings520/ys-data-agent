use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricStatus {
    Draft,
    Active,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricDefinition {
    pub id: String,
    pub version: String,
    pub status: MetricStatus,
    pub description: String,
    pub source_relation: String,
    pub expression: String,
    pub time_column: String,
    pub allowed_dimensions: Vec<String>,
    pub owner: String,
    pub freshness_sla_seconds: Option<u64>,
}
