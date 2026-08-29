use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{PrincipalId, Sensitivity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionTrust {
    /// Context from dbt docs, DB text, history artifacts, etc.
    /// Must never override system instructions or be treated as tool calls.
    UntrustedData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSourceType {
    MetricRegistry,
    DbtManifest,
    ObservedSchema,
    Freshness,
    TaskSummary,
    Fixture,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEvidence {
    pub source: String,
    pub source_type: ContextSourceType,
    pub version: String,
    pub observed_at: DateTime<Utc>,
    pub freshness: Option<DateTime<Utc>>,
    pub owner: Option<PrincipalId>,
    pub acl: Vec<String>,
    pub sensitivity: Sensitivity,
    pub confidence: f32,
    pub token_cost: u32,
    pub instruction_trust: InstructionTrust,
    pub text: String,
}

impl ContextEvidence {
    pub fn fixture(text: impl Into<String>) -> Self {
        Self {
            source: "fixture://content".to_owned(),
            source_type: ContextSourceType::Fixture,
            version: "v0".to_owned(),
            observed_at: Utc::now(),
            freshness: None,
            owner: None,
            acl: Vec::new(),
            sensitivity: Sensitivity::Internal,
            confidence: 1.0,
            token_cost: 0,
            instruction_trust: InstructionTrust::UntrustedData,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextOmission {
    pub uri: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextManifest {
    pub included: Vec<ContextEvidence>,
    pub summaries: Vec<String>,
    pub tool_view_version: String,
    pub token_budget: u32,
    pub tokens_used: u32,
    pub omitted: Vec<ContextOmission>,
}

impl ContextManifest {
    pub fn empty(token_budget: u32) -> Self {
        Self {
            included: Vec::new(),
            summaries: Vec::new(),
            tool_view_version: "v0".to_owned(),
            token_budget,
            tokens_used: 0,
            omitted: Vec::new(),
        }
    }

    pub fn omit(mut self, uri: impl Into<String>, reason: impl Into<String>) -> Self {
        self.omitted.push(ContextOmission {
            uri: uri.into(),
            reason: reason.into(),
        });
        self
    }
}
