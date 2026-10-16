// owned by WP2 (stub from the WP1 scaffold: real signatures, no-op bodies).
//! Lane health (contract C5): the lane status board (`GET /api/v1/lanes/status`)
//! and the per-channel-binding breaker. Stub until WP2 lands.

use std::sync::Arc;

use axum::Router;
use rusqlite::Connection;

use crate::AppState;

/// Whether posts on `binding_id` may go out (the channel breaker is closed).
/// Stub: always open.
pub fn channel_lane_open(_conn: &Connection, _binding_id: &str) -> bool {
    true
}

/// `GET /lanes/status` lives here once WP2 lands. Stub: no routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}

/// Lane samplers. Stub: nothing runs.
pub fn spawn(_state: Arc<AppState>) {}
