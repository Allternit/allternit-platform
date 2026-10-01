//! Embeddable live-computer widget (Phase 5): viewer "status" endpoint,
//! embed-token issuance, and the public noVNC viewer page.
//!
//! Design notes:
//!
//! - **Viewer status** (`GET /api/v1/computers/:id/status`) is owner-scoped
//!   exactly like `get_computer` and returns the token-less relative ws paths
//!   (callers mint short-lived tokens separately via `ws-token`).
//! - **Standalone `control_state` analog.** Bot desktops have a
//!   `BotDesktopControlState` (bot_controls / human_controls / …) because a
//!   bot and a human can fight over one desktop. Standalone computers have no
//!   bot arbitration, so the honest static value is [`STANDALONE_CONTROL_STATE`]:
//!   the owner always controls their own computer; admitted viewers may still
//!   be read-only at the token level.
//! - **Embed tokens** (`POST /api/v1/computers/:id/embed-token`) reuse the
//!   HMAC `sign_computer_token` machinery with purpose `"embed"`, are bound to
//!   the single computer, and are ALWAYS signed read-only. One token
//!   authorizes both the viewer page and the VNC connection: the VNC ws
//!   handler accepts purpose `"embed"` and forces read-only (see
//!   `resolve_vnc_access` in `computer_ws`). The invariant is: embed TOKENS
//!   are always read-only — the ws proxy suppresses client->TCP frames.
//! - **Viewer page** (`GET /embed/computers/:id?token=...`) is mounted on the
//!   PUBLIC router: it verifies the token (HMAC, computer binding, expiry,
//!   purpose) with NO Clerk auth, and serves a minimal self-contained noVNC
//!   page. It accepts two token purposes (see [`resolve_embed_page_access`]):
//!   `"embed"` (always view-only) and `"vnc"` minted by
//!   `POST /api/v1/computers/:id/ws-token`. The page is INTERACTIVE only for a
//!   `"vnc"` token whose claim is `read_only: false` — minting one is the
//!   existing approval-gated path in `computer_ws::issue_ws_token` (control
//!   lease holder, or an ACI confirmation grant), which stays the ONLY way to
//!   get write — and only while no one else holds the control lease (the same
//!   `read_only_unless_controller` downgrade the ws proxy applies). The page
//!   hands the same token to the VNC ws, whose proxy enforces read-only on
//!   the server regardless of what the page sets client-side. The bootstrap
//!   script is an external same-origin file (`/embed/viewer.js`) so the page
//!   works under `script-src 'self'` (an inline module is blocked by that
//!   CSP); per-page config travels in HTML-escaped `data-*` attributes. The Content-Security-Policy `frame-ancestors` source list
//!   comes from `ALLTERNIT_EMBED_FRAME_ANCESTORS` (default `"*"` — v1
//!   self-host allows embedding from any origin; tighten the env var to a
//!   space-separated origin list to restrict who may iframe the viewer).
//! - **noVNC assets** are vendored (NO CDN): `cmd/allternit-api/package.json`
//!   pins `@novnc/novnc` and `scripts/vendor-novnc.mjs` copies `core/` + `vendor/`
//!   into `cmd/allternit-api/assets/novnc/`, served by this module.
//!   LIMITATION: static files are read from disk at a path resolved relative
//!   to `env!("CARGO_MANIFEST_DIR")` — this works in the repo / self-host
//!   layout but is NOT embedded into the binary; a deployment that moves the
//!   binary away from the source tree must ship `assets/novnc/` or the viewer
//!   page loads with 404 assets.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::computer_routes::{fetch_computer, ComputerStatus};
use crate::AppState;

/// TTL for embed tokens: 15 minutes — long enough for an iframe viewer to
/// linger on a page, short enough that a leaked token (it travels in URLs)
/// self-expires quickly. Interactive ws tokens live 5 minutes.
const EMBED_TOKEN_TTL_SECONDS: u64 = 900;

/// Standalone computers have no bot-style control-state arbitration; the
/// owner always controls their own computer. This is the documented static
/// analog of `BotDesktopControlState` for the viewer status surface.
pub const STANDALONE_CONTROL_STATE: &str = "owner_controls";

/// Env var controlling the viewer page CSP `frame-ancestors`. Default `"*"`
/// (any origin may iframe the viewer) — right for v1 self-host; set to a
/// space-separated allowlist of origins to lock embedding down.
pub const FRAME_ANCESTORS_ENV: &str = "ALLTERNIT_EMBED_FRAME_ANCESTORS";

/// noVNC static assets root, resolved relative to this crate's manifest dir.
/// See the module doc for the disk-layout limitation.
fn novnc_assets_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("novnc")
}

// ── Routers ──────────────────────────────────────────────────────────────────

/// Authenticated REST routes, merged under `/api/v1` next to
/// `computer_ws::computer_api_router`.
pub fn api_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/computers/:id/status", get(computer_status))
        .route("/computers/:id/embed-token", post(issue_embed_token))
}

/// Public routes (no Clerk auth; the embed token IS the credential).
/// Mounted on the public router BEFORE the platform `/` fallback.
pub fn public_router() -> Router<Arc<AppState>> {
    if !novnc_assets_dir().is_dir() {
        warn!(
            path = %novnc_assets_dir().display(),
            "vendored noVNC assets not found; the embed viewer page will load with 404 assets \
             (run scripts/vendor-novnc.mjs or ship assets/novnc/ next to the binary)"
        );
    }
    Router::new()
        .route("/embed/computers/:id", get(embed_viewer_page))
        .route("/embed/viewer.js", get(serve_viewer_script))
        .route("/embed/assets/*path", get(serve_novnc_asset))
}

// ── A. Viewer status ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ComputerWsPaths {
    /// Relative ws paths; callers mint short-lived tokens separately
    /// (`POST /api/v1/computers/:id/ws-token`, `?token=` query param).
    pub pty: String,
    pub events: String,
    pub vnc: String,
}

#[derive(Debug, Serialize)]
pub struct ComputerStatusResponse {
    pub id: String,
    pub status: ComputerStatus,
    pub os: Option<String>,
    pub name: String,
    /// Static analog of `BotDesktopControlState` for standalone computers
    /// (see [`STANDALONE_CONTROL_STATE`]).
    pub control_state: &'static str,
    pub ws: ComputerWsPaths,
}

fn status_response(computer: &crate::computer_routes::ComputerResponse) -> ComputerStatusResponse {
    let id = &computer.id;
    ComputerStatusResponse {
        id: computer.id.clone(),
        status: computer.status,
        os: computer.os.clone(),
        name: computer.name.clone(),
        control_state: STANDALONE_CONTROL_STATE,
        ws: ComputerWsPaths {
            pty: format!("/ws/computers/{id}/pty"),
            events: format!("/ws/computers/{id}/events"),
            vnc: format!("/ws/computers/{id}/vnc"),
        },
    }
}

/// GET /api/v1/computers/:id/status — owner-scoped viewer state: status, os,
/// name, the ws paths (pty/events/vnc), and the standalone `control_state`
/// analog. Read-only; no ACI gate.
async fn computer_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(computer)) => {
            if computer.status == ComputerStatus::Running && crate::computer_ws::reconcile_gone_computer(&state, &computer).await {
                return (
                    StatusCode::GONE,
                    Json(serde_json::json!({ "error": "computer_gone", "message": crate::computer_ws::COMPUTER_GONE_MESSAGE })),
                )
                    .into_response();
            }
            Json(status_response(&computer)).into_response()
        }
        Ok(None) => crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => response,
    }
}

// ── B. Embed token ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct EmbedTokenResponse {
    pub token: String,
    pub expires_in: u64,
    /// Relative ws path (token appended by the caller as `?token=`).
    pub vnc_ws_url: String,
    /// Relative viewer page URL.
    pub viewer_url: String,
}

fn embed_token_response(
    token: String,
    computer_id: &str,
) -> EmbedTokenResponse {
    EmbedTokenResponse {
        vnc_ws_url: format!("/ws/computers/{computer_id}/vnc?token={token}"),
        viewer_url: format!("/embed/computers/{computer_id}?token={token}"),
        token,
        expires_in: EMBED_TOKEN_TTL_SECONDS,
    }
}

/// POST /api/v1/computers/:id/embed-token — mint a short-lived, single-
/// computer-bound, READ-ONLY embed token. One token authorizes both the
/// viewer page and the VNC ws connection (purpose "embed" is forced read-only
/// in the ws handler). Owner-scoped; audited.
pub async fn issue_embed_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    let secret = match crate::bot_desktop_stream::desktop_ws_secret(&state) {
        Some(s) => s,
        None => {
            return crate::computer_routes::error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop ws not configured",
            )
        }
    };
    let sandbox_id = computer
        .native_id
        .clone()
        .unwrap_or_else(|| computer.id.clone());
    // read_only: true — the embed invariant. The ws handler additionally
    // forces read-only for any purpose-"embed" token, so even a forged
    // read_only=false claim on an embed token stays view-only.
    let token = crate::bot_desktop_stream::sign_computer_token(
        &secret,
        &computer.id,
        &sandbox_id,
        &user.user_id,
        EMBED_TOKEN_TTL_SECONDS,
        "embed",
        true,
    );
    crate::computer_audit::log_computer_access(
        &state.db,
        &computer.id,
        &user.user_id,
        crate::computer_audit::KIND_EMBED_TOKEN,
        "embed viewer token issued (read-only)",
    );
    Json(embed_token_response(token, &computer.id)).into_response()
}

// ── C. Public viewer page ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EmbedPageQuery {
    token: String,
}

/// Verify an embed credential for computer `id`: HMAC signature, computer
/// binding, expiry, and purpose "embed". This is the entire auth for the
/// public viewer surface — no Clerk session exists for an iframe visitor.
pub fn verify_embed_token(
    secret: &str,
    token: &str,
    computer_id: &str,
) -> Result<(), crate::bot_desktop_stream::DesktopTokenError> {
    crate::bot_desktop_stream::verify_computer_token(secret, token, computer_id, "embed")
        .map(|_| ())
}

/// Decide what the public viewer page may do with `token` for computer `id`.
/// Returns the verified claims and whether the TOKEN grants write:
///
/// - purpose `"embed"` — view-only, always (even a forged `read_only: false`
///   claim; mirrors `resolve_vnc_access` in `computer_ws`).
/// - purpose `"vnc"` — write iff the claim is `read_only: false`. Such tokens
///   are only minted by `issue_ws_token` behind the control lease / ACI
///   confirmation gate, so this adds no new way to obtain write.
/// - anything else (`pty`, `events`, bot tokens) — rejected.
///
/// The caller still applies the control-lease downgrade before rendering.
pub fn resolve_embed_page_access(
    secret: &str,
    token: &str,
    computer_id: &str,
) -> Result<
    (crate::bot_desktop_stream::DesktopTokenClaims, bool),
    crate::bot_desktop_stream::DesktopTokenError,
> {
    use crate::bot_desktop_stream::DesktopTokenError;
    let claims = crate::bot_desktop_stream::verify_desktop_token(secret, token)?;
    if claims.computer_id.as_deref() != Some(computer_id) {
        return Err(DesktopTokenError::ComputerMismatch);
    }
    let writable = match claims.purpose.as_deref() {
        Some("embed") => false,
        Some("vnc") => !claims.read_only,
        _ => return Err(DesktopTokenError::PurposeMismatch),
    };
    Ok((claims, writable))
}

fn frame_ancestors() -> String {
    std::env::var(FRAME_ANCESTORS_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "*".to_string())
}

fn viewer_csp() -> String {
    format!(
        "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; connect-src 'self' ws: wss:; frame-ancestors {}",
        frame_ancestors()
    )
}

/// Escape text for a double-quoted HTML attribute value.
fn html_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn viewer_html(computer_id: &str, token: &str, writable: bool) -> String {
    let id = html_attr(computer_id);
    let token = html_attr(token);
    let writable = if writable { "true" } else { "false" };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Allternit Computer Viewer</title>
<style>
  html, body {{ margin: 0; padding: 0; height: 100%; background: #0b0b0c; overflow: hidden; }}
  #screen {{ width: 100vw; height: 100vh; }}
  #status {{
    position: fixed; left: 12px; bottom: 10px; padding: 4px 10px; border-radius: 6px;
    font: 12px/1.4 system-ui, sans-serif; color: #e5e5e5; background: rgba(0,0,0,.55);
    pointer-events: none;
  }}
</style>
</head>
<body>
<div id="screen" data-computer-id="{id}" data-token="{token}" data-writable="{writable}"></div>
<div id="status" role="status">connecting…</div>
<script type="module" src="/embed/viewer.js"></script>
</body>
</html>
"#
    )
}

/// Bootstrap for the viewer page, served same-origin from `/embed/viewer.js`
/// (the page CSP is `script-src 'self'`, which blocks inline scripts). Reads
/// its config from `#screen`'s `data-*` attributes. `data-writable` only
/// toggles noVNC's client-side `viewOnly`; the ws proxy enforces read-only
/// server-side from the token claims, so flipping it in devtools cannot
/// grant input. The asset root is `assets/novnc`, so `rfb.js` is served at
/// `/embed/assets/core/rfb.js` (its `../vendor/` imports resolve beside it).
const VIEWER_JS: &str = r#"import RFB from '/embed/assets/core/rfb.js';

const screen = document.getElementById('screen');
const statusEl = document.getElementById('status');
const computerId = screen.dataset.computerId;
const token = screen.dataset.token;
const writable = screen.dataset.writable === 'true';
const mode = writable ? 'interactive' : 'view-only';

const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
const wsUrl = `${proto}//${location.host}/ws/computers/${encodeURIComponent(computerId)}/vnc?token=${encodeURIComponent(token)}`;

const rfb = new RFB(screen, wsUrl, {
  scaleViewport: true,
  resizeSession: false,
});
rfb.viewOnly = !writable;
rfb.focusOnClick = writable;

rfb.addEventListener('connect', () => {
  statusEl.textContent = `connected (${mode})`;
  if (writable) rfb.focus();
});
rfb.addEventListener('disconnect', (e) => {
  statusEl.textContent = e.detail.clean ? 'disconnected' : 'connection lost';
});
rfb.addEventListener('securityfailure', () => { statusEl.textContent = 'security handshake failed'; });
rfb.addEventListener('credentialsrequired', () => { statusEl.textContent = 'credentials required'; });
"#;

/// GET /embed/viewer.js — the viewer page's same-origin bootstrap script.
async fn serve_viewer_script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        VIEWER_JS,
    )
        .into_response()
}

/// GET /embed/computers/:id?token= — public, token-gated, self-contained
/// noVNC viewer: view-only for embed tokens and read-only vnc tokens,
/// interactive for a write-granting vnc token (see
/// [`resolve_embed_page_access`]) unless someone else holds the control
/// lease. Served with a restrictive CSP whose `frame-ancestors` comes from
/// `ALLTERNIT_EMBED_FRAME_ANCESTORS`.
async fn embed_viewer_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<EmbedPageQuery>,
) -> Response {
    let secret = match crate::bot_desktop_stream::desktop_ws_secret(&state) {
        Some(s) => s,
        None => {
            return crate::computer_routes::error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop ws not configured",
            )
        }
    };
    let (claims, token_writable) = match resolve_embed_page_access(&secret, &query.token, &id) {
        Ok(access) => access,
        Err(e) => {
            warn!(error = %e, computer_id = %id, "invalid embed viewer token");
            return crate::computer_routes::error_response(StatusCode::FORBIDDEN, "invalid token");
        }
    };
    // Same one-controller downgrade the ws proxy applies: when someone else
    // holds the control lease, a write-granting token still only watches.
    let writable = token_writable
        && !crate::computer_ws::read_only_unless_controller(&state, &id, &claims.user_id, false)
            .await;
    let html = viewer_html(&id, &query.token, writable);
    let csp = viewer_csp();
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, csp.as_str()),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        html,
    )
        .into_response()
}

// ── D. Vendored noVNC static assets ──────────────────────────────────────────

/// Content type for a vendored asset path, by extension.
fn asset_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("html") => "text/html; charset=utf-8",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Resolve a requested asset path under the vendored noVNC root, rejecting
/// path traversal (`..` components, absolute paths, backslashes).
fn resolve_asset_path(root: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let mut out = root.to_path_buf();
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::Normal(c) => out.push(c),
            _ => return None,
        }
    }
    Some(out)
}

/// GET /embed/assets/*path — serve vendored noVNC files from disk
/// (`cmd/allternit-api/assets/novnc`, resolved via `CARGO_MANIFEST_DIR`).
/// See the module doc for the disk-layout limitation.
async fn serve_novnc_asset(Path(path): Path<String>) -> Response {
    let root = novnc_assets_dir();
    let Some(file_path) = resolve_asset_path(&root, &path) else {
        return crate::computer_routes::error_response(StatusCode::BAD_REQUEST, "invalid asset path");
    };
    let Ok(bytes) = tokio::fs::read(&file_path).await else {
        return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "asset not found");
    };
    (
        [
            (
                header::CONTENT_TYPE,
                asset_content_type(&path).to_string(),
            ),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        bytes,
    )
        .into_response()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-embed-secret-for-unit-tests";

    fn computer() -> crate::computer_routes::ComputerResponse {
        crate::computer_routes::ComputerResponse {
            id: "computer-1".into(),
            kind: crate::computer_routes::ComputerKind::CloudDesktop,
            provider: "incus".into(),
            status: ComputerStatus::Running,
            owner_type: "user".into(),
            owner_id: "user-1".into(),
            bot_id: None,
            session_id: None,
            name: "Builder Box".into(),
            os: Some("ubuntu-24.04".into()),
            cpu_cores: Some(4),
            memory_mb: Some(8192),
            disk_mb: Some(40960),
            region: None,
            host: None,
            native_id: Some("sandbox-1".into()),
            template_id: None,
            billing_source: "credits".into(),
            created_at: "2026-09-09T00:00:00Z".into(),
            updated_at: "2026-09-09T00:00:00Z".into(),
            idle_timeout_secs: None,
            last_activity_at: None,
            group_id: None,
            role: "user".into(),
        }
    }

    #[test]
    fn status_response_shape_serializes() {
        let value = serde_json::to_value(status_response(&computer())).unwrap();
        assert_eq!(value["id"], "computer-1");
        assert_eq!(value["status"], "running");
        assert_eq!(value["os"], "ubuntu-24.04");
        assert_eq!(value["name"], "Builder Box");
        // Standalone analog of BotDesktopControlState, documented constant.
        assert_eq!(value["control_state"], STANDALONE_CONTROL_STATE);
        // Token-less relative ws paths.
        assert_eq!(value["ws"]["pty"], "/ws/computers/computer-1/pty");
        assert_eq!(value["ws"]["events"], "/ws/computers/computer-1/events");
        assert_eq!(value["ws"]["vnc"], "/ws/computers/computer-1/vnc");
        assert!(value.get("token").is_none());
    }

    #[tokio::test]
    async fn status_handler_returns_viewer_state_for_owner() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', 'user-1', 'Builder Box', 'ubuntu-24.04', 'sandbox-1', 'credits')",
                [],
            )
            .unwrap();
        let user = AuthUser {
            user_id: "user-1".into(),
            organization_id: Some("org-1".into()),
            tenant_id: None,
            email: None,
            name: None,
            avatar_url: None,
            organization_role: None,
            organization_slug: None,
        };
        let response = computer_status(
            State(state.clone()),
            Extension(user),
            Path("computer-1".to_string()),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], "computer-1");
        assert_eq!(value["status"], "running");
        assert_eq!(value["ws"]["vnc"], "/ws/computers/computer-1/vnc");
        assert_eq!(value["control_state"], "owner_controls");
    }

    #[tokio::test]
    async fn status_handler_removes_a_computer_whose_machine_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn allternit_driver_interface::ExecutionDriver> =
            Arc::new(crate::computer_ws::tests::StubDriver(None));
        let state = crate::test_helpers::app_state_with_driver(temp.path(), Some(driver)).await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', 'user-1', 'Account computer', 'linux', 'gone-1', 'credits')",
                [],
            )
            .unwrap();
        let user = AuthUser {
            user_id: "user-1".into(),
            organization_id: Some("org-1".into()),
            tenant_id: None,
            email: None,
            name: None,
            avatar_url: None,
            organization_role: None,
            organization_slug: None,
        };
        let response = computer_status(State(state.clone()), Extension(user), Path("computer-1".to_string()))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::GONE);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"], "computer_gone");
        let status: String = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT status FROM computers WHERE id = 'computer-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "deleted");
    }

    #[tokio::test]
    async fn status_handler_is_owner_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', 'user-1', 'Builder Box', 'credits')",
                [],
            )
            .unwrap();
        let other_user = AuthUser {
            user_id: "user-2".into(),
            organization_id: Some("org-2".into()),
            tenant_id: None,
            email: None,
            name: None,
            avatar_url: None,
            organization_role: None,
            organization_slug: None,
        };
        let response = computer_status(
            State(state.clone()),
            Extension(other_user),
            Path("computer-1".to_string()),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn embed_token_verify_accepts_valid_token() {
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-1", "sandbox-1", "user-1", 60, "embed", true,
        );
        assert!(verify_embed_token(TEST_SECRET, &token, "computer-1").is_ok());
    }

    #[test]
    fn embed_token_verify_rejects_expired() {
        // Sign a token with exp=1 directly (past) — deterministic, no sleep.
        use crate::bot_desktop_stream::{b64_encode, hmac_sign};
        let header = b64_encode(br#"{"alg":"HS256","typ":"DT"}"#);
        let payload = b64_encode(
            br#"{"bot_id":"","computer_id":"computer-1","purpose":"embed","read_only":true,"sandbox_id":"s","user_id":"u","exp":1}"#,
        );
        let signing_input = format!("{}.{}", header, payload);
        let signature = hmac_sign(TEST_SECRET, &signing_input);
        let expired = format!("{}.{}", signing_input, signature);
        let err = verify_embed_token(TEST_SECRET, &expired, "computer-1").unwrap_err();
        assert!(matches!(
            err,
            crate::bot_desktop_stream::DesktopTokenError::Expired
        ));
    }

    #[test]
    fn embed_token_verify_rejects_wrong_computer() {
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-a", "sandbox-a", "user-1", 60, "embed", true,
        );
        // A token minted for computer-a must not authorize computer-b.
        let err = verify_embed_token(TEST_SECRET, &token, "computer-b").unwrap_err();
        assert!(matches!(
            err,
            crate::bot_desktop_stream::DesktopTokenError::ComputerMismatch
        ));
    }

    #[test]
    fn embed_token_verify_rejects_other_purposes() {
        // Interactive pty/events tokens must not work on the embed surface.
        for purpose in ["pty", "events", "vnc"] {
            let token = crate::bot_desktop_stream::sign_computer_token(
                TEST_SECRET, "computer-1", "s", "user-1", 60, purpose, false,
            );
            let err = verify_embed_token(TEST_SECRET, &token, "computer-1").unwrap_err();
            assert!(
                matches!(
                    err,
                    crate::bot_desktop_stream::DesktopTokenError::PurposeMismatch
                ),
                "purpose {purpose} must be rejected on the embed surface"
            );
        }
    }

    #[test]
    fn embed_token_response_shape() {
        let response = embed_token_response("tok-123".to_string(), "computer-1");
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["token"], "tok-123");
        assert_eq!(value["expires_in"], EMBED_TOKEN_TTL_SECONDS);
        assert_eq!(
            value["vnc_ws_url"],
            "/ws/computers/computer-1/vnc?token=tok-123"
        );
        assert_eq!(
            value["viewer_url"],
            "/embed/computers/computer-1?token=tok-123"
        );
    }

    #[test]
    fn asset_content_type_mapping() {
        assert_eq!(asset_content_type("core/rfb.js"), "text/javascript; charset=utf-8");
        assert_eq!(asset_content_type("app/styles.css"), "text/css; charset=utf-8");
        assert_eq!(asset_content_type("core/util/browser.js.map"), "application/json; charset=utf-8");
        assert_eq!(asset_content_type("vendor/pako/README.md"), "application/octet-stream");
    }

    #[test]
    fn asset_path_traversal_rejected() {
        let root = std::path::Path::new("/tmp/novnc-root");
        assert!(resolve_asset_path(root, "../secret").is_none());
        assert!(resolve_asset_path(root, "core/../../secret").is_none());
        assert!(resolve_asset_path(root, "/etc/passwd").is_none());
        assert!(resolve_asset_path(root, "").is_none());
        assert!(resolve_asset_path(root, "a\\b").is_none());
        assert_eq!(
            resolve_asset_path(root, "core/rfb.js").unwrap(),
            std::path::Path::new("/tmp/novnc-root/core/rfb.js")
        );
    }

    #[test]
    fn viewer_page_is_view_only_by_default_and_escapes_attrs() {
        let html = viewer_html("computer-1", "tok-with-\"quotes\"<x>", false);
        // External same-origin bootstrap: an inline module violates the CSP.
        assert!(html.contains(r#"<script type="module" src="/embed/viewer.js"></script>"#));
        assert!(!html.contains("import RFB"));
        assert!(html.contains(r#"data-writable="false""#));
        assert!(html.contains(r#"data-computer-id="computer-1""#));
        // The token is HTML-attribute escaped.
        assert!(html.contains("tok-with-&quot;quotes&quot;&lt;x&gt;"));
        assert!(!html.contains("tok-with-\"quotes\""));
    }

    #[test]
    fn viewer_page_writable_flag() {
        assert!(viewer_html("computer-1", "tok", true).contains(r#"data-writable="true""#));
    }

    #[test]
    fn viewer_script_drives_view_only_from_writable_flag() {
        // The asset root is assets/novnc, so rfb.js is at /embed/assets/core/.
        assert!(VIEWER_JS.contains("import RFB from '/embed/assets/core/rfb.js'"));
        assert!(
            resolve_asset_path(&novnc_assets_dir(), "core/rfb.js").unwrap().is_file(),
            "vendored rfb.js must exist where the viewer imports it"
        );
        assert!(VIEWER_JS.contains("screen.dataset.writable === 'true'"));
        assert!(VIEWER_JS.contains("rfb.viewOnly = !writable"));
        assert!(VIEWER_JS.contains("scaleViewport: true"));
    }

    #[test]
    fn page_access_embed_token_is_never_writable() {
        for read_only in [true, false] {
            let token = crate::bot_desktop_stream::sign_computer_token(
                TEST_SECRET, "computer-1", "s", "user-1", 60, "embed", read_only,
            );
            let (_, writable) = resolve_embed_page_access(TEST_SECRET, &token, "computer-1").unwrap();
            assert!(!writable, "embed tokens are view-only (claim read_only={read_only})");
        }
    }

    #[test]
    fn page_access_vnc_token_writable_only_when_it_grants_write() {
        let rw = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-1", "s", "user-1", 60, "vnc", false,
        );
        assert!(resolve_embed_page_access(TEST_SECRET, &rw, "computer-1").unwrap().1);
        let ro = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-1", "s", "user-1", 60, "vnc", true,
        );
        assert!(!resolve_embed_page_access(TEST_SECRET, &ro, "computer-1").unwrap().1);
    }

    #[test]
    fn page_access_rejects_other_purposes_wrong_computer_and_forgery() {
        use crate::bot_desktop_stream::DesktopTokenError;
        for purpose in ["pty", "events"] {
            let token = crate::bot_desktop_stream::sign_computer_token(
                TEST_SECRET, "computer-1", "s", "user-1", 60, purpose, false,
            );
            assert!(matches!(
                resolve_embed_page_access(TEST_SECRET, &token, "computer-1"),
                Err(DesktopTokenError::PurposeMismatch)
            ));
        }
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-a", "s", "user-1", 60, "vnc", false,
        );
        assert!(matches!(
            resolve_embed_page_access(TEST_SECRET, &token, "computer-b"),
            Err(DesktopTokenError::ComputerMismatch)
        ));
        // A write token signed with another secret is not honored.
        let forged = crate::bot_desktop_stream::sign_computer_token(
            "attacker-secret", "computer-1", "s", "user-1", 60, "vnc", false,
        );
        assert!(matches!(
            resolve_embed_page_access(TEST_SECRET, &forged, "computer-1"),
            Err(DesktopTokenError::Signature)
        ));
    }

    /// Same value computer_ws's handler tests put in the process env, so
    /// parallel tests setting the shared secret never race to different values.
    const HANDLER_SECRET: &str = "test-ws-secret-for-unit-tests";

    async fn page_state() -> Arc<AppState> {
        std::env::set_var("ALLTERNIT_DESKTOP_WS_SECRET", HANDLER_SECRET);
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        std::mem::forget(temp);
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', 'user-1', 'Builder Box', 'ubuntu-24.04', 'sandbox-1', 'credits')",
                [],
            )
            .unwrap();
        state
    }

    async fn page(state: &Arc<AppState>, token: String) -> (StatusCode, axum::http::HeaderMap, String) {
        let response = embed_viewer_page(
            State(state.clone()),
            Path("computer-1".to_string()),
            Query(EmbedPageQuery { token }),
        )
        .await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, headers, String::from_utf8(body.to_vec()).unwrap())
    }

    fn handler_token(purpose: &str, read_only: bool) -> String {
        crate::bot_desktop_stream::sign_computer_token(
            HANDLER_SECRET, "computer-1", "sandbox-1", "user-1", 60, purpose, read_only,
        )
    }

    #[tokio::test]
    async fn embed_page_writable_only_with_write_granting_token() {
        let state = page_state().await;
        // Embed tokens (even with a read_only=false claim): view-only, and
        // the security headers are intact.
        for ro in [true, false] {
            let (status, headers, html) = page(&state, handler_token("embed", ro)).await;
            assert_eq!(status, StatusCode::OK);
            assert!(html.contains(r#"data-writable="false""#));
            let csp = headers.get(header::CONTENT_SECURITY_POLICY).unwrap().to_str().unwrap();
            assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'"));
            assert_eq!(headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
            assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
            assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        }
        // Read-only vnc token: view-only.
        let (status, _, html) = page(&state, handler_token("vnc", true)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"data-writable="false""#));
        // Write-granting vnc token: interactive.
        let (status, _, html) = page(&state, handler_token("vnc", false)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"data-writable="true""#));
        // Interactive-shell tokens never open the viewer.
        let (status, _, _) = page(&state, handler_token("pty", false)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn embed_page_write_token_downgrades_when_someone_else_controls() {
        let state = page_state().await;
        let other = crate::computer_control_lease::Holder {
            kind: crate::computer_control_lease::HolderKind::User,
            id: "user-2".into(),
            label: None,
            device_id: None,
        };
        let conn = state.db.connect().unwrap();
        crate::computer_control_lease::take(&conn, "computer-1", &other, chrono::Utc::now().timestamp())
            .unwrap();
        let (status, _, html) = page(&state, handler_token("vnc", false)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"data-writable="false""#));
    }

    #[tokio::test]
    async fn viewer_script_is_served_as_javascript() {
        let response = serve_viewer_script().await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(response.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
    }

    #[test]
    fn viewer_csp_includes_frame_ancestors_default() {
        // Default frame-ancestors is "*" (v1 self-host) unless the env var is set.
        if std::env::var(FRAME_ANCESTORS_ENV).is_err() {
            let csp = viewer_csp();
            assert!(csp.contains("frame-ancestors *"), "csp: {csp}");
            assert!(csp.contains("default-src 'none'"));
            assert!(csp.contains("script-src 'self'"));
            assert!(csp.contains("connect-src 'self' ws: wss:"));
        }
    }
}
