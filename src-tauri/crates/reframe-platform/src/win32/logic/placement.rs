//! Applying a window placement: how often `SetWindowPlacement` is applied, and the fallback for
//! when it is rejected.
//!
//! `SetWindowPlacement` sets the restored frame and the show state in one step. Some environments
//! reject it without giving a reason (compatibility layers such as Wine do not support it for
//! other processes' windows). The same result is then reached with the more widely supported
//! `ShowWindowAsync` and `SetWindowPos`: restore the window, move its restored frame, then apply
//! the requested state.

use super::errors::{ERROR_ACCESS_DENIED, ERROR_INVALID_WINDOW_HANDLE};
use crate::WindowState;

/// How many times `SetWindowPlacement` is applied to reach `target` with the requested restored
/// frame:
/// - once, normally;
/// - once more for a minimized window that restores maximized (`WPF_RESTORETOMAXIMIZED`, set when
///   a maximized window is minimized) and should become normal: `SW_SHOWNORMAL` only maximizes it,
///   the second call restores it to its normal frame;
/// - once more when the window moves to a monitor with a different DPI: per-monitor-aware
///   applications rescale themselves on arrival (`WM_DPICHANGED`), which can change the size just
///   applied, and applying the placement again yields the exact requested geometry.
pub fn placement_passes(
    current: WindowState,
    restores_maximized: bool,
    target: WindowState,
    dpi_changes: bool,
) -> usize {
    let unminimize_to_normal =
        current == WindowState::Minimized && restores_maximized && target == WindowState::Normal;
    1 + usize::from(unminimize_to_normal) + usize::from(dpi_changes)
}

/// One step of the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackStep {
    /// `ShowWindowAsync(SW_RESTORE)`.
    Restore,
    /// `SetWindowPos` to the outer frame, in screen coordinates.
    Move,
    /// `ShowWindowAsync(SW_SHOWMAXIMIZED)`.
    Maximize,
    /// `ShowWindowAsync(SW_SHOWMINNOACTIVE)`.
    Minimize,
}

/// Whether a failed `SetWindowPlacement` should be retried through the fallback. Not when Windows
/// said why it failed: access denied (an elevated target, which the fallback cannot move either)
/// or an invalid handle (the window is gone).
pub fn should_fall_back(win32_code: Option<u32>) -> bool {
    !matches!(
        win32_code,
        Some(ERROR_ACCESS_DENIED | ERROR_INVALID_WINDOW_HANDLE)
    )
}

/// Steps that take a window from `current` to `target`. The window is restored first so the move
/// applies to its restored frame; a minimized window needs two restores because one minimized
/// from the maximized state restores to maximized first.
pub fn fallback_steps(current: WindowState, target: WindowState) -> Vec<FallbackStep> {
    let mut steps = match current {
        WindowState::Normal => Vec::new(),
        WindowState::Maximized | WindowState::Fullscreen => vec![FallbackStep::Restore],
        WindowState::Minimized => vec![FallbackStep::Restore, FallbackStep::Restore],
    };
    steps.push(FallbackStep::Move);
    match target {
        WindowState::Normal => {}
        WindowState::Maximized | WindowState::Fullscreen => steps.push(FallbackStep::Maximize),
        WindowState::Minimized => steps.push(FallbackStep::Minimize),
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::FallbackStep::*;
    use super::*;

    #[test]
    fn known_failures_are_not_retried() {
        assert!(!should_fall_back(Some(ERROR_ACCESS_DENIED)));
        assert!(!should_fall_back(Some(ERROR_INVALID_WINDOW_HANDLE)));
        assert!(should_fall_back(None));
        assert!(should_fall_back(Some(87)));
    }

    #[test]
    fn placement_passes_cover_restore_to_maximized_and_dpi_changes() {
        use WindowState::*;
        assert_eq!(placement_passes(Normal, false, Maximized, false), 1);
        assert_eq!(placement_passes(Maximized, false, Normal, false), 1);
        assert_eq!(placement_passes(Minimized, false, Normal, false), 1);
        // Minimized from the maximized state: the first SW_SHOWNORMAL only maximizes.
        assert_eq!(placement_passes(Minimized, true, Normal, false), 2);
        // ...which is exactly right when maximized or minimized is wanted.
        assert_eq!(placement_passes(Minimized, true, Maximized, false), 1);
        assert_eq!(placement_passes(Minimized, true, Minimized, false), 1);
        assert_eq!(placement_passes(Minimized, true, Fullscreen, false), 1);
        // A DPI change adds one more pass.
        assert_eq!(placement_passes(Normal, false, Normal, true), 2);
        assert_eq!(placement_passes(Minimized, true, Normal, true), 3);
    }

    #[test]
    fn steps_restore_move_then_apply_the_state() {
        assert_eq!(
            fallback_steps(WindowState::Normal, WindowState::Normal),
            vec![Move]
        );
        assert_eq!(
            fallback_steps(WindowState::Normal, WindowState::Maximized),
            vec![Move, Maximize]
        );
        assert_eq!(
            fallback_steps(WindowState::Maximized, WindowState::Minimized),
            vec![Restore, Move, Minimize]
        );
        assert_eq!(
            fallback_steps(WindowState::Minimized, WindowState::Normal),
            vec![Restore, Restore, Move]
        );
        assert_eq!(
            fallback_steps(WindowState::Fullscreen, WindowState::Fullscreen),
            vec![Restore, Move, Maximize]
        );
    }
}
