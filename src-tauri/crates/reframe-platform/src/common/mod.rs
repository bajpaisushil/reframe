//! Cross-platform helpers shared by the OS implementations.

pub mod launch;
pub mod locate;
pub mod process;
mod unsupported;

pub use unsupported::UnsupportedPlatform;
