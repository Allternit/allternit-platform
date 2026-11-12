//! Console pricing (Wave 2, lane L2): `GET /api/v1/platform/pricing` (meter
//! prices from `stripe_plan`, tool prices from `data/tool-prices.json`, model
//! deprecations). Clerk session auth behind `console_gate`, merged once in
//! `platform_v1::router_with_agency`. Empty until L2 lands.

use std::sync::Arc;

use axum::Router;

use crate::ApiState;

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new()
}
