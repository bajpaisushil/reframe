//! Privacy permissions (TCC): Accessibility and Screen Recording checks and prompts.

use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// Whether Reframe is a trusted Accessibility client. Takes effect immediately after the user
/// grants it.
pub(super) fn accessibility_granted() -> bool {
    // SAFETY: plain FFI call without arguments.
    unsafe { AXIsProcessTrusted() }
}

/// Whether Reframe may read other applications' window titles (Screen Recording). macOS only
/// applies a newly granted permission after Reframe restarts.
pub(super) fn screen_recording_granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Shows the system "Reframe would like to control this computer using accessibility features"
/// prompt (asynchronously) when Reframe is not trusted yet. Returns whether it is trusted now.
pub(super) fn prompt_accessibility() -> bool {
    // SAFETY: `kAXTrustedCheckOptionPrompt` is an immutable CFString exported by HIServices.
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
    // SAFETY: the options dictionary maps the documented key to a CFBoolean.
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
}

/// Shows the system Screen Recording prompt. macOS only shows it the first time an application
/// asks; afterwards this returns `false` without any UI until the user changes the setting.
pub(super) fn request_screen_recording() -> bool {
    CGRequestScreenCaptureAccess()
}
