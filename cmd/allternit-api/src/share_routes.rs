//! Local share routes: start, list, pause, rotate and stop live-link gates.
//!
//! Apps + MCP 2026 program, plan v2 step 16. Served on the runtime's own API
//! port (127.0.0.1:18013 in dev, :8013 packaged), behind `auth_middleware`
//! like every other local route.
//!
//! | Method + path                 | Body                                   | Answer |
//! |-------------------------------|----------------------------------------|--------|
//! | `GET /shares`                 |                                        | `[Share]` (`[]` when none) |
//! | `POST /shares`                | `{port, subject?, auth_mode?, label?, share_link_id?}` | 201 `{share, token}` |
//! | `POST /shares/:id/pause`      | `{paused?}` (default true)             | `{share}` |
//! | `POST /shares/:id/rotate`     |                                        | `{share, token}` |
//! | `POST /shares/:id/stop`, `DELETE /shares/:id` |                        | `{stopped: true, id}` |
//!
//! `Share` = `{id, port, gate_port, gate_url, subject, auth_mode, status,
//! label, share_link_id, created_at}`. `token` is shown only on create and
//! rotate; the runtime keeps its hash. Errors are `{error, message}`:
//! 400 `invalid_port` / `public_mcp_refused`, 403 `port_refused`,
//! 404 `share_not_found`, 409 `already_shared`, 429 `too_many_shares`.
//!
//! The cloud registry (`/api/v1/share-links` in cloud-api) and the OpenTunnel
//! client stay with the caller (Desktop share-manager, gizzi, agent-daemon):
//! they hold the user's cloud session. The caller passes the registry id as
//! `share_link_id` so the two records can be matched, and points the tunnel
//! at `gate_url`. Gates live in memory and end with the process; the caller
//! re-creates them after a runtime restart.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::share_gate::{spawn_gate, GatePolicy, RunningGate, ShareAuthMode, ShareRefusal, ShareSubject};

/// Most gates one runtime keeps open at once (the cloud caps live links at 3
/// per account; this only bounds local listeners).
pub const MAX_LOCAL_SHARES: usize = 10;

/// Owner recorded when no signed-in user reached the route.
const LOCAL_OWNER: &str = "local";

struct LocalShare {
    owner: String,
    label: Option<String>,
    share_link_id: Option<String>,
    created_at: String,
    gate: RunningGate,
}

impl LocalShare {
    fn view(&self, id: &str) -> Value {
        let policy = &self.gate.policy;
        json!({
            "id": id,
            "port": policy.target_port,
            "gate_port": self.gate.port(),
            "gate_url": format!("http://127.0.0.1:{}", self.gate.port()),
            "subject": policy.subject,
            "auth_mode": policy.auth_mode,
            "status": if policy.is_paused() { "paused" } else { "active" },
            "label": self.label,
            "share_link_id": self.share_link_id,
            "created_at": self.created_at,
        })
    }
}

/// The live gates of this runtime.
#[derive(Default)]
pub struct ShareRegistry {
    shares: tokio::sync::Mutex<HashMap<String, LocalShare>>,
}

static GLOBAL_REGISTRY: Lazy<Arc<ShareRegistry>> = Lazy::new(|| Arc::new(ShareRegistry::default()));

/// The `/shares` routes on the process-wide registry.
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    router_with(GLOBAL_REGISTRY.clone())
}

/// The `/shares` routes on a given registry (tests use a fresh one).
pub fn router_with<S: Clone + Send + Sync + 'static>(registry: Arc<ShareRegistry>) -> Router<S> {
    Router::new()
        .route("/shares", get(list_shares).post(create_share))
        .route("/shares/:id", axum::routing::delete(stop_share))
        .route("/shares/:id/stop", post(stop_share))
        .route("/shares/:id/pause", post(pause_share))
        .route("/shares/:id/rotate", post(rotate_share))
        .layer(Extension(registry))
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": code, "message": message.into() }))).into_response()
}

fn refusal(refusal: ShareRefusal) -> Response {
    let status = match refusal {
        ShareRefusal::InternalPort(_) => StatusCode::FORBIDDEN,
        ShareRefusal::InvalidPort | ShareRefusal::PublicMcp => StatusCode::BAD_REQUEST,
    };
    let mut body = json!({ "error": refusal.code(), "message": refusal.message() });
    if let ShareRefusal::InternalPort(port) = refusal {
        body["port"] = json!(port);
    }
    (status, Json(body)).into_response()
}

fn owner_of(user: &Option<Extension<AuthUser>>) -> String {
    user.as_ref().map(|Extension(u)| u.user_id.clone()).unwrap_or_else(|| LOCAL_OWNER.to_string())
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "share_not_found", "No share with that id.")
}

async fn list_shares(
    Extension(registry): Extension<Arc<ShareRegistry>>,
    user: Option<Extension<AuthUser>>,
) -> Response {
    let owner = owner_of(&user);
    let shares = registry.shares.lock().await;
    let mut rows: Vec<(&String, &LocalShare)> = shares.iter().filter(|(_, s)| s.owner == owner).collect();
    rows.sort_by(|a, b| a.1.created_at.cmp(&b.1.created_at));
    Json(Value::Array(rows.into_iter().map(|(id, s)| s.view(id)).collect())).into_response()
}

#[derive(Deserialize)]
struct CreateShare {
    port: u16,
    #[serde(default)]
    subject: Option<ShareSubject>,
    #[serde(default)]
    auth_mode: Option<ShareAuthMode>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    share_link_id: Option<String>,
}

async fn create_share(
    Extension(registry): Extension<Arc<ShareRegistry>>,
    user: Option<Extension<AuthUser>>,
    Json(body): Json<CreateShare>,
) -> Response {
    let owner = owner_of(&user);
    let subject = body.subject.unwrap_or(ShareSubject::App);
    let auth_mode = body.auth_mode.unwrap_or_default();
    let label = body.label.map(|l| l.trim().chars().take(80).collect::<String>()).filter(|l| !l.is_empty());
    let share_link_id = body.share_link_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    let mut shares = registry.shares.lock().await;
    // Drop gates whose listener already ended.
    shares.retain(|_, s| !s.gate.policy.is_stopped());
    if let Some((id, _)) = shares
        .iter()
        .find(|(_, s)| s.owner == owner && s.gate.policy.target_port == body.port)
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "already_shared",
                "message": format!("Port {} is already shared.", body.port),
                "id": id,
            })),
        )
            .into_response();
    }
    if shares.len() >= MAX_LOCAL_SHARES {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_shares",
            format!("This computer already has {MAX_LOCAL_SHARES} shares open. Stop one first."),
        );
    }
    // No gate may point at another gate.
    let gate_ports: BTreeSet<u16> = shares.values().map(|s| s.gate.port()).collect();
    let id = format!("shr_{}", uuid::Uuid::new_v4().simple());
    let (policy, token) = match GatePolicy::new(id.clone(), body.port, subject, auth_mode, &gate_ports) {
        Ok(pair) => pair,
        Err(why) => return refusal(why),
    };
    let gate = match spawn_gate(policy).await {
        Ok(gate) => gate,
        Err(e) => {
            tracing::warn!(error = %e, "share gate failed to start");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "gate_start_failed", "The share gate could not start.");
        }
    };
    let share = LocalShare {
        owner,
        label,
        share_link_id,
        created_at: chrono::Utc::now().to_rfc3339(),
        gate,
    };
    let view = share.view(&id);
    shares.insert(id, share);
    (StatusCode::CREATED, Json(json!({ "share": view, "token": token }))).into_response()
}

#[derive(Deserialize, Default)]
struct PauseShare {
    #[serde(default)]
    paused: Option<bool>,
}

async fn pause_share(
    Extension(registry): Extension<Arc<ShareRegistry>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    body: Option<Json<PauseShare>>,
) -> Response {
    let owner = owner_of(&user);
    let paused = body.and_then(|Json(b)| b.paused).unwrap_or(true);
    let shares = registry.shares.lock().await;
    match shares.get(&id).filter(|s| s.owner == owner) {
        Some(share) => {
            share.gate.policy.set_paused(paused);
            Json(json!({ "share": share.view(&id) })).into_response()
        }
        None => not_found(),
    }
}

async fn rotate_share(
    Extension(registry): Extension<Arc<ShareRegistry>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> Response {
    let owner = owner_of(&user);
    let shares = registry.shares.lock().await;
    match shares.get(&id).filter(|s| s.owner == owner) {
        Some(share) => {
            let token = share.gate.policy.rotate_token();
            Json(json!({ "share": share.view(&id), "token": token })).into_response()
        }
        None => not_found(),
    }
}

async fn stop_share(
    Extension(registry): Extension<Arc<ShareRegistry>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> Response {
    let owner = owner_of(&user);
    let mut shares = registry.shares.lock().await;
    if shares.get(&id).map(|s| s.owner != owner).unwrap_or(true) {
        return not_found();
    }
    // Dropping the RunningGate stops the listener and every open stream.
    shares.remove(&id);
    Json(json!({ "stopped": true, "id": id })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        router_with::<()>(Arc::new(ShareRegistry::default()))
    }

    async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(v) => {
                builder = builder.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let response = app.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn shares_list_is_empty_array_when_none() {
        let (status, body) = call(&app(), "GET", "/shares", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn shares_create_refuses_runtime_ports() {
        let app = app();
        for port in [8013u16, 18013, 4096] {
            let (status, body) = call(&app, "POST", "/shares", Some(json!({ "port": port }))).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "port {port}");
            assert_eq!(body["error"], "port_refused");
        }
        let (status, body) =
            call(&app, "POST", "/shares", Some(json!({ "port": 5173, "subject": "mcp", "auth_mode": "public" }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "public_mcp_refused");
        let (_, list) = call(&app, "GET", "/shares", None).await;
        assert_eq!(list, json!([]));
    }

    #[tokio::test]
    async fn shares_create_pause_stop_lifecycle() {
        let app = app();
        // Hold a free port so nothing else takes it; no app needs to answer.
        let holder = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = holder.local_addr().unwrap().port();

        let (status, created) = call(&app, "POST", "/shares", Some(json!({ "port": port, "label": "demo" }))).await;
        assert_eq!(status, StatusCode::CREATED);
        let id = created["share"]["id"].as_str().unwrap().to_string();
        assert!(created["token"].as_str().unwrap().starts_with("ast_"));
        assert_eq!(created["share"]["auth_mode"], "token");
        assert_eq!(created["share"]["status"], "active");
        let gate_port = created["share"]["gate_port"].as_u64().unwrap();

        // A share can't target another share's gate.
        let (status, _) = call(&app, "POST", "/shares", Some(json!({ "port": gate_port }))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Same port twice → 409.
        let (status, dup) = call(&app, "POST", "/shares", Some(json!({ "port": port }))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(dup["id"], id.as_str());

        let (status, paused) = call(&app, "POST", &format!("/shares/{id}/pause"), Some(json!({}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(paused["share"]["status"], "paused");
        let (_, resumed) = call(&app, "POST", &format!("/shares/{id}/pause"), Some(json!({ "paused": false }))).await;
        assert_eq!(resumed["share"]["status"], "active");

        let (_, list) = call(&app, "GET", "/shares", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);

        let (status, stopped) = call(&app, "DELETE", &format!("/shares/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stopped["stopped"], true);
        let (_, list) = call(&app, "GET", "/shares", None).await;
        assert_eq!(list, json!([]));
        let (status, _) = call(&app, "POST", &format!("/shares/{id}/stop"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        drop(holder);
    }
}
