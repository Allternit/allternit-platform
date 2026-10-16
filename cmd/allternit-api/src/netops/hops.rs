// owned by WP3 (stub from the WP1 scaffold: real signatures, no-op bodies).
//! Agent hops and trace context (contract C3): W3C `traceparent`
//! (`00-<32 hex trace id>-<16 hex span id>-01`), hop receipts, the loop guard
//! and the thread timeline. Stub until WP3 lands.

use std::sync::Arc;

use axum::Router;
use rusqlite::Connection;

use crate::AppState;

/// A parsed W3C trace context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceParent {
    pub trace_id: String,
    pub span_id: String,
    pub sampled: bool,
}

impl TraceParent {
    /// The header form, `00-<trace>-<span>-<flags>`.
    pub fn header(&self) -> String {
        format!("00-{}-{}-{}", self.trace_id, self.span_id, if self.sampled { "01" } else { "00" })
    }
}

/// Parse a `traceparent` header. Stub: always `None`.
pub fn parse(_header: &str) -> Option<TraceParent> {
    None
}

/// A child span of `parent` (same trace, new span id). Stub: returns the parent unchanged.
pub fn child(parent: &TraceParent) -> TraceParent {
    parent.clone()
}

/// The active trace of a thread, as a `traceparent` header value. Stub: `None`.
pub fn traceparent_for_thread(_conn: &Connection, _thread_id: &str) -> Option<String> {
    None
}

/// `GET /threads/:id/timeline` lives here once WP3 lands. Stub: no routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}

/// Stub: nothing runs.
pub fn spawn(_state: Arc<AppState>) {}
