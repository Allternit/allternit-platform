//! Console pricing (Wave 2, lane L2): `GET /api/v1/platform/pricing` (meter
//! prices from `stripe_plan`, tool prices from `data/tool-prices.json`, model
//! deprecations). Clerk session auth behind `console_gate`, merged once in
//! `platform_v1::router_with_agency`.
//!
//! `data/tool-prices.json` ships as an empty array until Eoj sets hosted-tool
//! prices. Row schema: `{"tool": "web_search", "unit": "1k calls",
//! "price_usd": 10.0, "provider": "exa"}`. Rows are returned as written.
//!
//! Deprecations: the static model catalog carries no retirement date, so this
//! returns an empty list until it does. The console also reads
//! `expiration_date` from the live `/v1/models` list.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde_json::{json, Value};

use super::{console, stripe_plan, PlatformError};
use crate::ApiState;

const TOOL_PRICES: &str = include_str!("../../../data/tool-prices.json");

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/platform/pricing", get(get_pricing))
}

fn pricing_body() -> Value {
    let meters: Vec<Value> = stripe_plan::METER_PLANS
        .iter()
        .map(|m| json!({ "meter": m.meters[0], "name": m.display_name, "unit": m.unit, "payg_cents": m.payg_cents, "growth_cents": m.growth_cents }))
        .collect();
    let tools: Value = serde_json::from_str(TOOL_PRICES).unwrap_or_else(|_| json!([]));
    json!({ "meters": meters, "tools": tools, "deprecations": [] })
}

async fn get_pricing(State(_state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>, PlatformError> {
    console::principal(&headers).await?;
    Ok(Json(pricing_body()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::super::{projects::Principal, router_gated, Gate, TestSession};
    use crate::routes::test_support::{test_state, MockGateway};

    #[test]
    fn pricing_lists_named_meters_and_no_tools() {
        let v = super::pricing_body();
        let meters = v["meters"].as_array().unwrap();
        assert!(!meters.is_empty());
        assert!(meters.iter().all(|m| m["name"].as_str().is_some_and(|n| !n.is_empty())));
        assert!(v["tools"].as_array().unwrap().is_empty());
        assert!(v["deprecations"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pricing_needs_a_session() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let who = Principal { user_id: "u1".into(), org_id: None, org_admin: false };
        let app = router_gated(&state, Gate::Forced(true)).layer(axum::Extension(TestSession(who))).with_state(state.clone());
        let req = Request::builder().uri("/api/v1/platform/pricing").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
