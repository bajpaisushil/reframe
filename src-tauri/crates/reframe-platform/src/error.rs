use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::PermissionKind;

/// Errors from platform operations. Unsupported operations must use
/// [`PlatformError::Unsupported`] with a reason a user can understand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum PlatformError {
    #[error("{operation} is not supported here: {reason}")]
    Unsupported { operation: String, reason: String },

    #[error("{operation} needs permission: {message}")]
    PermissionDenied {
        operation: String,
        permission: Option<PermissionKind>,
        message: String,
    },

    #[error("{what} was not found")]
    NotFound { what: String },

    #[error("could not launch {program}: {message}")]
    LaunchFailed { program: String, message: String },

    #[error("{operation} failed: {message}")]
    Os { operation: String, message: String },
}

impl PlatformError {
    pub fn unsupported(operation: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Unsupported {
            operation: operation.into(),
            reason: reason.into(),
        }
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::NotFound { what: what.into() }
    }

    pub fn os(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Os {
            operation: operation.into(),
            message: message.into(),
        }
    }

    pub fn launch_failed(program: impl Into<String>, message: impl Into<String>) -> Self {
        Self::LaunchFailed {
            program: program.into(),
            message: message.into(),
        }
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }
}

pub type PlatformResult<T> = Result<T, PlatformError>;
