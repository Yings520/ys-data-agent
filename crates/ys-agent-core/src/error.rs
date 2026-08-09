use thiserror::Error;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("invalid {entity} transition from {from} to {to}")]
    InvalidTransition {
        entity: &'static str,
        from: String,
        to: String,
    },
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
}

pub type CoreResult<T> = Result<T, CoreError>;
