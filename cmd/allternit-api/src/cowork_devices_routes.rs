//! Cowork devices (Settings → Cowork → "Cowork local devices").
//!
//! Every client that talks to this API for a user identifies itself with a
//! stable `X-Allternit-Device-Id` (Desktop: an id kept in the app's userData;
//! browsers/PWA: an id kept in local storage). Desktop apps register on launch
//! and — when the request carries the desktop access token — are trusted
//! automatically. Other clients show up untrusted until the user trusts them.
//!
//! With `require_trusted_devices` on (user_cowork_preferences),
//! [`trusted_device_middleware`] refuses `/remote-control/*` from any device
//! that isn't trusted, and records unknown ones as pending so the user can
//! trust them from Settings.

use axum::{
    body::Body,
    extract::{Extension, Json, Path, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

pub const DEVICE_ID_HEADER: &str = "x-allternit-device-id";
pub const DEVICE_NAME_HEADER: &str = "x-allternit-device-name";
pub const DEVICE_PLATFORM_HEADER: &str = "x-allternit-device-platform";
/// Set by the desktop's cloud relay on every request it forwards from another
/// device. Relayed requests carry the desktop access token (the relay brokers
/// auth), so without this marker every phone would pass as "this computer".
pub const RELAYED_HEADER: &str = "x-allternit-relayed";

const MAX_NAME_LEN: usize = 80;
const KINDS: [&str; 3] = ["desktop", "browser", "mobile"];

pub fn cowork_devices_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cowork/devices", get(list_devices))
        .route("/cowork/devices/register", post(register_device))
        .route("/cowork/devices/:id", patch(update_device).delete(remove_device))
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CoworkDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub kind: String,
    pub trusted: bool,
    pub added_at: String,
    pub last_seen_at: String,
    /// The device making this request.
    pub current: bool,
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(MAX_NAME_LEN).collect())
}

fn valid_device_id(id: &str) -> bool {
    (8..=128).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn list_for_user(conn: &rusqlite::Connection, user_id: &str, current: Option<&str>) -> rusqlite::Result<Vec<CoworkDevice>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, platform, kind, trusted, added_at, last_seen_at
         FROM cowork_devices WHERE user_id = ?1 ORDER BY last_seen_at DESC",
    )?;
    let rows = stmt.query_map(params![user_id], |r| {
        let id: String = r.get(0)?;
        Ok(CoworkDevice {
            current: current == Some(id.as_str()),
            id,
            name: r.get(1)?,
            platform: r.get(2)?,
            kind: r.get(3)?,
            trusted: r.get::<_, i64>(4)? != 0,
            added_at: r.get(5)?,
            last_seen_at: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// Insert or refresh a device. A user-chosen name is kept on refresh; trust
/// is only ever raised here (auto-trust), never lowered.
pub fn upsert_device(
    conn: &rusqlite::Connection,
    user_id: &str,
    id: &str,
    name: &str,
    platform: &str,
    kind: &str,
    trusted: bool,
) -> rusqlite::Result<()> {
    let ts = now();
    conn.execute(
        "INSERT INTO cowork_devices (id, user_id, name, platform, kind, trusted, added_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(user_id, id) DO UPDATE SET
            platform = excluded.platform,
            kind = excluded.kind,
            trusted = MAX(cowork_devices.trusted, excluded.trusted),
            last_seen_at = excluded.last_seen_at",
        params![id, user_id, name, platform, kind, trusted as i64, ts],
    )?;
    Ok(())
}

fn err(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

// ─── GET /cowork/devices ─────────────────────────────────────────────────────

async fn list_devices(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let current = header(&headers, DEVICE_ID_HEADER);
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        list_for_user(&conn, &user.user_id, current.as_deref())
    })
    .await;
    match result {
        Ok(Ok(devices)) => Json(json!({ "devices": devices })).into_response(),
        Ok(Err(e)) => {
            warn!("cowork devices: list failed: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())
        }
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

// ─── POST /cowork/devices/register ───────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterBody {
    id: String,
    name: String,
    platform: String,
    kind: String,
}

async fn register_device(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<RegisterBody>,
) -> Response {
    let id = body.id.trim().to_string();
    if !valid_device_id(&id) {
        return err(StatusCode::BAD_REQUEST, "invalid_device_id", "Device id must be 8–128 letters, digits, - or _.");
    }
    if !KINDS.contains(&body.kind.as_str()) {
        return err(StatusCode::BAD_REQUEST, "invalid_kind", "kind must be desktop, browser or mobile.");
    }
    let name: String = body.name.trim().chars().take(MAX_NAME_LEN).collect();
    let name = if name.is_empty() { "Unnamed device".to_string() } else { name };
    let platform: String = body.platform.trim().chars().take(32).collect();
    // Only a request carrying the desktop access token (the local app itself)
    // is trusted on registration; a browser can't self-trust by claiming
    // kind = "desktop", and neither can a device reaching us over the relay
    // (which carries the desktop token on its behalf).
    let auto_trust = body.kind == "desktop"
        && headers.get(RELAYED_HEADER).is_none()
        && crate::auth::verify_desktop_access_token(&headers, &state.config);

    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        upsert_device(&conn, &user.user_id, &id, &name, &platform, &body.kind, auto_trust)?;
        list_for_user(&conn, &user.user_id, Some(&id))
    })
    .await;
    match result {
        Ok(Ok(devices)) => Json(json!({ "devices": devices })).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

// ─── PATCH /cowork/devices/:id ───────────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateBody {
    name: Option<String>,
    trusted: Option<bool>,
}

async fn update_device(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let current = header(&headers, DEVICE_ID_HEADER);
    if body.trusted == Some(false) && current.as_deref() == Some(id.as_str()) {
        return err(StatusCode::CONFLICT, "current_device", "You can't untrust the device you're using.");
    }
    let name = body.name.map(|n| n.trim().chars().take(MAX_NAME_LEN).collect::<String>());
    if name.as_deref() == Some("") {
        return err(StatusCode::BAD_REQUEST, "invalid_name", "Name can't be empty.");
    }
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let n = conn.execute(
            "UPDATE cowork_devices SET name = COALESCE(?3, name), trusted = COALESCE(?4, trusted)
             WHERE user_id = ?1 AND id = ?2",
            params![user.user_id, id, name, body.trusted.map(|t| t as i64)],
        )?;
        Ok::<_, rusqlite::Error>((n, list_for_user(&conn, &user.user_id, current.as_deref())?))
    })
    .await;
    match result {
        Ok(Ok((0, _))) => err(StatusCode::NOT_FOUND, "not_found", "No such device."),
        Ok(Ok((_, devices))) => Json(json!({ "devices": devices })).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

// ─── DELETE /cowork/devices/:id ──────────────────────────────────────────────

async fn remove_device(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let current = header(&headers, DEVICE_ID_HEADER);
    if current.as_deref() == Some(id.as_str()) {
        return err(StatusCode::CONFLICT, "current_device", "You can't remove the device you're using.");
    }
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let n = conn.execute("DELETE FROM cowork_devices WHERE user_id = ?1 AND id = ?2", params![user.user_id, id])?;
        Ok::<_, rusqlite::Error>((n, list_for_user(&conn, &user.user_id, current.as_deref())?))
    })
    .await;
    match result {
        Ok(Ok((0, _))) => err(StatusCode::NOT_FOUND, "not_found", "No such device."),
        Ok(Ok((_, devices))) => Json(json!({ "devices": devices })).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

// ─── Enforcement ─────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum DeviceCheck {
    Allowed,
    /// No device id on the request.
    Unidentified,
    /// Known or newly recorded, but not trusted.
    Untrusted,
}

/// Decide whether this request's device may use remote control. Records an
/// unknown device as pending (untrusted) so it shows up in Settings.
pub fn check_device(conn: &rusqlite::Connection, user_id: &str, headers: &HeaderMap, desktop_token: bool) -> rusqlite::Result<DeviceCheck> {
    if !crate::cowork_preferences_routes::load_prefs(conn, user_id).require_trusted_devices {
        return Ok(DeviceCheck::Allowed);
    }
    // The local app's own calls (desktop access token) are this computer —
    // unless the desktop relay forwarded them from another device.
    if desktop_token && headers.get(RELAYED_HEADER).is_none() {
        return Ok(DeviceCheck::Allowed);
    }
    let Some(id) = header(headers, DEVICE_ID_HEADER).filter(|id| valid_device_id(id)) else {
        return Ok(DeviceCheck::Unidentified);
    };
    let trusted: Option<i64> = conn
        .query_row(
            "SELECT trusted FROM cowork_devices WHERE user_id = ?1 AND id = ?2",
            params![user_id, id],
            |r| r.get(0),
        )
        .optional()?;
    match trusted {
        Some(t) if t != 0 => {
            conn.execute(
                "UPDATE cowork_devices SET last_seen_at = ?3 WHERE user_id = ?1 AND id = ?2",
                params![user_id, id, now()],
            )?;
            Ok(DeviceCheck::Allowed)
        }
        Some(_) => Ok(DeviceCheck::Untrusted),
        None => {
            let name = header(headers, DEVICE_NAME_HEADER).unwrap_or_else(|| "New device".to_string());
            let platform = header(headers, DEVICE_PLATFORM_HEADER).unwrap_or_else(|| "Web".to_string());
            upsert_device(conn, user_id, &id, &name, &platform, "browser", false)?;
            Ok(DeviceCheck::Untrusted)
        }
    }
}

/// Route layer for `/remote-control/*`: with "Require trusted devices" on,
/// only trusted devices (or the local app itself) get through.
pub async fn trusted_device_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    let Some(user) = req.extensions().get::<AuthUser>().cloned().or_else(|| crate::auth::get_user(&headers)) else {
        return next.run(req).await;
    };
    let desktop_token = crate::auth::verify_desktop_access_token(&headers, &state.config);
    let db = state.db.clone();
    let verdict = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        check_device(&conn, &user.user_id, &headers, desktop_token)
    })
    .await;
    match verdict {
        Ok(Ok(DeviceCheck::Allowed)) => next.run(req).await,
        Ok(Ok(DeviceCheck::Unidentified)) => err(
            StatusCode::FORBIDDEN,
            "device_unidentified",
            "Require trusted devices is on and this device didn't identify itself.",
        ),
        Ok(Ok(DeviceCheck::Untrusted)) => err(
            StatusCode::FORBIDDEN,
            "device_not_trusted",
            "This device isn't trusted yet. Trust it in Settings → Cowork on your computer.",
        ),
        Ok(Err(e)) => {
            warn!("cowork devices: trust check failed: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", "Couldn't check this device.")
        }
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE user_cowork_preferences (
                user_id TEXT PRIMARY KEY, trusted_folders TEXT NOT NULL DEFAULT '[]',
                global_instructions TEXT NOT NULL DEFAULT '', cloud_continuation INTEGER NOT NULL DEFAULT 0,
                continuation_api_url TEXT, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
        )
        .unwrap();
        conn.execute_batch(include_str!("../migrations/V188__cowork_devices_settings.sql")).unwrap();
        conn
    }

    fn headers(id: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(id) = id {
            h.insert(DEVICE_ID_HEADER, id.parse().unwrap());
            h.insert(DEVICE_NAME_HEADER, "Joe's iPhone".parse().unwrap());
        }
        h
    }

    fn require(conn: &rusqlite::Connection, on: bool) {
        conn.execute(
            "INSERT INTO user_cowork_preferences (user_id, require_trusted_devices) VALUES ('u', ?1)
             ON CONFLICT(user_id) DO UPDATE SET require_trusted_devices = excluded.require_trusted_devices",
            params![on as i64],
        )
        .unwrap();
    }

    #[test]
    fn off_by_default_lets_everyone_through() {
        let conn = db();
        assert_eq!(check_device(&conn, "u", &headers(None), false).unwrap(), DeviceCheck::Allowed);
    }

    #[test]
    fn unknown_device_is_recorded_pending_and_refused_until_trusted() {
        let conn = db();
        require(&conn, true);
        let h = headers(Some("phone-device-0001"));
        assert_eq!(check_device(&conn, "u", &h, false).unwrap(), DeviceCheck::Untrusted);
        let listed = list_for_user(&conn, "u", None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Joe's iPhone");
        assert!(!listed[0].trusted);

        conn.execute("UPDATE cowork_devices SET trusted = 1 WHERE id = 'phone-device-0001'", []).unwrap();
        assert_eq!(check_device(&conn, "u", &h, false).unwrap(), DeviceCheck::Allowed);
    }

    #[test]
    fn missing_id_is_refused_but_the_local_app_is_allowed() {
        let conn = db();
        require(&conn, true);
        assert_eq!(check_device(&conn, "u", &headers(None), false).unwrap(), DeviceCheck::Unidentified);
        assert_eq!(check_device(&conn, "u", &headers(None), true).unwrap(), DeviceCheck::Allowed);
    }

    #[test]
    fn relayed_requests_do_not_pass_as_this_computer() {
        let conn = db();
        require(&conn, true);
        let mut h = headers(Some("phone-device-0002"));
        h.insert(RELAYED_HEADER, "1".parse().unwrap());
        // The relay attaches the desktop token, but the device is still checked.
        assert_eq!(check_device(&conn, "u", &h, true).unwrap(), DeviceCheck::Untrusted);
        let mut anonymous = headers(None);
        anonymous.insert(RELAYED_HEADER, "1".parse().unwrap());
        assert_eq!(check_device(&conn, "u", &anonymous, true).unwrap(), DeviceCheck::Unidentified);
    }

    #[test]
    fn upsert_keeps_a_chosen_name_and_never_lowers_trust() {
        let conn = db();
        upsert_device(&conn, "u", "mac-device-0001", "MacBook", "macOS", "desktop", true).unwrap();
        conn.execute("UPDATE cowork_devices SET name = 'Studio Mac'", []).unwrap();
        upsert_device(&conn, "u", "mac-device-0001", "MacBook", "macOS", "desktop", false).unwrap();
        let d = &list_for_user(&conn, "u", Some("mac-device-0001")).unwrap()[0];
        assert_eq!(d.name, "Studio Mac");
        assert!(d.trusted);
        assert!(d.current);
    }
}
