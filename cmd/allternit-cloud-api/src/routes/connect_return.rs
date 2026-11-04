//! Public return for connector sign-ins that run on a user's own runtime.
//!
//! A paired runtime (allternit-api, `oauth_return.rs`) sends OAuth redirects here
//! instead of `http://127.0.0.1:8013`, so a sign-in started from the iPhone app,
//! ai.allternit.com or the PWA can finish on any device:
//!
//! - `GET /connect/return/connectors` → the runtime's `/api/v1/connectors/oauth/callback`
//! - `GET /connect/return/mcp` → `/mcp/oauth/callback`
//! - `GET /connect/return/sidecar/oauth/callback` → `/oauth/callback` (connector sidecar)
//! - `GET /connect/client.json` → the Client ID Metadata Document listing the MCP return
//!
//! The runtime is named in the OAuth `state` (`art.<runtime_id>.<random>`). This
//! route checks that shape, finds the runtime, relays the GET with its query intact
//! over the runtime socket and returns the runtime's page. Only those three
//! callback paths can be reached; nothing else on the runtime. The runtime still
//! matches the full state to its pending sign-in and checks PKCE, so this hop
//! only carries the browser back.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, RawQuery, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};

use crate::routes::runtime_relay::{relay_request_to_runtime_with, RelayRequest};
use crate::ApiState;

pub const STATE_TAG: &str = "art";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/connect/client.json", get(client_document))
        .route("/connect/return/sidecar/oauth/callback", get(sidecar_return))
        .route("/connect/return/:target", get(target_return))
}

/// The runtime callback a return target reaches; `None` for anything else.
pub fn local_path(target: &str) -> Option<&'static str> {
    match target {
        "connectors" => Some("/api/v1/connectors/oauth/callback"),
        "mcp" => Some("/mcp/oauth/callback"),
        "sidecar" => Some("/oauth/callback"),
        _ => None,
    }
}

fn valid_runtime_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The runtime id a tagged `state` names (same shape allternit-api writes).
pub fn runtime_from_state(state: &str) -> Option<&str> {
    let rest = state.strip_prefix(STATE_TAG)?.strip_prefix('.')?;
    let (id, inner) = rest.split_once('.')?;
    (valid_runtime_id(id) && !inner.is_empty()).then_some(id)
}

/// The runtime id from a callback query's `state`.
pub fn runtime_from_query(query: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "state")
        .and_then(|(_, v)| runtime_from_state(&v).map(str::to_string))
}

/// The CIMD document for sign-ins that return through the cloud.
pub fn client_metadata(base: &str) -> serde_json::Value {
    let base = base.trim_end_matches('/');
    serde_json::json!({
        "client_id": format!("{base}/connect/client.json"),
        "client_name": "Allternit",
        "client_uri": base,
        "redirect_uris": [format!("{base}/connect/return/mcp")],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })
}

async fn client_document() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=3600")],
        Json(client_metadata(&crate::routes::channel_inbound::public_base())),
    )
        .into_response()
}

async fn target_return(State(state): State<Arc<ApiState>>, Path(target): Path<String>, RawQuery(query): RawQuery) -> Response {
    // The sidecar's return has its own path (its redirect is `{origin}/oauth/callback`).
    if target == "sidecar" {
        return page(StatusCode::NOT_FOUND, "Sign-in link not recognised", "Start connecting again from Allternit.");
    }
    relay_return(&state, &target, query.unwrap_or_default()).await
}

async fn sidecar_return(State(state): State<Arc<ApiState>>, RawQuery(query): RawQuery) -> Response {
    relay_return(&state, "sidecar", query.unwrap_or_default()).await
}

async fn relay_return(state: &ApiState, target: &str, query: String) -> Response {
    let Some(path) = local_path(target) else {
        return page(StatusCode::NOT_FOUND, "Sign-in link not recognised", "Start connecting again from Allternit.");
    };
    let Some(runtime_id) = runtime_from_query(&query) else {
        return page(StatusCode::BAD_REQUEST, "Couldn’t finish signing in", "This sign-in link is incomplete. Start connecting again from Allternit.");
    };
    let owner = sqlx::query_scalar::<_, String>("SELECT user_id FROM runtime_devices WHERE id = $1 AND revoked_at IS NULL")
        .bind(&runtime_id)
        .fetch_optional(&state.db)
        .await;
    let user_id = match owner {
        Ok(Some(u)) => u,
        Ok(None) => return page(StatusCode::NOT_FOUND, "Couldn’t finish signing in", "The computer that started this sign-in is no longer linked to Allternit."),
        Err(e) => {
            tracing::warn!(%runtime_id, "connect return: runtime lookup failed: {e}");
            return page(StatusCode::SERVICE_UNAVAILABLE, "Couldn’t finish signing in", "Allternit couldn’t look up your computer. Try again in a moment.");
        }
    };
    // Never log the query: it carries the authorization code.
    let request = RelayRequest {
        method: "GET".into(),
        path: if query.is_empty() { path.to_string() } else { format!("{path}?{query}") },
        headers: HashMap::from([("accept".to_string(), "text/html".to_string())]),
        body: String::new(),
        body_encoding: "utf8".into(),
    };
    let response = relay_request_to_runtime_with(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &user_id,
        &runtime_id,
        request,
        &[],
        HashMap::new(),
    )
    .await;
    match response {
        // The runtime's own page (HTML, success or failure) goes to the browser as is.
        Ok(r) if !is_json(&r) => r,
        Ok(r) => match r.status() {
            StatusCode::SERVICE_UNAVAILABLE => page(
                StatusCode::SERVICE_UNAVAILABLE,
                "Your computer is offline",
                "This sign-in finishes on the computer that started it. Open Allternit on it (or wait for it to wake), then reload this page.",
            ),
            StatusCode::GATEWAY_TIMEOUT => page(StatusCode::GATEWAY_TIMEOUT, "Your computer didn’t answer", "Reload this page to try again."),
            s => page(s, "Couldn’t finish signing in", "Allternit couldn’t reach the computer that started this sign-in."),
        },
        Err(e) => {
            tracing::warn!(%runtime_id, "connect return: relay refused: {e}");
            page(StatusCode::BAD_GATEWAY, "Couldn’t finish signing in", "Allternit couldn’t reach the computer that started this sign-in.")
        }
    }
}

fn is_json(r: &Response) -> bool {
    r.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn page(status: StatusCode, title: &str, message: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{t} · Allternit</title><style>:root{{color-scheme:light dark;--bg:#fff;--fg:#1d1d1f;--muted:#6e6e73}}\
@media (prefers-color-scheme:dark){{:root{{--bg:#1c1c1e;--fg:#f5f5f7;--muted:#a1a1a6}}}}\
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:16px/1.5 -apple-system,system-ui,sans-serif}}\
main{{max-width:28rem;padding:2rem 1rem;text-align:center}}h1{{font-size:1.25rem;margin:0 0 .5rem}}p{{color:var(--muted);margin:0}}</style></head>\
<body><main><h1>{t}</h1><p>{m}</p></main></body></html>",
        t = escape(title),
        m = escape(message)
    );
    (status, Html(html)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_three_callbacks_are_reachable() {
        assert_eq!(local_path("connectors"), Some("/api/v1/connectors/oauth/callback"));
        assert_eq!(local_path("mcp"), Some("/mcp/oauth/callback"));
        assert_eq!(local_path("sidecar"), Some("/oauth/callback"));
        for other in ["", "api", "..", "mcp/../api", "connectors%2F..", "MCP", "oauth"] {
            assert_eq!(local_path(other), None, "{other}");
        }
    }

    #[test]
    fn runtime_comes_only_from_a_well_formed_state() {
        assert_eq!(runtime_from_query("code=abc&state=art.rt_0f3c.k-9_x").as_deref(), Some("rt_0f3c"));
        assert_eq!(runtime_from_query("state=art.rt_0f3c.a.b&code=1").as_deref(), Some("rt_0f3c"));
        for bad in [
            "code=abc",
            "state=",
            "state=0f3c-uuid",
            "state=art.rt_0f3c",
            "state=art.rt_0f3c.",
            "state=art..x",
            "state=art.rt%2F..%2Fapi.x",
            "state=art.rt%3Fx%3D1.x",
            "state=xart.rt.x",
        ] {
            assert_eq!(runtime_from_query(bad), None, "{bad}");
        }
    }

    #[test]
    fn client_document_lists_exactly_the_cloud_return() {
        let d = client_metadata("https://api.allternit.com/");
        assert_eq!(d["client_id"], "https://api.allternit.com/connect/client.json");
        assert_eq!(d["redirect_uris"], serde_json::json!(["https://api.allternit.com/connect/return/mcp"]));
        assert_eq!(d["token_endpoint_auth_method"], "none");
    }
}
