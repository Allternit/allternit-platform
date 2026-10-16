// owned by WP4 (stub from the WP1 scaffold: real signatures, no-op bodies).
//! Runtime presence (contract C6): a 60 s heartbeat to the cloud
//! (`POST /api/v1/runtime-presence/heartbeat`, relay-signed) carrying the
//! routines due in the next 24 h, so the cloud can tell the owner when a
//! scheduled run is missed because this computer is asleep. Stub until WP4 lands.

use std::sync::Arc;

use axum::Router;
use serde_json::Value;

use crate::AppState;

/// The heartbeat body (`{runtimeId, nextDue: [{routineId, title, at}]}`). Stub: empty object.
pub fn heartbeat_payload(_conn: &rusqlite::Connection, _owner: &str) -> Value {
    Value::Object(Default::default())
}

/// Stub: no routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}

/// The heartbeat loop. Stub: nothing runs.
pub fn spawn(_state: Arc<AppState>) {}
