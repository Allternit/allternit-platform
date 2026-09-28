//! Runtime Backend Preference API routes

use axum::{
    extract::{Extension, Json, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

fn self_port() -> u16 {
    crate::config::AppConfig::load().api_port()
}

pub fn runtime_backend_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runtime-backend", get(runtime_backend_status))
        .route(
            "/runtime/backend",
            get(get_runtime_backend).post(set_runtime_backend),
        )
        .route("/runtime/backend/manual", post(register_manual_backend))
        .route(
            "/runtime/execution-mode",
            get(get_runtime_execution_mode).put(set_runtime_execution_mode),
        )
}

async fn runtime_backend_status() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "runtime-backend",
    }))
}

#[derive(Serialize, Clone)]
struct RuntimeExecutionModePayload {
    mode: String,
    updated_at: String,
    supported_modes: Vec<String>,
}

#[derive(Deserialize)]
struct SetExecutionModeBody {
    mode: String,
}

fn execution_mode_store() -> &'static Mutex<RuntimeExecutionModePayload> {
    static STORE: OnceLock<Mutex<RuntimeExecutionModePayload>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(RuntimeExecutionModePayload {
            mode: "safe".to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            supported_modes: vec!["plan".to_string(), "safe".to_string(), "auto".to_string()],
        })
    })
}

async fn get_runtime_execution_mode() -> impl IntoResponse {
    match execution_mode_store().lock() {
        Ok(guard) => Json(guard.clone()).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "execution_mode_unavailable" })),
        )
            .into_response(),
    }
}

async fn set_runtime_execution_mode(Json(body): Json<SetExecutionModeBody>) -> impl IntoResponse {
    let mode = body.mode;
    if !matches!(mode.as_str(), "plan" | "safe" | "auto") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_execution_mode",
                "supported_modes": ["plan", "safe", "auto"],
            })),
        )
            .into_response();
    }

    match execution_mode_store().lock() {
        Ok(mut guard) => {
            guard.mode = mode;
            guard.updated_at = chrono::Utc::now().to_rfc3339();
            Json(guard.clone()).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "execution_mode_unavailable" })),
        )
            .into_response(),
    }
}

// ─── GET /runtime/backend ─────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct BackendTargetRow {
    id: String,
    ssh_connection_id: Option<String>,
    name: String,
    status: String,
    install_state: String,
    backend_url: Option<String>,
    gateway_url: Option<String>,
    gateway_ws_url: Option<String>,
    installed_version: Option<String>,
    supported_client_range: Option<String>,
    last_verified_at: Option<String>,
    last_heartbeat_at: Option<String>,
    last_error: Option<String>,
}

async fn get_runtime_backend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = headers
        .get("x-allternit-user-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();
    let port = self_port();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Read preference
        let pref: (String, String, Option<String>) = conn
            .query_row(
                "SELECT mode, fallback_mode, active_remote_backend_target_id
             FROM user_backend_preferences WHERE user_id = ?1",
                params![user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or(("local".to_string(), "local".to_string(), None));

        // List all targets for this user
        let mut stmt = conn.prepare(
            "SELECT id, ssh_connection_id, name, status, install_state, backend_url, gateway_url,
                    gateway_ws_url, installed_version, supported_client_range,
                    last_verified_at, last_heartbeat_at, last_error
             FROM remote_backend_targets WHERE user_id = ?1 ORDER BY updated_at DESC",
        )?;
        let targets: Vec<BackendTargetRow> = stmt
            .query_map(params![user_id], |row| {
                Ok(BackendTargetRow {
                    id: row.get(0)?,
                    ssh_connection_id: row.get(1)?,
                    name: row.get(2)?,
                    status: row.get(3)?,
                    install_state: row.get(4)?,
                    backend_url: row.get(5)?,
                    gateway_url: row.get(6)?,
                    gateway_ws_url: row.get(7)?,
                    installed_version: row.get(8)?,
                    supported_client_range: row.get(9)?,
                    last_verified_at: row.get(10)?,
                    last_heartbeat_at: row.get(11)?,
                    last_error: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Find active backend
        let active = if let Some(ref target_id) = pref.2 {
            targets.iter().find(|t| t.id == *target_id).cloned()
        } else {
            None
        };

        Ok::<_, rusqlite::Error>((pref, active, targets))
    })
    .await;

    match result {
        Ok(Ok((pref, active, targets))) => {
            let gateway_url = active
                .as_ref()
                .and_then(|a| a.gateway_url.clone())
                .unwrap_or_else(|| format!("http://127.0.0.1:{}", port));
            let gateway_ws_url = active
                .as_ref()
                .and_then(|a| a.gateway_ws_url.clone())
                .unwrap_or_else(|| format!("ws://127.0.0.1:{}", port));

            let active_backend = active.map(|a| {
                json!({
                    "id": a.id,
                    "ssh_connection_id": a.ssh_connection_id,
                    "name": a.name,
                    "status": a.status,
                    "install_state": a.install_state,
                    "backend_url": a.backend_url,
                    "gateway_url": a.gateway_url,
                    "gateway_ws_url": a.gateway_ws_url,
                    "installed_version": a.installed_version,
                    "supported_client_range": a.supported_client_range,
                    "last_verified_at": a.last_verified_at,
                    "last_heartbeat_at": a.last_heartbeat_at,
                    "last_error": a.last_error,
                })
            });

            Json(json!({
                "mode": pref.0,
                "fallback_mode": pref.1,
                "source": "desktop",
                "gateway_url": gateway_url,
                "gateway_ws_url": gateway_ws_url,
                "active_backend": active_backend,
                "available_backends": targets,
            }))
            .into_response()
        }
        _ => {
            // Return local default on error
            Json(json!({
                "mode": "local",
                "fallback_mode": "local",
                "source": "desktop",
                "gateway_url": format!("http://127.0.0.1:{}", port),
                "gateway_ws_url": format!("ws://127.0.0.1:{}", port),
                "active_backend": {
                    "id": "local-desktop",
                    "ssh_connection_id": null,
                    "name": "Allternit Desktop (local)",
                    "status": "ready",
                    "install_state": "installed",
                    "backend_url": format!("http://127.0.0.1:{}", port),
                    "gateway_url": format!("http://127.0.0.1:{}", port),
                    "gateway_ws_url": format!("ws://127.0.0.1:{}", port),
                    "installed_version": env!("CARGO_PKG_VERSION"),
                    "supported_client_range": null,
                    "last_verified_at": null,
                    "last_heartbeat_at": null,
                    "last_error": null,
                },
                "available_backends": [],
            }))
            .into_response()
        }
    }
}

// ─── POST /runtime/backend ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SetBackendBody {
    mode: Option<String>,
    #[serde(alias = "fallbackMode")]
    fallback_mode: Option<String>,
    #[serde(alias = "backendTargetId")]
    backend_target_id: Option<String>,
    #[serde(alias = "sshConnectionId")]
    ssh_connection_id: Option<String>,
}

async fn set_runtime_backend(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<SetBackendBody>,
) -> impl IntoResponse {
    let mode = body.mode.unwrap_or_else(|| "local".to_string());
    let fallback = body.fallback_mode.unwrap_or_else(|| "local".to_string());

    let db = state.db.clone();
    let user_id = user.user_id;
    let target_id = body.backend_target_id.clone();
    let ssh_conn_id = body.ssh_connection_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Resolve target_id from sshConnectionId if needed
        let resolved_target = if target_id.is_some() {
            target_id
        } else if let Some(ref ssh_id) = ssh_conn_id {
            conn.query_row(
                "SELECT id FROM remote_backend_targets WHERE user_id = ?1 AND ssh_connection_id = ?2",
                params![user_id, ssh_id],
                |row| row.get::<_, String>(0),
            ).ok()
        } else {
            None
        };

        // Upsert preference
        let pref_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO user_backend_preferences (id, user_id, mode, fallback_mode, active_remote_backend_target_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id) DO UPDATE SET
                mode = excluded.mode,
                fallback_mode = excluded.fallback_mode,
                active_remote_backend_target_id = excluded.active_remote_backend_target_id,
                updated_at = CURRENT_TIMESTAMP",
            params![pref_id, user_id, mode, fallback, resolved_target],
        )?;

        Ok::<_, rusqlite::Error>(resolved_target)
    }).await;

    match result {
        Ok(Ok(_)) => get_runtime_backend(State(state), headers)
            .await
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error setting backend: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── POST /runtime/backend/manual ─────────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterManualBody {
    name: Option<String>,
    #[serde(alias = "gatewayUrl")]
    gateway_url: String,
    #[serde(alias = "gatewayWsUrl")]
    gateway_ws_url: Option<String>,
    #[serde(alias = "gatewayToken")]
    gateway_token: Option<String>,
}

async fn register_manual_backend(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<RegisterManualBody>,
) -> impl IntoResponse {
    let gateway_url = body.gateway_url.trim_end_matches('/').to_string();
    if gateway_url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "gatewayUrl is required"})),
        )
            .into_response();
    }

    // Health check
    let health = match verify_backend_health(&gateway_url, body.gateway_token.as_deref()).await {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Runtime backend is not reachable: {}", e)})),
            )
                .into_response();
        }
    };

    if !health.reachable {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Runtime backend is not reachable"})),
        )
            .into_response();
    }

    let db = state.db.clone();
    let user_id = user.user_id;
    let name = body
        .name
        .unwrap_or_else(|| "Manual Runtime Backend".to_string());
    let gw_url = gateway_url.clone();
    let gw_ws = body.gateway_ws_url.clone();
    let token = body.gateway_token.clone();

    let name_for_db = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Extract host and port for SSH connection fields
        let (host, port) = parse_url_host_port(&gw_url)?;
        let ssh_conn_id = format!("manual-{}-{}", user_id, &base64_url_encode(&gw_url)[..24.min(base64_url_encode(&gw_url).len())]);

        // Upsert SSH connection
        conn.execute(
            "INSERT INTO ssh_connections (id, user_id, name, host, port, username, auth_type, status, allternit_installed, last_connected_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'manual', 'password', 'connected', 1, CURRENT_TIMESTAMP)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                host = excluded.host,
                port = excluded.port,
                status = 'connected',
                allternit_installed = 1,
                last_connected_at = CURRENT_TIMESTAMP",
            params![&ssh_conn_id, &user_id, &name_for_db, &host, port],
        )?;

        // Upsert backend target
        let target_id = uuid::Uuid::new_v4().to_string();
        let gw_ws_url = gw_ws.unwrap_or_else(|| {
            gw_url.replace("http://", "ws://").replace("https://", "wss://")
        });

        conn.execute(
            "INSERT INTO remote_backend_targets (id, user_id, ssh_connection_id, name, status, install_state, backend_url, gateway_url, gateway_ws_url, encrypted_gateway_token, last_verified_at)
             VALUES (?1, ?2, ?3, ?4, 'ready', 'installed', ?5, ?6, ?7, ?8, CURRENT_TIMESTAMP)
             ON CONFLICT(ssh_connection_id) DO UPDATE SET
                user_id = excluded.user_id,
                name = excluded.name,
                status = 'ready',
                install_state = 'installed',
                backend_url = excluded.backend_url,
                gateway_url = excluded.gateway_url,
                gateway_ws_url = excluded.gateway_ws_url,
                encrypted_gateway_token = excluded.encrypted_gateway_token,
                last_verified_at = CURRENT_TIMESTAMP",
            params![&target_id, &user_id, &ssh_conn_id, &name_for_db, &gw_url, &gw_url, &gw_ws_url, token.as_deref().map(crate::token_crypto::seal)],
        )?;

        // Set as active preference
        let pref_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO user_backend_preferences (id, user_id, mode, fallback_mode, active_remote_backend_target_id)
             VALUES (?1, ?2, 'byoc-vps', 'local', ?3)
             ON CONFLICT(user_id) DO UPDATE SET
                mode = 'byoc-vps',
                fallback_mode = 'local',
                active_remote_backend_target_id = ?3,
                updated_at = CURRENT_TIMESTAMP",
            params![pref_id, user_id, target_id],
        )?;

        Ok::<_, rusqlite::Error>(target_id)
    }).await;

    match result {
        Ok(Ok(target_id)) => Json(json!({
            "success": true,
            "message": "Runtime backend registered",
            "backend_target": {
                "id": target_id,
                "name": name,
                "status": "ready",
                "install_state": "installed",
                "backend_url": gateway_url,
            },
        }))
        .into_response(),
        Ok(Err(e)) => {
            warn!("DB error registering manual backend: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn verify_backend_health(
    url: &str,
    token: Option<&str>,
) -> Result<HealthResult, reqwest::Error> {
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{}/health", url.trim_end_matches('/')));
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let resp = req
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;
    Ok(HealthResult {
        reachable: resp.status().is_success(),
    })
}

struct HealthResult {
    reachable: bool,
}

fn base64_url_encode(input: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(input)
}

fn parse_url_host_port(url: &str) -> Result<(String, i64), rusqlite::Error> {
    // Simple URL parser: scheme://host:port/path
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let (host_port, _) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));

    let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
        let h = h.trim_start_matches('[').trim_end_matches(']');
        match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port as i64),
            Err(_) => (host_port.to_string(), 443i64),
        }
    } else {
        let default_port = if url.starts_with("https://") { 443 } else { 80 };
        (host_port.to_string(), default_port as i64)
    };

    Ok((host, port))
}
