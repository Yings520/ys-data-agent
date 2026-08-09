use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserQuestion {
    pub text: String,
}

impl UserQuestion {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
    pub primary_key_position: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SchemaSnapshot {
    pub tables: Vec<TableSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeneratedQuery {
    pub sql: String,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reasons: Vec<String>,
}

impl PolicyDecision {
    pub fn allow() -> Self {
        Self {
            allowed: true,
            reasons: Vec::new(),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reasons: vec![reason.into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CellValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(String),
}

impl fmt::Display for CellValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(formatter, "NULL"),
            Self::Integer(value) => write!(formatter, "{value}"),
            Self::Real(value) => write!(formatter, "{value}"),
            Self::Text(value) | Self::Blob(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    #[serde(default, skip_serializing)]
    pub rows: Vec<Vec<CellValue>>,
    pub row_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunEvent {
    pub stage: String,
    pub elapsed_ms: u128,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunErrorRecord {
    pub category: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRun {
    pub run_id: Uuid,
    pub question: UserQuestion,
    pub schema: Option<SchemaSnapshot>,
    pub generated_query: Option<GeneratedQuery>,
    pub policy_decision: Option<PolicyDecision>,
    pub result: Option<QueryResult>,
    pub events: Vec<RunEvent>,
    pub error: Option<RunErrorRecord>,
}

impl AgentRun {
    pub fn new(question: UserQuestion) -> Self {
        Self {
            run_id: Uuid::new_v4(),
            question,
            schema: None,
            generated_query: None,
            policy_decision: None,
            result: None,
            events: Vec::new(),
            error: None,
        }
    }
}
