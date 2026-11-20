//! CoreFoundation helpers: reading CoreFoundation containers and converting values to the plain
//! data used by [`crate::macos_logic`]. Nothing here panics on unexpected value types; values
//! that cannot be represented are skipped.

use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};

use crate::macos_logic::{PlistDict, PlistValue};

/// Nesting depth converted by [`plist_value`] (window-info dictionaries nest one level).
const MAX_DEPTH: u8 = 4;

/// Converts a CoreFoundation property-list value (string, boolean, number or dictionary with
/// string keys) to plain data. Other types yield `None`.
pub(super) fn plist_value(value: &CFType) -> Option<PlistValue> {
    plist_value_at(value, MAX_DEPTH)
}

fn plist_value_at(value: &CFType, depth: u8) -> Option<PlistValue> {
    if let Some(string) = value.downcast_ref::<CFString>() {
        return Some(PlistValue::String(string.to_string()));
    }
    if let Some(boolean) = value.downcast_ref::<CFBoolean>() {
        return Some(PlistValue::Bool(boolean.as_bool()));
    }
    if let Some(number) = value.downcast_ref::<CFNumber>() {
        return if number.is_float_type() {
            number.as_f64().map(PlistValue::Float)
        } else {
            number.as_i64().map(PlistValue::Int)
        };
    }
    if depth > 0
        && let Some(dict) = value.downcast_ref::<CFDictionary>()
    {
        return Some(PlistValue::Dict(plist_dict_at(dict, depth - 1)));
    }
    None
}

fn plist_dict_at(dict: &CFDictionary, depth: u8) -> PlistDict {
    // SAFETY: only used on dictionaries created by CoreGraphics, which use the CFType key and
    // value callbacks, so every key and value is a CoreFoundation object.
    let typed = unsafe { dict.cast_unchecked::<CFType, CFType>() };
    let (keys, values) = typed.to_vecs();
    keys.iter()
        .zip(values.iter())
        .filter_map(|(key, value)| {
            let key = key.downcast_ref::<CFString>()?.to_string();
            Some((key, plist_value_at(value, depth)?))
        })
        .collect()
}

/// Looks `key` up in a dictionary whose keys are strings and whose values are CoreFoundation
/// objects (CoreGraphics window-info dictionaries).
pub(super) fn dict_get(dict: &CFDictionary, key: &CFString) -> Option<CFRetained<CFType>> {
    // SAFETY: the lookup hashes and compares `key` through the dictionary's own callbacks, and
    // values of CoreGraphics dictionaries are CoreFoundation objects (retained on return).
    unsafe { dict.cast_unchecked::<CFString, CFType>() }.get(key)
}

/// The elements of a CoreFoundation array returned by CoreGraphics or the Accessibility API
/// (each retained).
pub(super) fn array_items(array: &CFArray) -> Vec<CFRetained<CFType>> {
    // SAFETY: arrays produced by CoreGraphics and the Accessibility API hold CoreFoundation
    // objects (CFType callbacks).
    let typed = unsafe { array.cast_unchecked::<CFType>() };
    (0..typed.len())
        .filter_map(|index| typed.get(index))
        .collect()
}
