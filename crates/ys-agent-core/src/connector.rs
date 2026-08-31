use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AllowedDataScope, CoreError, CoreResult, QueryBudget, QueryParameter, Sensitivity};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque credential locator only. Never a DSN/password/token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialReference(String);

impl CredentialReference {
    pub fn new(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        if value.chars().any(|c| c.is_whitespace()) {
            return Err(CoreError::validation(
                "invalid_credential_reference",
                "credential reference must not contain whitespace",
            ));
        }

        if value.contains("://") && value.starts_with("env:") {
            return Err(CoreError::validation(
                "inline_secret_rejected",
                "credential reference must not embed URLs or other secrets",
            ));
        }

        let Some((scheme, rest)) = value.split_once(':') else {
            return Err(CoreError::validation(
                "invalid_credential_reference",
                "credential reference requires a scheme such as env:",
            ));
        };

        if scheme != "env" || rest.is_empty() {
            return Err(CoreError::validation(
                "invalid_credential_reference",
                "v0.2 only supports env:NAME credential references",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the name of the environment variable identified by this reference.
    pub fn environment_variable_name(&self) -> &str {
        self.0
            .strip_prefix("env:")
            .expect("CredentialReference only permits env references")
    }
}

// Database abilities → schema seen → cost check → query request → query result
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub source_id: SourceId,
    pub dialect: String,
    pub catalog_reader: bool,
    pub sql_query_executor: bool,
    pub freshness_reader: bool,
    pub supports_explain: bool,
    pub supports_read_only_tx: bool,
    pub max_concurrency: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaKnowledgeKind {
    Observed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key_position: Option<u32>,
    pub sensitivity: Sensitivity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedRelation {
    pub name: String,
    pub columns: Vec<ObservedColumn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedSchema {
    pub source_id: SourceId,
    pub kind: SchemaKnowledgeKind,
    pub relations: Vec<ObservedRelation>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryCostEstimate {
    pub estimated_cost_units: Option<u64>,
    pub scanned_bytes: Option<u64>,
    pub estimator_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryPreflightDecision {
    Allowed,
    ConfirmationRequired,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryPreflight {
    pub sql: String,
    pub decision: QueryPreflightDecision,
    pub cost: QueryCostEstimate,
    pub reason_codes: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CellValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Real(f64),
    Text(String),
    BlobSummary { bytes: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub source_id: SourceId,
    pub sql: String,
    pub parameters: Vec<QueryParameter>,
    pub budget: QueryBudget,
    pub query_tag: String,
    pub scope: AllowedDataScope,
    pub confirmation_granted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub truncated: bool,
    pub remote_query_id: Option<String>,
    pub row_count: usize,
    pub serialized_bytes: usize,
    pub warning_codes: Vec<String>,
    pub model_preview: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreshnessObservation {
    pub source_id: SourceId,
    pub relation: String,
    pub observed_at: DateTime<Utc>,
    pub data_as_of: Option<DateTime<Utc>>,
    pub lag_seconds: Option<u64>,
}
