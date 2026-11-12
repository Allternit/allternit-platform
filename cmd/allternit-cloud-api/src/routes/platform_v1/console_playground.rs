//! Console playground (Wave 1, lane L1B): the 15-minute playground key
//! (`POST /api/v1/platform/projects/:id/playground-key`), saved prompts and
//! their signed-in share links (`/api/v1/platform/shared-prompts/:token`).
//! Clerk session auth behind `console_gate`, merged once in
//! `platform_v1::router_with_agency`. Empty until L1B lands.

use std::sync::Arc;

use axum::Router;

use crate::ApiState;

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new()
}
