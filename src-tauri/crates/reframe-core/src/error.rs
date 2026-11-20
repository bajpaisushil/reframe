use reframe_platform::PlatformError;
use uuid::Uuid;

/// Errors surfaced by the workspace engine. The Tauri layer converts them into a serializable
/// `{ kind, message, details }` object for the UI.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{}", .0.join(" "))]
    Validation(Vec<String>),

    #[error("Workspace not found.")]
    WorkspaceNotFound(Uuid),

    #[error("{0} not found.")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("This file is not a valid Reframe workspace: {0}")]
    InvalidImport(String),

    #[error(transparent)]
    Platform(#[from] PlatformError),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Data error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("File error: {0}")]
    Io(#[from] std::io::Error),
}

impl CoreError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(vec![message.into()])
    }

    /// Machine-readable kind for the UI.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation",
            Self::WorkspaceNotFound(_) | Self::NotFound(_) => "notFound",
            Self::Conflict(_) => "conflict",
            Self::InvalidImport(_) => "invalidImport",
            Self::Platform(PlatformError::Unsupported { .. }) => "unsupported",
            Self::Platform(PlatformError::PermissionDenied { .. }) => "permissionDenied",
            Self::Platform(_) => "platform",
            Self::Database(_) => "database",
            Self::Serialization(_) => "serialization",
            Self::Io(_) => "io",
        }
    }

    /// Individual problems (for validation errors).
    pub fn details(&self) -> Vec<String> {
        match self {
            Self::Validation(problems) => problems.clone(),
            _ => Vec::new(),
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
