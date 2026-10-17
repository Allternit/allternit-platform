//! Agent network operations (Phase 1 "observable and robust", 1b "injection
//! defense"). One module per shared contract, so `lib.rs`, `main.rs` and the
//! router are touched once:
//!
//! | module       | contract | owner | what it does |
//! |--------------|----------|-------|--------------|
//! | [`alerts`]   | C1, C2   | WP1   | attention items, re-notify, daily digest, critical SMS (off by default) |
//! | [`lanes`]    | C5       | WP2   | lane status board, channel breaker |
//! | [`hops`]     | C3       | WP3   | W3C trace context, hop receipts, loop guard |
//! | [`presence`] | C6       | WP4   | runtime heartbeat to the cloud |
//! | [`provenance`] | C4     | WP5   | trust labels, nonce fences |
//!
//! [`router`] is merged into the v1 API by `inbox_routes::inbox_router`;
//! [`spawn`] is called once from `main` beside the local routine scheduler.

use std::sync::Arc;

use axum::Router;

use crate::AppState;

pub mod alerts;
pub mod hops;
pub mod lanes;
pub mod presence;
pub mod provenance;
pub mod sms_verify;

/// Every netops route (nested under `/api/v1`, behind auth).
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(alerts::router())
        .merge(lanes::router())
        .merge(hops::router())
        .merge(presence::router())
        .merge(provenance::router())
        .merge(sms_verify::router())
}

/// Start every netops background task.
pub fn spawn(state: Arc<AppState>) {
    alerts::spawn(state.clone());
    lanes::spawn(state.clone());
    hops::spawn(state.clone());
    presence::spawn(state.clone());
    provenance::spawn(state);
}
