//! Runtime side of the public MCP edge.
//!
//! `mcp.allternit.com` is served by cloud-api, because Claude and ChatGPT can't
//! reach a user's Mac or cloud computer. The cloud verifies the OAuth token
//! (`aud`, `bots:act` / `agents:read`, Clerk signature), finds the owner's
//! runtime, and forwards the JSON-RPC body here over the signed relay, with the
//! verified user in `x-allternit-owner`. [`RelayedAuth`] checks that signature
//! (so only cloud-api can reach these paths, and only as the user this runtime
//! is paired as); the connector's own owner checks then run unchanged.
//!
//! * `POST /webhooks/mcp-edge/bots/:vendorBotId` — the vendor-bot connector.
//!   `x-allternit-mcp-client` names the OAuth client (for the keys page and the
//!   revoke check).
//! * `POST /webhooks/mcp-edge/server` — the read-only agents server.
//!
//! A vendor bot this runtime doesn't hold answers 404 `{"error":"not_found"}`;
//! the cloud then tries the owner's next runtime.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::post,
    Router,
};

use crate::relay_auth::{secret_layer, RelaySecret, RelayedAuth};
use crate::AppState;

pub const BOTS_PATH_PREFIX: &str = "/webhooks/mcp-edge/bots";
pub const SERVER_PATH: &str = "/webhooks/mcp-edge/server";
/// Header cloud-api sets to the OAuth client behind the call.
pub const CLIENT_HEADER: &str = "x-allternit-mcp-client";

pub fn mcp_edge_router() -> Router<Arc<AppState>> {
    mcp_edge_router_with(crate::relay_auth::process_secret())
}

pub fn mcp_edge_router_with(secret: Arc<dyn RelaySecret>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/webhooks/mcp-edge/bots/:vendor_bot_id", post(bot_rpc))
        .route(SERVER_PATH, post(agents_rpc))
        .layer(secret_layer(secret))
}

async fn bot_rpc(State(state): State<Arc<AppState>>, Path(vendor_bot_id): Path<String>, headers: HeaderMap, auth: RelayedAuth) -> Response {
    let client = headers.get(CLIENT_HEADER).and_then(|v| v.to_str().ok()).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
    let req = match auth.json::<serde_json::Value>() {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    crate::mcp_vendor_bots::serve_bot_rpc(&state, &auth.owner, &vendor_bot_id, client, &req).await
}

async fn agents_rpc(State(state): State<Arc<AppState>>, auth: RelayedAuth) -> Response {
    crate::mcp_server_routes::relayed_agents_rpc(&state, &auth.owner, &auth.body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_auth::{relayed_post, StaticRelaySecret};
    use axum::http::StatusCode;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    const TOKEN: &str = "device-token";

    async fn app(tag: &str) -> Router {
        let st = crate::aai_facade::test_util::setup(tag, "READY").await;
        mcp_edge_router_with(Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: "user-a".into() })).with_state(st)
    }

    async fn call(app: &Router, path: &str, body: &Value, signed_as: Option<(&str, &str)>, client: Option<&str>) -> (StatusCode, Value) {
        let mut req = relayed_post(path, body.to_string().as_bytes(), signed_as);
        if let Some(c) = client {
            req.headers_mut().insert(CLIENT_HEADER, c.parse().unwrap());
        }
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn list() -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })
    }

    #[tokio::test]
    async fn an_unsigned_or_wrongly_signed_call_never_reaches_the_connector() {
        let app = app("edge-unsigned").await;
        let path = "/webhooks/mcp-edge/bots/bot-vendor";
        assert_eq!(call(&app, path, &list(), None, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(&app, path, &list(), Some(("other-token", "user-a")), None).await.0, StatusCode::UNAUTHORIZED);
        // Signed by this runtime's key but for a different owner than it is paired as.
        assert_eq!(call(&app, path, &list(), Some((TOKEN, "user-b")), None).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_signed_call_runs_the_connector_as_the_verified_owner() {
        let app = app("edge-owner").await;
        let (status, body) = call(&app, "/webhooks/mcp-edge/bots/bot-vendor", &list(), Some((TOKEN, "user-a")), Some("claude-connector")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["tools"].as_array().map(Vec::len), Some(crate::mcp_vendor_bots::tool_descriptors().len()), "{body}");
    }

    #[tokio::test]
    async fn a_bot_this_runtime_does_not_hold_is_a_plain_404() {
        let app = app("edge-404").await;
        for bot in ["nope", "bot-native"] {
            let (status, body) = call(&app, &format!("/webhooks/mcp-edge/bots/{bot}"), &list(), Some((TOKEN, "user-a")), None).await;
            assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")), "{bot}");
        }
    }

    #[tokio::test]
    async fn a_revoked_client_is_refused_on_its_next_call() {
        let st = crate::aai_facade::test_util::setup("edge-revoked", "READY").await;
        let app = mcp_edge_router_with(Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: "user-a".into() })).with_state(st.clone());
        let path = "/webhooks/mcp-edge/bots/bot-vendor";
        assert_eq!(call(&app, path, &list(), Some((TOKEN, "user-a")), Some("claude-connector")).await.0, StatusCode::OK);
        st.db.connect().unwrap().execute("UPDATE vendor_connector_clients SET revoked_at = '2026-01-01T00:00:00Z' WHERE client = 'claude-connector'", []).unwrap();
        assert_eq!(call(&app, path, &list(), Some((TOKEN, "user-a")), Some("claude-connector")).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_agents_server_is_reachable_read_only() {
        let app = app("edge-agents").await;
        let (status, body) = call(&app, SERVER_PATH, &list(), Some((TOKEN, "user-a")), None).await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = body["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"list_agents") && !names.iter().any(|n| n.starts_with("shell")), "{names:?}");
    }
}
