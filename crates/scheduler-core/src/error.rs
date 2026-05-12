use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ValidationError {
    #[error("{field}: {message}")]
    Field { field: String, message: String },
}

impl ValidationError {
    pub fn field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Field {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("validation failed: {0}")]
    Validation(ValidationError),
    #[error("invalid run transition from {from:?} to {to:?}")]
    InvalidRunTransition {
        from: crate::run::RunStatus,
        to: crate::run::RunStatus,
    },
    #[error("schedule error: {0}")]
    Schedule(String),
}
