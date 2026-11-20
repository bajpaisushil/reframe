//! Displays: CoreGraphics (usable from any thread) for the display list, bounds and identity;
//! `NSScreen` (main thread only) for names, usable areas and backing scale factors.

use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFRetained, CFUUID, CGRect};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayIsBuiltin,
    CGDisplayMode, CGError, CGGetActiveDisplayList, CGMainDisplayID,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

use crate::macos_logic::{DisplayRecord, FRect, ScreenRecord};
use crate::{PlatformError, PlatformResult};

// Declared here instead of using `objc2-color-sync`, whose binding assumes a non-NULL result and
// panics otherwise; NULL is possible for a display that was just disconnected.
#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> Option<NonNull<CFUUID>>;
}

/// More displays than any Mac supports.
const MAX_DISPLAYS: u32 = 32;

fn frect(rect: CGRect) -> FRect {
    FRect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

/// Active displays in CoreGraphics order (the main display first).
pub(super) fn display_records() -> PlatformResult<Vec<DisplayRecord>> {
    let mut ids = [0 as CGDirectDisplayID; MAX_DISPLAYS as usize];
    let mut count: u32 = 0;
    // SAFETY: both pointers are valid for `MAX_DISPLAYS` / one element.
    let error = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        return Err(PlatformError::os(
            "Listing monitors",
            format!(
                "CGGetActiveDisplayList failed with CoreGraphics error {}",
                error.0
            ),
        ));
    }
    let main = CGMainDisplayID();
    let count = usize::try_from(count).unwrap_or(0).min(ids.len());
    Ok(ids[..count]
        .iter()
        .map(|&id| DisplayRecord {
            id,
            bounds: frect(CGDisplayBounds(id)),
            uuid: display_uuid(id),
            is_main: id == main,
            is_builtin: CGDisplayIsBuiltin(id),
            mode_scale: mode_scale(id),
        })
        .collect())
}

/// The display's UUID, stable across reboots and reconnections.
fn display_uuid(id: CGDirectDisplayID) -> Option<String> {
    // SAFETY: plain FFI call; the result follows the Create rule (+1 reference).
    let raw = unsafe { CGDisplayCreateUUIDFromDisplayID(id) }?;
    let uuid = unsafe { CFRetained::from_raw(raw) };
    CFUUID::new_string(None, Some(&uuid)).map(|s| s.to_string())
}

/// Pixel density of the current display mode (2.0 for a Retina "looks like" mode).
fn mode_scale(id: CGDirectDisplayID) -> Option<f64> {
    let mode = CGDisplayCopyDisplayMode(id)?;
    let points = CGDisplayMode::width(Some(&mode));
    let pixels = CGDisplayMode::pixel_width(Some(&mode));
    (points > 0 && pixels > 0).then(|| pixels as f64 / points as f64)
}

fn read_screen(screen: &NSScreen, number_key: &NSString) -> ScreenRecord {
    // `deviceDescription` and `localizedName` are declared non-null, and the generated bindings
    // panic if AppKit returns nil anyway; asking for optional results keeps a nil harmless.
    // SAFETY: both are argument-less methods returning an object (or nil).
    let description: Option<Retained<NSDictionary<NSString, AnyObject>>> =
        unsafe { msg_send![screen, deviceDescription] };
    let name: Option<Retained<NSString>> = unsafe { msg_send![screen, localizedName] };
    let display_id = description
        .and_then(|d| d.objectForKey(number_key))
        .and_then(|value| {
            value
                .downcast_ref::<NSNumber>()
                .map(NSNumber::unsignedIntValue)
        });
    ScreenRecord {
        display_id,
        name: name.map(|n| n.to_string()),
        frame: frect(screen.frame()),
        visible_frame: frect(screen.visibleFrame()),
        scale: screen.backingScaleFactor(),
    }
}

/// `None` if reading panicked (a panic must never escape into the main-queue callback, where
/// it would abort the application).
fn read_screens() -> Option<Vec<ScreenRecord>> {
    std::panic::catch_unwind(|| {
        let mtm = MainThreadMarker::new()?;
        Some(autoreleasepool(|_| {
            let number_key = NSString::from_str("NSScreenNumber");
            NSScreen::screens(mtm)
                .iter()
                .map(|screen| read_screen(&screen, &number_key))
                .collect()
        }))
    })
    .ok()
    .flatten()
}

/// Reads every `NSScreen`. AppKit only allows this on the main thread: called elsewhere, the
/// read is dispatched to the main queue and awaited for at most `timeout`. Returns `None` when
/// the main thread does not answer in time (e.g. a host without a running main run loop); the
/// late result is then discarded.
pub(super) fn screens(timeout: Duration) -> Option<Vec<ScreenRecord>> {
    if MainThreadMarker::new().is_some() {
        return read_screens();
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        if let Some(records) = read_screens() {
            // The receiver is gone if the caller timed out; nothing to do then.
            let _ = sender.send(records);
        }
    });
    receiver.recv_timeout(timeout).ok()
}
