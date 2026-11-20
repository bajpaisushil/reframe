//! A small safe layer over the HIServices Accessibility API (`AXUIElement`), built on
//! `objc2-application-services`.
//!
//! Every call is a synchronous request to the target application, answered by its main thread.
//! The process-wide messaging timeout is lowered to [`MESSAGING_TIMEOUT`] (see
//! [`set_global_timeout`]) so a busy or hung application cannot stall Reframe for the default
//! six seconds. All functions return the raw `AXError` code on failure; callers turn it into a
//! [`crate::PlatformError`] with [`crate::macos_logic::ax_error`].

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CGPoint, CGSize,
};

use super::cf;
use crate::macos_logic::{FRect, ax_code};

/// Seconds an Accessibility request may take before it fails with "cannot complete".
pub(super) const MESSAGING_TIMEOUT: f32 = 1.0;

/// Attribute and action names: the string values of the `kAX…Attribute` / `kAX…Action`
/// constants, which are C macros and therefore not exported by the framework bindings.
pub(super) mod name {
    pub(crate) const WINDOWS: &str = "AXWindows";
    pub(crate) const FOCUSED_WINDOW: &str = "AXFocusedWindow";
    pub(crate) const TITLE: &str = "AXTitle";
    pub(crate) const POSITION: &str = "AXPosition";
    pub(crate) const SIZE: &str = "AXSize";
    pub(crate) const MINIMIZED: &str = "AXMinimized";
    pub(crate) const FULL_SCREEN: &str = "AXFullScreen";
    pub(crate) const SUBROLE: &str = "AXSubrole";
    pub(crate) const MAIN: &str = "AXMain";
    pub(crate) const CLOSE_BUTTON: &str = "AXCloseButton";
    pub(crate) const RAISE: &str = "AXRaise";
    pub(crate) const PRESS: &str = "AXPress";
}

// Declared here instead of using `objc2-application-services`, whose bindings assume these never
// return NULL and panic if they do.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> Option<NonNull<AXUIElement>>;
    fn AXUIElementCreateSystemWide() -> Option<NonNull<AXUIElement>>;
}

/// Result of an Accessibility call: the `AXError` code on failure.
pub(super) type AxResult<T> = Result<T, i32>;

fn check(code: i32) -> AxResult<()> {
    if code == ax_code::SUCCESS {
        Ok(())
    } else {
        Err(code)
    }
}

/// Lowers the messaging timeout for every Accessibility request made by this process
/// (best effort; failure leaves the system default in place).
pub(super) fn set_global_timeout() {
    // SAFETY: plain FFI call; the result follows the Create rule (+1 reference).
    if let Some(raw) = unsafe { AXUIElementCreateSystemWide() } {
        let system = AxElement(unsafe { CFRetained::from_raw(raw) });
        system.set_messaging_timeout(MESSAGING_TIMEOUT);
    }
}

/// An accessibility element: an application, a window, a button...
pub(super) struct AxElement(CFRetained<AXUIElement>);

impl AxElement {
    /// The top-level element of the application with process id `pid`. Creating it always
    /// succeeds for a valid pid; requests fail later if the process does not answer.
    pub(super) fn application(pid: u32) -> Option<Self> {
        let pid = libc::pid_t::try_from(pid).ok().filter(|&pid| pid > 0)?;
        // SAFETY: plain FFI call; the result follows the Create rule (+1 reference).
        let raw = unsafe { AXUIElementCreateApplication(pid) }?;
        let element = Self(unsafe { CFRetained::from_raw(raw) });
        element.set_messaging_timeout(MESSAGING_TIMEOUT);
        Some(element)
    }

    pub(super) fn set_messaging_timeout(&self, seconds: f32) {
        // SAFETY: valid element. A failure only keeps the previous timeout.
        let _ = unsafe { self.0.set_messaging_timeout(seconds) };
    }

    /// The raw value of an attribute; `Ok(None)` when the attribute has no value or the element
    /// does not support it.
    pub(super) fn attribute(&self, attribute: &str) -> AxResult<Option<CFRetained<CFType>>> {
        let attribute = CFString::from_str(attribute);
        let mut value: *const CFType = std::ptr::null();
        // SAFETY: `value` is a valid out-pointer; on success it receives a +1 reference (or NULL).
        let code = unsafe {
            self.0
                .copy_attribute_value(&attribute, NonNull::from(&mut value))
        }
        .0;
        match code {
            ax_code::SUCCESS => Ok(NonNull::new(value.cast_mut())
                // SAFETY: the copied value is owned by us (Copy rule).
                .map(|value| unsafe { CFRetained::from_raw(value) })),
            ax_code::NO_VALUE | ax_code::ATTRIBUTE_UNSUPPORTED => Ok(None),
            code => Err(code),
        }
    }

    /// Several attributes read with a single request when the application supports it (one
    /// request per attribute otherwise). Missing or unsupported attributes are `None`.
    pub(super) fn attributes(
        &self,
        attributes: &[&str],
    ) -> AxResult<Vec<Option<CFRetained<CFType>>>> {
        let names: Vec<CFRetained<CFString>> = attributes
            .iter()
            .map(|name| CFString::from_str(name))
            .collect();
        let name_refs: Vec<&CFString> = names.iter().map(|name| &**name).collect();
        let array = CFArray::<CFString>::from_objects(&name_refs);
        let mut values: *const CFArray = std::ptr::null();
        // SAFETY: `values` is a valid out-pointer; on success it receives a +1 reference.
        let code = unsafe {
            self.0.copy_multiple_attribute_values(
                array.as_opaque(),
                AXCopyMultipleAttributeOptions(0),
                NonNull::from(&mut values),
            )
        }
        .0;
        if code == ax_code::SUCCESS
            && let Some(values) = NonNull::new(values.cast_mut())
        {
            // SAFETY: the copied array is owned by us (Copy rule).
            let values = unsafe { CFRetained::from_raw(values) };
            let items = cf::array_items(&values);
            if items.len() == attributes.len() {
                // Failed attributes are AXValues of the AXError type or kCFNull; the typed
                // decoders below treat them as missing.
                return Ok(items.into_iter().map(Some).collect());
            }
        }
        match code {
            ax_code::API_DISABLED | ax_code::INVALID_UI_ELEMENT | ax_code::CANNOT_COMPLETE => {
                Err(code)
            }
            _ => attributes.iter().map(|name| self.attribute(name)).collect(),
        }
    }

    pub(super) fn boolean(&self, attribute: &str) -> AxResult<Option<bool>> {
        Ok(self.attribute(attribute)?.as_deref().and_then(to_bool))
    }

    /// `AXPosition` + `AXSize` as one rectangle (CoreGraphics global space).
    pub(super) fn frame(&self) -> AxResult<Option<FRect>> {
        let values = self.attributes(&[name::POSITION, name::SIZE])?;
        Ok(to_frame(
            values.first().and_then(|v| v.as_deref()),
            values.get(1).and_then(|v| v.as_deref()),
        ))
    }

    /// An attribute whose value is another element (`AXFocusedWindow`, `AXCloseButton`...).
    pub(super) fn element(&self, attribute: &str) -> AxResult<Option<AxElement>> {
        Ok(self
            .attribute(attribute)?
            .and_then(|value| value.downcast::<AXUIElement>().ok())
            .map(AxElement))
    }

    /// An attribute whose value is an array of elements (`AXWindows`).
    pub(super) fn elements(&self, attribute: &str) -> AxResult<Vec<AxElement>> {
        let Some(value) = self.attribute(attribute)? else {
            return Ok(Vec::new());
        };
        let Ok(array) = value.downcast::<CFArray>() else {
            return Ok(Vec::new());
        };
        Ok(cf::array_items(&array)
            .into_iter()
            .filter_map(|item| item.downcast::<AXUIElement>().ok())
            .map(AxElement)
            .collect())
    }

    fn set(&self, attribute: &str, value: &CFType) -> AxResult<()> {
        let attribute = CFString::from_str(attribute);
        // SAFETY: valid element, attribute name and value.
        check(unsafe { self.0.set_attribute_value(&attribute, value) }.0)
    }

    pub(super) fn set_bool(&self, attribute: &str, value: bool) -> AxResult<()> {
        self.set(attribute, CFBoolean::new(value))
    }

    pub(super) fn set_position(&self, x: f64, y: f64) -> AxResult<()> {
        let point = CGPoint::new(x, y);
        // SAFETY: the pointer refers to a CGPoint, matching the declared value type.
        let value = unsafe { AXValue::new(AXValueType::CGPoint, NonNull::from(&point).cast()) }
            .ok_or(ax_code::FAILURE)?;
        self.set(name::POSITION, &value)
    }

    pub(super) fn set_size(&self, width: f64, height: f64) -> AxResult<()> {
        let size = CGSize::new(width, height);
        // SAFETY: the pointer refers to a CGSize, matching the declared value type.
        let value = unsafe { AXValue::new(AXValueType::CGSize, NonNull::from(&size).cast()) }
            .ok_or(ax_code::FAILURE)?;
        self.set(name::SIZE, &value)
    }

    pub(super) fn perform(&self, action: &str) -> AxResult<()> {
        let action = CFString::from_str(action);
        // SAFETY: valid element and action name.
        check(unsafe { self.0.perform_action(&action) }.0)
    }

    /// The CoreGraphics window number of a window element.
    ///
    /// Uses `_AXUIElementGetWindow`, a private but long-stable HIServices function used by most
    /// macOS window managers. It is looked up at run time, so if a future macOS removes it this
    /// returns `None` and callers fall back to matching windows by geometry and title.
    pub(super) fn window_id(&self) -> Option<u32> {
        let get_window = window_id_function()?;
        let mut id: u32 = 0;
        // SAFETY: valid element and out-pointer; the function only reads the element.
        let code = unsafe { get_window(CFRetained::as_ptr(&self.0).as_ptr(), &mut id) };
        (code == ax_code::SUCCESS && id != 0).then_some(id)
    }

    /// Whether both values refer to the same user-interface element (`CFEqual`).
    pub(super) fn same_as(&self, other: &AxElement) -> bool {
        let a: &CFType = &self.0;
        let b: &CFType = &other.0;
        a == b
    }
}

type GetWindowFn = unsafe extern "C" fn(*const AXUIElement, *mut u32) -> i32;

fn window_id_function() -> Option<GetWindowFn> {
    static FUNCTION: OnceLock<Option<GetWindowFn>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // SAFETY: NUL-terminated symbol name; RTLD_DEFAULT searches every loaded image
        // (HIServices is loaded because this crate links ApplicationServices).
        let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"_AXUIElementGetWindow".as_ptr()) };
        if symbol.is_null() {
            log::debug!("_AXUIElementGetWindow is unavailable; matching windows by geometry");
            None
        } else {
            // SAFETY: `AXError _AXUIElementGetWindow(AXUIElementRef, CGWindowID *)` has had this
            // signature since Mac OS X 10.6.
            Some(unsafe { std::mem::transmute::<*mut c_void, GetWindowFn>(symbol) })
        }
    })
}

/// Decodes a `CFString` attribute value.
pub(super) fn to_string(value: &CFType) -> Option<String> {
    value.downcast_ref::<CFString>().map(|s| s.to_string())
}

/// Decodes a boolean attribute value (`CFBoolean`, or a number used as a flag).
pub(super) fn to_bool(value: &CFType) -> Option<bool> {
    if let Some(boolean) = value.downcast_ref::<CFBoolean>() {
        return Some(boolean.as_bool());
    }
    value
        .downcast_ref::<CFNumber>()
        .and_then(CFNumber::as_i64)
        .map(|n| n != 0)
}

fn to_point(value: &CFType) -> Option<(f64, f64)> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut point = CGPoint::ZERO;
    // SAFETY: the out-pointer refers to a CGPoint; AXValueGetValue checks the stored type.
    let ok = unsafe { value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) };
    (ok && point.x.is_finite() && point.y.is_finite()).then_some((point.x, point.y))
}

fn to_size(value: &CFType) -> Option<(f64, f64)> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut size = CGSize::ZERO;
    // SAFETY: the out-pointer refers to a CGSize; AXValueGetValue checks the stored type.
    let ok = unsafe { value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) };
    (ok && size.width.is_finite() && size.height.is_finite()).then_some((size.width, size.height))
}

/// Combines `AXPosition` and `AXSize` values into a frame.
pub(super) fn to_frame(position: Option<&CFType>, size: Option<&CFType>) -> Option<FRect> {
    let (x, y) = to_point(position?)?;
    let (width, height) = to_size(size?)?;
    let frame = FRect::new(x, y, width, height);
    frame.is_valid().then_some(frame)
}
