//! Console governance (Wave 4, lane L4B): audit log read
//! (`GET /api/v1/platform/audit-log`), org policy
//! (`GET,PUT /api/v1/platform/org/policy`) and key edits
//! (`PATCH /api/v1/platform/projects/:id/keys/:key_id`). Clerk session auth
//! behind `console_gate`, merged once in `platform_v1::router_with_agency`.
//! Empty until L4B lands.

use std::sync::Arc;

use axum::Router;

use crate::ApiState;

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new()
}
