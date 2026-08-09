use std::path::PathBuf;

use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("missing configuration: {0}")]
    Configuration(String),

    #[error("cannot open database {path}: {source}")]
    DatabaseConnection {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("cannot inspect database schema: {0}")]
    SchemaInspection(#[source] rusqlite::Error),

    #[error("LLM request failed: {0}")]
    LlmRequest(#[from] reqwest::Error),

    #[error("invalid model response: {0}")]
    InvalidModelResponse(String),

    #[error("cannot parse SQL: {0}")]
    SqlParse(#[source] sqlparser::parser::ParserError),

    #[error("unsafe SQL rejected: {0}")]
    UnsafeSql(String),

    #[error("SQL execution failed: {0}")]
    SqlExecution(#[source] rusqlite::Error),

    #[error("trace operation failed: {0}")]
    Trace(String),

    #[error("agent run failed [{category}]: {message}")]
    AgentRunFailed { category: String, message: String },
}

impl AppError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "ConfigurationError",
            Self::DatabaseConnection { .. } => "DatabaseError",
            Self::SchemaInspection(_) => "SchemaError",
            Self::LlmRequest(_) => "LlmError",
            Self::InvalidModelResponse(_) => "ModelError",
            Self::SqlParse(_) => "SqlParseError",
            Self::UnsafeSql(_) => "UnsafeSqlError",
            Self::SqlExecution(_) => "SqlExecutionError",
            Self::Trace(_) => "TraceError",
            Self::AgentRunFailed { .. } => "AgentRunError",
        }
    }
}
