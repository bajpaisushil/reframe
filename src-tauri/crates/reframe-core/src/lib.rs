//! # reframe-core
//!
//! The Reframe workspace engine, independent of Tauri and of any specific operating system:
//!
//! - [`model`] — workspaces, snapshots, restore runs, settings (with generated TS bindings)
//! - [`adapters`] — per-application capture/restore integrations and their registry
//! - [`discovery`] / [`capture`] — "Explore" and turning running apps into workspace entries
//! - [`restoration`] — DAG planning, orchestrated launches, window matching & placement
//! - [`switching`] — Replace / Merge / Additive planning
//! - [`database`] — local SQLite persistence
//! - [`portability`] — JSON import/export with machine-specific path mapping
//! - [`service`] — the facade used by the Tauri command layer

pub mod adapters;
pub mod capture;
pub mod database;
pub mod discovery;
pub mod error;
pub mod health;
pub mod model;
pub mod portability;
pub mod restoration;
pub mod service;
pub mod switching;
pub mod util;

pub use error::{CoreError, CoreResult};
pub use service::ReframeService;
