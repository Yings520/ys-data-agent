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
#[serde(try_from = "String", into = "String")]
pub struct CredentialReference(CredentialLocator);

#[derive(Debug, Clone, PartialEq, Eq)]
enum CredentialLocator {
    Env(String),
    DatasourceVault(crate::DatasourceSecretRef),
}

impl CredentialReference {
    pub fn new(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        if let Some(name) = value.strip_prefix("env:") {
            let mut chars = name.chars();
            if chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Ok(Self(CredentialLocator::Env(name.to_owned())));
            }
        } else if let Some(locator) = value.strip_prefix("datasource:") {
            let parts: Vec<_> = locator.split(':').collect();
            if let [workspace, profile, generation] = parts.as_slice()
                && let (Ok(workspace), Ok(profile), Ok(generation)) =
                    (workspace.parse(), profile.parse(), generation.parse())
            {
                return crate::DatasourceSecretRef::new(workspace, profile, generation)
                    .map(Self::from_datasource);
            }
        }
        Err(CoreError::validation(
            "invalid_credential_reference",
            "credential reference must identify an environment variable or datasource generation",
        ))
    }

    /// Returns the name of the environment variable identified by this reference.
    pub fn environment_variable_name(&self) -> Option<&str> {
        match &self.0 {
            CredentialLocator::Env(name) => Some(name),
            CredentialLocator::DatasourceVault(_) => None,
        }
    }

    pub fn from_datasource(reference: crate::DatasourceSecretRef) -> Self {
        Self(CredentialLocator::DatasourceVault(reference))
    }

    pub fn datasource_reference(&self) -> Option<crate::DatasourceSecretRef> {
        match &self.0 {
            CredentialLocator::Env(_) => None,
            CredentialLocator::DatasourceVault(reference) => Some(*reference),
        }
    }
}

impl TryFrom<String> for CredentialReference {
    type Error = CoreError;

    fn try_from(value: String) -> CoreResult<Self> {
        Self::new(value)
    }
}

impl From<CredentialReference> for String {
    fn from(value: CredentialReference) -> Self {
        match value.0 {
            CredentialLocator::Env(name) => format!("env:{name}"),
            CredentialLocator::DatasourceVault(reference) => format!(
                "datasource:{}:{}:{}",
                reference.workspace_id(),
                reference.profile_id(),
                reference.generation()
            ),
        }
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
    #[serde(default)]
    pub preflight_reader: bool,
    #[serde(default)]
    pub read_only_mechanism: Option<ReadOnlyMechanism>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadOnlyMechanism {
    FileReadOnly,
    TransactionReadOnly,
}

impl CapabilityDescriptor {
    /// A capability claim is necessary but not sufficient for Ready: probe and Policy evidence
    /// must independently confirm the exact revision before activation.
    pub fn supports_governed_query(&self) -> bool {
        self.catalog_reader
            && self.sql_query_executor
            && self.freshness_reader
            && self.preflight_reader
            && self.read_only_mechanism.is_some()
            && self.max_concurrency > 0
    }
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
