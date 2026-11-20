//! Removing sensitive data (credentials, tokens, secret-looking parameters) before export.

use crate::model::AppState;

/// Sanitize adapter state for export: strip URL credentials and secret-looking query
/// parameters, drop non-http(s)/file URLs that may embed tokens, clear unapproved startup
/// commands' approval, etc.
///
/// PLACEHOLDER (returns the state unchanged) until the portability implementation lands.
/// Export is disabled (`NotImplemented`) until then, so nothing unsanitized leaves the app.
pub fn sanitize_state(state: &AppState) -> AppState {
    state.clone()
}
