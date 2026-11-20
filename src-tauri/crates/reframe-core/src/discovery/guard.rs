//! Running adapter code defensively.
//!
//! Adapters are the extension point of the engine, so discovery and capture never trust them
//! not to panic: every call goes through [`guarded`]. A panicking adapter is skipped (during
//! identification) or replaced by the generic adapter for that application, and the user gets
//! one warning per failing adapter.
//!
//! Limitation: this relies on unwinding. In a build with `panic = "abort"` a panicking adapter
//! still terminates the process.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use reframe_platform::{ProcessInfo, WindowInfo};

use crate::adapters::generic::GenericAdapter;
use crate::adapters::{AdapterRegistry, AppAdapter, AppIdentity};
use crate::util;

/// Run `f`, turning a panic into `Err(panic message)`.
pub(crate) fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string())
    })
}

/// Adapter failures collected during one scan/capture, reported as one warning per adapter.
#[derive(Debug, Default)]
pub(crate) struct AdapterIssues {
    /// (adapter name — `None` for the generic adapter —, affected applications) in
    /// first-failure order.
    failures: Vec<(Option<String>, Vec<String>)>,
}

impl AdapterIssues {
    /// Record that `adapter` (see [`failure_label`]) panicked while handling `app`.
    pub fn record(&mut self, adapter: Option<String>, app: &str, operation: &str, message: &str) {
        log::warn!(
            "{} panicked during {operation} for {app:?}: {message}",
            adapter.as_deref().unwrap_or("the generic adapter")
        );
        let index = match self.failures.iter().position(|(name, _)| *name == adapter) {
            Some(index) => index,
            None => {
                self.failures.push((adapter, Vec::new()));
                self.failures.len() - 1
            }
        };
        let apps = &mut self.failures[index].1;
        if !apps.iter().any(|a| a == app) {
            apps.push(app.to_string());
        }
    }

    pub fn into_warnings(self) -> Vec<String> {
        self.failures
            .into_iter()
            .map(|(adapter, apps)| match adapter {
                Some(adapter) => format!(
                    "The {adapter} integration failed for {}; basic support was used instead.",
                    list_names(&apps)
                ),
                None => format!(
                    "Reframe could not inspect {}; only basic launch information is available.",
                    list_names(&apps)
                ),
            })
            .collect()
    }
}

/// How a failing adapter is named in warnings: its quoted display name, or `None` for the
/// generic adapter (whose failure leaves no further fallback).
pub(crate) fn failure_label(
    registry: &AdapterRegistry,
    adapter: &Arc<dyn AppAdapter>,
) -> Option<String> {
    (!Arc::ptr_eq(adapter, &registry.generic())).then(|| adapter_name(adapter.as_ref()))
}

/// "A", "A and B", "A, B and C", "A, B, C and 4 more".
fn list_names(names: &[String]) -> String {
    const SHOWN: usize = 3;
    match names {
        [] => "an application".to_string(),
        [one] => one.clone(),
        [rest @ .., last] if names.len() <= SHOWN => format!("{} and {last}", rest.join(", ")),
        _ => format!(
            "{} and {} more",
            names[..SHOWN].join(", "),
            names.len() - SHOWN
        ),
    }
}

/// The adapter's display name, or a neutral label if even that panics.
pub(crate) fn adapter_name(adapter: &dyn AppAdapter) -> String {
    guarded(|| adapter.name())
        .map(|name| format!("\"{name}\""))
        .unwrap_or_else(|_| "an application".to_string())
}

/// Friendly label for a process in messages.
pub(crate) fn process_label(process: &ProcessInfo) -> String {
    if let Some(name) = process
        .product_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        return name.to_string();
    }
    let raw = process
        .executable_path
        .as_deref()
        .map(util::exe_stem)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| process.name.clone());
    let pretty = util::prettify_name(&raw);
    if pretty.is_empty() {
        "an application".to_string()
    } else {
        pretty
    }
}

/// Identity used when every adapter, including the generic one, failed.
pub(crate) fn fallback_identity(process: &ProcessInfo) -> AppIdentity {
    AppIdentity {
        app_id: GenericAdapter::app_id_for(process),
        display_name: process_label(process),
    }
}

/// An application recognised by an adapter.
pub(crate) struct Identified {
    pub adapter: Arc<dyn AppAdapter>,
    pub identity: AppIdentity,
}

/// Like [`AdapterRegistry::identify`], but a panicking adapter is skipped (and recorded) so the
/// next adapter — ultimately the generic one — gets the process.
pub(crate) fn identify(
    registry: &AdapterRegistry,
    process: &ProcessInfo,
    windows: &[&WindowInfo],
    issues: &mut AdapterIssues,
) -> Identified {
    for adapter in registry.all() {
        match guarded(|| adapter.identify(process, windows)) {
            Ok(Some(identity)) => {
                return Identified {
                    adapter: Arc::clone(adapter),
                    identity,
                };
            }
            Ok(None) => {}
            Err(message) => issues.record(
                failure_label(registry, adapter),
                &process_label(process),
                "identify",
                &message,
            ),
        }
    }
    Identified {
        adapter: registry.generic(),
        identity: fallback_identity(process),
    }
}

/// Result of [`with_fallback`]: the adapter and identity actually used.
pub(crate) struct Outcome<T> {
    pub adapter_id: String,
    pub identity: AppIdentity,
    /// The generic adapter produced this result (identified it, or replaced a failing adapter).
    pub generic: bool,
    /// `None` when even the generic adapter failed.
    pub value: Option<T>,
}

/// Run `op` with the identified adapter. If it panics (or its `id()` does), record the failure
/// and redo the work with the generic adapter, re-identifying the application with it so the
/// result is a consistent generic entry.
pub(crate) fn with_fallback<T>(
    registry: &AdapterRegistry,
    identified: Identified,
    main: &ProcessInfo,
    windows: &[&WindowInfo],
    issues: &mut AdapterIssues,
    mut op: impl FnMut(&dyn AppAdapter, &AppIdentity) -> T,
) -> Outcome<T> {
    let Identified { adapter, identity } = identified;
    let generic = registry.generic();
    let is_generic = Arc::ptr_eq(&adapter, &generic);
    match guarded(|| (adapter.id(), op(adapter.as_ref(), &identity))) {
        Ok((id, value)) => {
            return Outcome {
                adapter_id: id.to_string(),
                identity,
                generic: is_generic,
                value: Some(value),
            };
        }
        Err(message) => issues.record(
            failure_label(registry, &adapter),
            &identity.display_name,
            "capture",
            &message,
        ),
    }

    let identity = if is_generic {
        identity
    } else {
        guarded(|| generic.identify(main, windows))
            .ok()
            .flatten()
            .unwrap_or_else(|| fallback_identity(main))
    };
    if !is_generic {
        match guarded(|| op(generic.as_ref(), &identity)) {
            Ok(value) => {
                return Outcome {
                    adapter_id: guarded(|| generic.id()).unwrap_or("generic").to_string(),
                    identity,
                    generic: true,
                    value: Some(value),
                };
            }
            Err(message) => issues.record(None, &identity.display_name, "capture", &message),
        }
    }
    Outcome {
        adapter_id: guarded(|| generic.id()).unwrap_or("generic").to_string(),
        identity,
        generic: true,
        value: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_reports_panic_messages() {
        assert_eq!(guarded(|| 7), Ok(7));
        let err = guarded(|| -> u32 { panic!("boom {}", 1) }).unwrap_err();
        assert_eq!(err, "boom 1");
        let err = guarded(|| -> u32 { std::panic::panic_any(42_u8) }).unwrap_err();
        assert_eq!(err, "unknown panic");
    }

    #[test]
    fn issues_are_grouped_per_adapter() {
        assert!(AdapterIssues::default().into_warnings().is_empty());
        let mut issues = AdapterIssues::default();
        let broken = || Some("\"Broken\"".to_string());
        for app in ["A", "B", "A", "C", "D", "E"] {
            issues.record(broken(), app, "capture", "boom");
        }
        issues.record(Some("\"Other\"".into()), "Z", "identify", "boom");
        issues.record(None, "Y", "capture", "boom");
        issues.record(None, "X", "identify", "boom");
        let warnings = issues.into_warnings();
        assert_eq!(
            warnings,
            vec![
                "The \"Broken\" integration failed for A, B, C and 2 more; basic support was used instead."
                    .to_string(),
                "The \"Other\" integration failed for Z; basic support was used instead."
                    .to_string(),
                "Reframe could not inspect Y and X; only basic launch information is available."
                    .to_string(),
            ]
        );
    }

    #[test]
    fn the_generic_adapter_has_no_failure_label() {
        let test: Arc<dyn AppAdapter> = crate::discovery::test_support::TestAdapter::new("x");
        let registry = AdapterRegistry::with_adapters(vec![Arc::clone(&test)]);
        assert_eq!(failure_label(&registry, &registry.generic()), None);
        assert_eq!(
            failure_label(&registry, &test).as_deref(),
            Some("\"Test integration\"")
        );
    }

    #[test]
    fn name_lists() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(list_names(&names(&[])), "an application");
        assert_eq!(list_names(&names(&["A", "B"])), "A and B");
        assert_eq!(list_names(&names(&["A", "B", "C"])), "A, B and C");
    }
}
