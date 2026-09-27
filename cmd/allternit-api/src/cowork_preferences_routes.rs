//! Cowork Preference API routes
//!
//! Per-user Cowork preferences (`/cowork-preferences`): folders Cowork
//! agents may read/write, and free-form instructions applied to every
//! Cowork session. Mirrors agent_preferences_routes.rs's storage pattern.

use axum::{
    extract::{Extension, Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

const MAX_TRUSTED_FOLDERS: usize = 50;
const MAX_INSTRUCTIONS_LEN: usize = 20_000;

pub fn cowork_preferences_router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/cowork-preferences",
        get(get_cowork_preferences).put(set_cowork_preferences),
    )
}

#[derive(Serialize)]
struct CoworkPreferencesPayload {
    trusted_folders: Vec<String>,
    global_instructions: String,
    cloud_continuation: bool,
    continuation_api_url: Option<String>,
    require_trusted_devices: bool,
    files_location: Option<String>,
    preferred_browser: String,
    open_links_in_app: bool,
    allowed_sites: Vec<String>,
    updated_at: String,
}

/// Every stored Cowork preference for a user (defaults when no row exists).
#[derive(Clone)]
pub struct CoworkPrefs {
    pub trusted_folders: Vec<String>,
    pub global_instructions: String,
    pub cloud_continuation: bool,
    pub continuation_api_url: Option<String>,
    pub require_trusted_devices: bool,
    pub files_location: Option<String>,
    pub preferred_browser: String,
    pub open_links_in_app: bool,
    pub allowed_sites: Vec<String>,
    pub updated_at: String,
}

pub fn load_prefs(conn: &rusqlite::Connection, user_id: &str) -> CoworkPrefs {
    conn.query_row(
        "SELECT trusted_folders, global_instructions, cloud_continuation, continuation_api_url,
                require_trusted_devices, files_location, preferred_browser, open_links_in_app,
                allowed_sites, updated_at
         FROM user_cowork_preferences WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(CoworkPrefs {
                trusted_folders: parse_string_list(&row.get::<_, String>(0)?),
                global_instructions: row.get(1)?,
                cloud_continuation: row.get::<_, i64>(2)? != 0,
                continuation_api_url: row.get(3)?,
                require_trusted_devices: row.get::<_, i64>(4)? != 0,
                files_location: row.get(5)?,
                preferred_browser: row.get(6)?,
                open_links_in_app: row.get::<_, i64>(7)? != 0,
                allowed_sites: parse_string_list(&row.get::<_, String>(8)?),
                updated_at: row.get(9)?,
            })
        },
    )
    .unwrap_or_else(|_| CoworkPrefs {
        trusted_folders: Vec::new(),
        global_instructions: String::new(),
        cloud_continuation: false,
        continuation_api_url: None,
        require_trusted_devices: false,
        files_location: None,
        preferred_browser: "built-in".to_string(),
        open_links_in_app: true,
        allowed_sites: Vec::new(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    })
}

impl From<CoworkPrefs> for CoworkPreferencesPayload {
    fn from(p: CoworkPrefs) -> Self {
        CoworkPreferencesPayload {
            trusted_folders: p.trusted_folders,
            global_instructions: p.global_instructions,
            cloud_continuation: p.cloud_continuation,
            continuation_api_url: p.continuation_api_url,
            require_trusted_devices: p.require_trusted_devices,
            files_location: p.files_location,
            preferred_browser: p.preferred_browser,
            open_links_in_app: p.open_links_in_app,
            allowed_sites: p.allowed_sites,
            updated_at: p.updated_at,
        }
    }
}

pub fn cloud_continuation_enabled(conn: &rusqlite::Connection, user_id: &str) -> bool {
    conn.query_row(
        "SELECT cloud_continuation FROM user_cowork_preferences WHERE user_id = ?1",
        params![user_id],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .map(|v| v != 0)
    .unwrap_or(false)
}

fn parse_string_list(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

// ─── GET /cowork-preferences ───────────────────────────────────────────────

async fn get_cowork_preferences(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        Ok::<_, rusqlite::Error>(load_prefs(&conn, &user_id))
    })
    .await;

    match result {
        Ok(Ok(prefs)) => Json(CoworkPreferencesPayload::from(prefs)).into_response(),
        Ok(Err(e)) => {
            warn!("DB error reading cowork preferences: {}", e);
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

// ─── PUT /cowork-preferences ────────────────────────────────────────────────

#[derive(Deserialize)]
struct SetCoworkPreferencesBody {
    trusted_folders: Option<Vec<String>>,
    global_instructions: Option<String>,
    cloud_continuation: Option<bool>,
    continuation_api_url: Option<String>,
    require_trusted_devices: Option<bool>,
    /// Absolute folder; an empty string clears it.
    files_location: Option<String>,
    preferred_browser: Option<String>,
    open_links_in_app: Option<bool>,
    allowed_sites: Option<Vec<String>>,
}

const MAX_ALLOWED_SITES: usize = 200;
pub const PREFERRED_BROWSERS: [&str; 2] = ["built-in", "chrome"];

fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/') || path.get(1..3).is_some_and(|s| s == ":\\")
}

fn validate_trusted_folders(folders: &[String]) -> Result<Vec<String>, String> {
    if folders.len() > MAX_TRUSTED_FOLDERS {
        return Err(format!("A maximum of {MAX_TRUSTED_FOLDERS} trusted folders is supported."));
    }
    let mut cleaned: Vec<String> = Vec::with_capacity(folders.len());
    for folder in folders {
        let trimmed = folder.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Cowork agents run locally with the working directory passed straight
        // to the process spawner (see cowork.runtime.ts) — require an absolute
        // path so a relative entry can't silently resolve outside what the
        // user intended to trust.
        if !is_absolute_path(trimmed) {
            return Err(format!("\"{trimmed}\" is not an absolute path."));
        }
        if !cleaned.iter().any(|existing: &String| existing == trimmed) {
            cleaned.push(trimmed.to_string());
        }
    }
    Ok(cleaned)
}

/// Reduce each entry to a bare lowercase hostname ("https://Example.com/x" →
/// "example.com"); rejects entries with no usable host.
pub fn normalize_allowed_sites(sites: &[String]) -> Result<Vec<String>, String> {
    if sites.len() > MAX_ALLOWED_SITES {
        return Err(format!("A maximum of {MAX_ALLOWED_SITES} allowed sites is supported."));
    }
    let mut out: Vec<String> = Vec::new();
    for raw in sites {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let without_scheme = trimmed.split_once("://").map(|(_, r)| r).unwrap_or(trimmed);
        let host = without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .split('@')
            .next_back()
            .unwrap_or("")
            .trim_start_matches("*.")
            .to_ascii_lowercase();
        let host = host.split(':').next().unwrap_or("").to_string();
        let valid = !host.is_empty()
            && host.contains('.') || host == "localhost";
        if !valid || host.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
            return Err(format!("\"{trimmed}\" is not a site (use a hostname like example.com)."));
        }
        if !out.contains(&host) {
            out.push(host);
        }
    }
    Ok(out)
}

fn bad_request(error: &str, message: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error, "message": message }))).into_response()
}

async fn set_cowork_preferences(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<SetCoworkPreferencesBody>,
) -> impl IntoResponse {
    let trusted_folders = match body.trusted_folders.as_deref().map(validate_trusted_folders) {
        Some(Ok(cleaned)) => Some(cleaned),
        Some(Err(message)) => return bad_request("invalid_trusted_folders", message),
        None => None,
    };
    let allowed_sites = match body.allowed_sites.as_deref().map(normalize_allowed_sites) {
        Some(Ok(cleaned)) => Some(cleaned),
        Some(Err(message)) => return bad_request("invalid_allowed_sites", message),
        None => None,
    };
    if let Some(ref browser) = body.preferred_browser {
        if !PREFERRED_BROWSERS.contains(&browser.as_str()) {
            return bad_request("invalid_preferred_browser", format!("Preferred browser must be one of {PREFERRED_BROWSERS:?}."));
        }
    }
    let files_location: Option<Option<String>> = match body.files_location.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(path) if is_absolute_path(path) => Some(Some(path.trim_end_matches('/').to_string())),
        Some(path) => return bad_request("invalid_files_location", format!("\"{path}\" is not an absolute path.")),
        None => None,
    };
    if let Some(ref instructions) = body.global_instructions {
        if instructions.len() > MAX_INSTRUCTIONS_LEN {
            return bad_request(
                "instructions_too_long",
                format!("Global instructions must be under {MAX_INSTRUCTIONS_LEN} characters."),
            );
        }
    }

    let db = state.db.clone();
    let user_id = user.user_id;
    let env_url = state.config.continuation_api_url();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Merge with the existing row so omitted fields keep their values.
        let before = load_prefs(&conn, &user_id);
        let mut next = before.clone();
        if let Some(v) = trusted_folders { next.trusted_folders = v; }
        if let Some(v) = body.global_instructions { next.global_instructions = v; }
        if let Some(v) = body.cloud_continuation { next.cloud_continuation = v; }
        if let Some(v) = body.continuation_api_url {
            let t = v.trim().trim_end_matches('/').to_string();
            next.continuation_api_url = if t.is_empty() { None } else { Some(t) };
        }
        if let Some(v) = body.require_trusted_devices { next.require_trusted_devices = v; }
        if let Some(v) = files_location { next.files_location = v; }
        if let Some(v) = body.preferred_browser { next.preferred_browser = v; }
        if let Some(v) = body.open_links_in_app { next.open_links_in_app = v; }
        if let Some(v) = allowed_sites { next.allowed_sites = v; }

        if next.cloud_continuation && !before.cloud_continuation {
            let has_url = env_url.is_some() || next.continuation_api_url.is_some();
            let has_cloud_relay = std::env::var("ALLTERNIT_CLOUD_API_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .is_some();
            if !has_url && !has_cloud_relay {
                return Err(rusqlite::Error::InvalidParameterName(
                    "cloud continuation needs a hosted/paired always-on node (api.allternit.com continuation/ensure) or ALLTERNIT_CONTINUATION_API_URL".into(),
                ));
            }
        }

        conn.execute(
            "INSERT INTO user_cowork_preferences (user_id, trusted_folders, global_instructions, cloud_continuation,
                continuation_api_url, require_trusted_devices, files_location, preferred_browser, open_links_in_app, allowed_sites)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(user_id) DO UPDATE SET
                trusted_folders = excluded.trusted_folders,
                global_instructions = excluded.global_instructions,
                cloud_continuation = excluded.cloud_continuation,
                continuation_api_url = excluded.continuation_api_url,
                require_trusted_devices = excluded.require_trusted_devices,
                files_location = excluded.files_location,
                preferred_browser = excluded.preferred_browser,
                open_links_in_app = excluded.open_links_in_app,
                allowed_sites = excluded.allowed_sites,
                updated_at = CURRENT_TIMESTAMP",
            params![
                user_id,
                serde_json::to_string(&next.trusted_folders).unwrap_or_else(|_| "[]".into()),
                next.global_instructions,
                next.cloud_continuation as i64,
                next.continuation_api_url,
                next.require_trusted_devices as i64,
                next.files_location,
                next.preferred_browser,
                next.open_links_in_app as i64,
                serde_json::to_string(&next.allowed_sites).unwrap_or_else(|_| "[]".into()),
            ],
        )?;
        let saved = load_prefs(&conn, &user_id);
        Ok::<_, rusqlite::Error>((before, saved))
    })
    .await;

    match result {
        Ok(Ok((before, saved))) => {
            if before.preferred_browser != saved.preferred_browser || before.allowed_sites != saved.allowed_sites {
                let (browser, old_sites, new_sites) =
                    (saved.preferred_browser.clone(), before.allowed_sites.clone(), saved.allowed_sites.clone());
                tokio::spawn(async move {
                    if let Err(e) = sync_gizzi_browser_config(&browser, &old_sites, &new_sites).await {
                        warn!("cowork prefs: gizzi browser config sync failed: {e}");
                    }
                });
            }
            Json(CoworkPreferencesPayload::from(saved)).into_response()
        }
        Ok(Err(e)) => {
            let message = e.to_string();
            if message.contains("cloud continuation needs") {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({ "error": "continuation_unconfigured", "message": message })),
                )
                    .into_response();
            }
            warn!("DB error setting cowork preferences: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": message}))).into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal error"}))).into_response()
        }
    }
}

// ─── gizzi sync ─────────────────────────────────────────────────────────────

/// The browser tool's default adapter for each preferred browser.
fn preferred_adapter(browser: &str) -> &'static str {
    match browser {
        // The Allternit extension in the user's Chrome.
        "chrome" => "extension-tab",
        // The app's own browser, driven over CDP.
        _ => "cdp",
    }
}

/// gizzi permission patterns for a host (URL targets of the browser tool).
pub fn site_patterns(host: &str) -> [String; 2] {
    [format!("*://{host}/*"), format!("*://*.{host}/*")]
}

/// The gizzi global-config patch for these preferences: the browser tool's
/// default adapter, and browser-permission rules — allowed sites "allow", and
/// sites just removed back to "ask" (a deep merge can't delete keys).
pub fn gizzi_browser_patch(browser: &str, old_sites: &[String], new_sites: &[String]) -> serde_json::Value {
    let mut rules = serde_json::Map::new();
    for host in old_sites.iter().filter(|h| !new_sites.contains(h)) {
        for p in site_patterns(host) {
            rules.insert(p, json!("ask"));
        }
    }
    for host in new_sites {
        for p in site_patterns(host) {
            rules.insert(p, json!("allow"));
        }
    }
    let mut patch = json!({ "browser": { "preferred_adapter": preferred_adapter(browser) } });
    if !rules.is_empty() {
        patch["permission"] = json!({ "browser": rules });
    }
    patch
}

async fn sync_gizzi_browser_config(browser: &str, old_sites: &[String], new_sites: &[String]) -> Result<(), String> {
    let url = format!("{}/v1/config/global", crate::agent_session_routes::gizzi_base());
    let client = crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new());
    let res = client
        .patch(url)
        .json(&gizzi_browser_patch(browser, old_sites, new_sites))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("gizzi returned {}", res.status()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_sites_reduce_to_hosts() {
        let got = normalize_allowed_sites(&[
            "https://Example.com/path?q=1".into(),
            "docs.rs".into(),
            "*.github.com".into(),
            "http://user@localhost:3000".into(),
            "example.com".into(),
            "  ".into(),
        ])
        .unwrap();
        assert_eq!(got, vec!["example.com", "docs.rs", "github.com", "localhost"]);
    }

    #[test]
    fn allowed_sites_reject_non_hosts() {
        assert!(normalize_allowed_sites(&["not a site".into()]).is_err());
        assert!(normalize_allowed_sites(&["nodot".into()]).is_err());
    }

    #[test]
    fn gizzi_patch_allows_new_and_resets_removed_sites() {
        let patch = gizzi_browser_patch("chrome", &["old.com".into(), "keep.com".into()], &["keep.com".into(), "new.com".into()]);
        assert_eq!(patch["browser"]["preferred_adapter"], "extension-tab");
        let rules = &patch["permission"]["browser"];
        assert_eq!(rules["*://old.com/*"], "ask");
        assert_eq!(rules["*://*.old.com/*"], "ask");
        assert_eq!(rules["*://keep.com/*"], "allow");
        assert_eq!(rules["*://new.com/*"], "allow");
    }

    #[test]
    fn gizzi_patch_built_in_uses_cdp_and_omits_empty_rules() {
        let patch = gizzi_browser_patch("built-in", &[], &[]);
        assert_eq!(patch["browser"]["preferred_adapter"], "cdp");
        assert!(patch.get("permission").is_none());
    }
}
