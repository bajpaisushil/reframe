//! Applications through AppKit: running applications (`NSWorkspace`, `NSRunningApplication`),
//! bundle lookups (Launch Services, `NSBundle`), activation and icons.
//!
//! `NSWorkspace` and `NSRunningApplication` may be used from any thread; their state is updated
//! by the main run loop, which the Tauri application always runs.

use std::path::{Path, PathBuf};

use objc2::AllocAnyThread;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSApplicationActivationPolicy, NSBitmapImageFileType,
    NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImageInterpolation, NSRunningApplication, NSWorkspace,
};
use objc2_foundation::{NSBundle, NSDictionary, NSPoint, NSRect, NSSize, NSString, NSURL};

use crate::macos_logic::AppRecord;
use crate::{PlatformError, PlatformResult};

fn pid_of(app: &NSRunningApplication) -> Option<u32> {
    u32::try_from(app.processIdentifier())
        .ok()
        .filter(|&pid| pid != 0)
}

fn running_app(pid: u32) -> Option<objc2::rc::Retained<NSRunningApplication>> {
    let pid = libc::pid_t::try_from(pid).ok().filter(|&pid| pid > 0)?;
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
}

/// Every running application known to Launch Services (regular, accessory and background).
pub(super) fn running_applications() -> Vec<AppRecord> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| !app.isTerminated())
        .filter_map(|app| {
            Some(AppRecord {
                pid: pid_of(&app)?,
                bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
                name: app.localizedName().map(|s| s.to_string()),
                bundle_path: app.bundleURL().and_then(|url| url.to_file_path()),
                regular: app.activationPolicy() == NSApplicationActivationPolicy::Regular,
                hidden: app.isHidden(),
            })
        })
        .collect()
}

/// The application that owns the menu bar.
pub(super) fn frontmost_pid() -> Option<u32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| pid_of(&app))
}

/// Bundle (or, for unbundled applications, executable) path of a running application.
pub(super) fn application_path(pid: u32) -> Option<PathBuf> {
    let app = running_app(pid)?;
    app.bundleURL()
        .or_else(|| app.executableURL())
        .and_then(|url| url.to_file_path())
}

/// Asks macOS to activate an application (bring its main window to the front). Returns
/// whether the request was accepted. Since macOS 14 activation is cooperative and the system
/// may decline it while the user works in another application.
pub(super) fn activate(pid: u32) -> bool {
    let Some(app) = running_app(pid) else {
        return false;
    };
    // Deprecated in macOS 14, where it no longer has an effect; still required before that for
    // the activation to win over the currently active application.
    #[allow(deprecated)]
    let options = NSApplicationActivationOptions::ActivateIgnoringOtherApps;
    app.activateWithOptions(options)
}

/// Where Launch Services finds the application with this bundle identifier.
pub(super) fn url_for_bundle_id(bundle_id: &str) -> Option<PathBuf> {
    NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
        .and_then(|url| url.to_file_path())
}

/// `CFBundleIdentifier` of the bundle at `path`.
pub(super) fn bundle_identifier(path: &Path) -> Option<String> {
    let url = NSURL::from_file_path(path)?;
    NSBundle::bundleWithURL(&url)?
        .bundleIdentifier()
        .map(|id| id.to_string())
        .filter(|id| !id.trim().is_empty())
}

/// The Finder icon of `path` rendered at exactly `pixels` x `pixels` and encoded as PNG.
///
/// `NSWorkspace.iconForFile` returns a multi-representation image; drawing it into a bitmap of
/// the requested size (rather than taking its TIFF representation, whose first page may have
/// any size) makes AppKit pick and scale the best representation.
pub(super) fn icon_png(path: &Path, pixels: u32) -> PlatformResult<Option<Vec<u8>>> {
    const OPERATION: &str = "Reading an application icon";
    let Some(path) = path.to_str() else {
        return Ok(None);
    };
    let side = isize::try_from(pixels)
        .map_err(|_| PlatformError::os(OPERATION, "icon size out of range"))?;
    let points = f64::from(pixels);
    let image = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path));

    // SAFETY: NULL planes make AppKit allocate the pixel buffer; the remaining parameters
    // describe a valid 8-bit RGBA bitmap (bytes per row / bits per pixel computed by AppKit),
    // and `NSDeviceRGBColorSpace` is an immutable AppKit constant.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }
    .ok_or_else(|| PlatformError::os(OPERATION, "could not allocate a bitmap"))?;
    bitmap.setSize(NSSize::new(points, points));

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
        .ok_or_else(|| PlatformError::os(OPERATION, "could not create a drawing context"))?;
    // Graphics contexts are per thread; drawing into a bitmap is allowed off the main thread.
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setImageInterpolation(NSImageInterpolation::High);
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::ZERO, NSSize::new(points, points)),
        NSRect::ZERO, // the whole image
        NSCompositingOperation::Copy,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
    // SAFETY: an empty property dictionary is valid for every file type.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }
    .ok_or_else(|| PlatformError::os(OPERATION, "could not encode the icon as PNG"))?;
    let bytes = png.to_vec();
    Ok((!bytes.is_empty()).then_some(bytes))
}
