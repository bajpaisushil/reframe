//! Platform-independent decision logic of the Windows implementation.
//!
//! Everything in here works on plain data and never calls Win32, so it can be unit-tested on any
//! OS: on Windows it is compiled as `win32::logic`; elsewhere `lib.rs` compiles the very same
//! files for unit tests only (which is why unused-item warnings are silenced off Windows).
//!
//! The `win32` modules gather raw facts through small `unsafe` wrappers and hand them to these
//! functions to decide what to report or which Win32 values to pass.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

pub mod errors;
pub mod filter;
pub mod geometry;
pub mod icon;
pub mod launch;
pub mod locate;
pub mod monitor;
pub mod placement;
pub mod version;
