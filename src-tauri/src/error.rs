//! Serializable command errors: `{ kind, message, details }` (see `ApiError` in src/lib/api.ts).

use reframe_core::CoreError;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: String,
    pub message: String,
    pub details: Vec<String>,
}

impl CommandError {
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: "internal".into(),
            message: message.into(),
            details: Vec::new(),
        }
    }
}

impl From<CoreError> for CommandError {
    fn from(e: CoreError) -> Self {
        if matches!(
            e,
            CoreError::Database(_) | CoreError::Io(_) | CoreError::Serialization(_)
        ) {
            log::error!("{e}");
        }
        Self {
            kind: e.kind().to_string(),
            message: e.to_string(),
            details: e.details(),
        }
    }
}

pub type CommandResult<T> = Result<T, CommandError>;
