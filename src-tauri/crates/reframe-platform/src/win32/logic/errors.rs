//! Mapping Win32 error codes of window operations to [`PlatformError`]s.

use crate::PlatformError;

/// `ERROR_ACCESS_DENIED`.
pub const ERROR_ACCESS_DENIED: u32 = 5;
/// `ERROR_INVALID_WINDOW_HANDLE`.
pub const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;

/// Explanation for operations blocked by User Interface Privilege Isolation.
pub const ELEVATED_WINDOW_MESSAGE: &str = "the window belongs to an application running as \
     administrator, and Windows does not let a non-elevated Reframe control it (User Interface \
     Privilege Isolation). Run Reframe as administrator to manage that window.";

/// Error for a failed operation on a window: access denied means the target is elevated (UIPI),
/// an invalid handle means the window is gone; anything else is reported as an OS error.
pub fn window_error(
    operation: &str,
    window: &str,
    win32_code: Option<u32>,
    detail: &str,
) -> PlatformError {
    match win32_code {
        Some(ERROR_ACCESS_DENIED) => PlatformError::PermissionDenied {
            operation: operation.to_string(),
            permission: None,
            message: ELEVATED_WINDOW_MESSAGE.to_string(),
        },
        Some(ERROR_INVALID_WINDOW_HANDLE) => window_not_found(window),
        _ => PlatformError::os(operation, detail),
    }
}

/// The window no longer exists (or the id never named one).
pub fn window_not_found(window: &str) -> PlatformError {
    PlatformError::not_found(format!("Window {window}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_is_a_permission_error() {
        let err = window_error(
            "Moving a window",
            "0x1a",
            Some(ERROR_ACCESS_DENIED),
            "denied",
        );
        match &err {
            PlatformError::PermissionDenied {
                operation,
                permission,
                message,
            } => {
                assert_eq!(operation, "Moving a window");
                assert_eq!(*permission, None);
                assert!(message.contains("administrator"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            err.to_string()
                .starts_with("Moving a window needs permission")
        );
    }

    #[test]
    fn invalid_handle_is_not_found() {
        assert_eq!(
            window_error(
                "Closing a window",
                "0x1a",
                Some(ERROR_INVALID_WINDOW_HANDLE),
                "x"
            ),
            PlatformError::not_found("Window 0x1a")
        );
        assert_eq!(
            window_not_found("0x2").to_string(),
            "Window 0x2 was not found"
        );
    }

    #[test]
    fn other_codes_are_os_errors() {
        assert_eq!(
            window_error(
                "Focusing a window",
                "0x1a",
                Some(87),
                "The parameter is incorrect."
            ),
            PlatformError::os("Focusing a window", "The parameter is incorrect.")
        );
        assert_eq!(
            window_error("Focusing a window", "0x1a", None, "unknown"),
            PlatformError::os("Focusing a window", "unknown")
        );
    }
}
