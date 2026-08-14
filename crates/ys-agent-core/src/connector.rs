use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AllowedDataScope, CoreError, CoreResult, QueryBudget, Sensitivity};

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
}

// Database abilities → schema seen → cost check → query request → query result
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub source_id: SourceId,
    pub dialect: String,
    pub supports_explain: bool,
    pub supports_read_only_tx: bool,
    pub max_concurrency: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedColumn {
    pub name: String,
    pub data_type: String,
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
    pub relations: Vec<ObservedRelation>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryCostEstimate {
    pub estimated_cost_units: Option<u64>,
    pub scanned_bytes: Option<u64>,
    pub estimator_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryPreflight {
    pub sql: String,
    pub cost: QueryCostEstimate,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub source_id: SourceId,
    pub sql: String,
    pub budget: QueryBudget,
    pub query_tag: String,
    pub scope: AllowedDataScope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
    pub remote_query_id: Option<String>,
    pub row_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreshnessObservation {
    pub source_id: SourceId,
    pub relation: String,
    pub observed_at: DateTime<Utc>,
    pub data_as_of: Option<DateTime<Utc>>,
    pub lag_seconds: Option<u64>,
}
