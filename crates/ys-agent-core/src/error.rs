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

    #[error("optimistic concurrency conflict on run {run_id}")]
    ConcurrencyConflict { run_id: String },
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
            Self::ConcurrencyConflict { .. } => "concurrency_conflict",
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
