use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("invalid {entity} transition from {from} to {to}")]
    InvalidTransition {
        entity: &'static str,
        from: String,
        to: String,
    },

    #[error("{message}")]
    Validation { code: &'static str, message: String },

    #[error("unsupported event schema version {version}")]
    UnsupportedSchemaVersion { version: u32 },

    #[error("idempotency conflict for command {command_id}")]
    IdempotencyConflict { command_id: String },

    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },

    #[error("artifact access denied: {reason}")]
    ArtifactAccessDenied { reason: String },
    #[error("corrupt artifact {artifact_id}: {reason}")]
    CorruptArtifact { artifact_id: String, reason: String },

    #[error("storage error: {message}")]
    Storage { message: String },

    #[error("optimistic concurrency conflict on run {run_id}")]
    ConcurrencyConflict { run_id: String },

    #[error("unsupported capability: {0}")]
    UnsupportedCapability(String),

    #[error("replay provider has no response left")]
    ReplayExhausted,

    #[error("duplicate tool registration: {0}")]
    DuplicateTool(String),
}

impl CoreError {
    pub(crate) fn invalid_transition(
        entity: &'static str,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Self {
        Self::InvalidTransition {
            entity,
            from: from.into(),
            to: to.into(),
        }
    }

    pub fn validation(code: &'static str, message: impl Into<String>) -> Self {
        Self::Validation {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidTransition { .. } => "invalid_transition",
            Self::Validation { code, .. } => code,
            Self::UnsupportedSchemaVersion { .. } => "unsupported_schema_version",
            Self::IdempotencyConflict { .. } => "idempotency_conflict",
            Self::NotFound { .. } => "not_found",
            Self::ArtifactAccessDenied { .. } => "artifact_access_denied",
            Self::CorruptArtifact { .. } => "corrupt_artifact",
            Self::Storage { .. } => "storage_error",
            Self::ConcurrencyConflict { .. } => "concurrency_conflict",
            Self::UnsupportedCapability(_) => "unsupported_capability",
            Self::ReplayExhausted => "replay_exhausted",
            Self::DuplicateTool(_) => "duplicate_tool",
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
