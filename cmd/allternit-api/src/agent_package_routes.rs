//! Bot copy between computers: `GET /api/v1/agents/:id/package` and
//! `PUT /api/v1/agents/:id/package`.
//!
//! Bots live per computer (the `agents` table on each allternit-api). A cloud
//! routine or Always on failover needs the bot on the cloud computer too, so
//! the web reads the package from the home computer and puts it on the cloud
//! computer with the same id. The package carries the agent row (persona,
//! brain, settings, skills) and the workspace text files (Soul, instructions,
//! memory). It never carries credentials, inbound addresses, phone numbers or
//! channel bindings: those stay on the home computer.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::agent_workspace_paths::workspace_dir_for;
use crate::agent_workspace_routes::{unify_legacy_files, ROOT_FILES};
use crate::auth::AuthUser;
use crate::AppState;

pub fn agent_package_router() -> Router<Arc<AppState>> {
    Router::new().route("/agents/:id/package", get(get_package).put(put_package))
}

/// Workspace files copied besides `ROOT_FILES` and `.allternit/bot/*`.
const EXTRA_FILES: [&str; 5] = ["MEMORY.md", "memory/MEMORY.md", "GIZZI.md", ".claude/CLAUDE.md", "SYSTEM_LAW.md"];
const BOT_DIR: &str = ".allternit/bot";
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_FILES: usize = 64;

/// Agent columns that travel. Not `user_id` (the receiver's own), not
/// `identity_key`/`workspace_id`/`parent_agent_id`/`is_primary` (local to
/// each computer), not status/run times.
const COPY_COLUMNS: [&str; 24] = [
    "name", "description", "type", "model", "provider", "capabilities", "system_prompt", "tools",
    "max_iterations", "temperature", "config", "avatar", "trust_tier", "harness_config",
    "enabled_modes", "character_json", "allowed_skills", "allowed_tools", "category", "tags",
    "data_classification", "write_scope", "mode", "owns",
];
/// Columns stored as JSON text.
const JSON_COLUMNS: [&str; 11] = [
    "capabilities", "tools", "config", "harness_config", "enabled_modes", "character_json",
    "allowed_skills", "allowed_tools", "tags", "owns", "delegates",
];

/// Config keys that hold credentials or per-computer bindings (inbound email,
/// numbers, chat channels, signed-in sessions). Matched case-insensitively
/// as substrings.
const PRIVATE_CONFIG_MARKERS: [&str; 14] = [
    "secret", "token", "password", "credential", "apikey", "api_key", "phone", "email",
    "channel", "binding", "messaging", "webhook", "vmoperator", "session",
];

/// The config with credentials and per-computer bindings removed (top level).
pub(crate) fn strip_private_config(config: Value) -> Value {
    match config {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(k, _)| {
                    let k = k.to_ascii_lowercase();
                    !PRIVATE_CONFIG_MARKERS.iter().any(|m| k.contains(m))
                })
                .collect::<Map<String, Value>>(),
        ),
        other => other,
    }
}

fn is_copyable_path(path: &str) -> bool {
    if ROOT_FILES.contains(&path) || EXTRA_FILES.contains(&path) {
        return true;
    }
    match path.strip_prefix(".allternit/bot/") {
        Some(name) => {
            !name.is_empty()
                && !name.contains(['/', '\\', '\0'])
                && !name.starts_with('.')
                && !name.ends_with(".pre-unify")
                && (name.ends_with(".md") || name.ends_with(".json") || name.ends_with(".txt"))
        }
        None => false,
    }
}

/// The bot's workspace text files, by relative path.
pub(crate) fn pack_files(dir: &std::path::Path) -> BTreeMap<String, String> {
    unify_legacy_files(dir);
    let mut names: Vec<String> = ROOT_FILES.iter().chain(EXTRA_FILES.iter()).map(|s| s.to_string()).collect();
    if let Ok(entries) = std::fs::read_dir(dir.join(BOT_DIR)) {
        for e in entries.flatten() {
            if let Some(n) = e.file_name().to_str() {
                names.push(format!("{BOT_DIR}/{n}"));
            }
        }
    }
    let mut out = BTreeMap::new();
    for rel in names {
        if out.len() >= MAX_FILES || !is_copyable_path(&rel) {
            continue;
        }
        let p = dir.join(&rel);
        let small = std::fs::metadata(&p).map(|m| m.is_file() && m.len() <= MAX_FILE_BYTES).unwrap_or(false);
        if !small {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            out.insert(rel, text);
        }
    }
    out
}

/// Write packaged files into a workspace; paths outside the allowed set are skipped.
pub(crate) fn unpack_files(dir: &std::path::Path, files: &BTreeMap<String, String>) -> std::io::Result<usize> {
    let mut written = 0;
    for (rel, text) in files.iter().take(MAX_FILES) {
        if !is_copyable_path(rel) || text.len() as u64 > MAX_FILE_BYTES {
            continue;
        }
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, text)?;
        written += 1;
    }
    Ok(written)
}

fn load_agent_fields(conn: &rusqlite::Connection, id: &str, user_id: &str) -> rusqlite::Result<Option<Map<String, Value>>> {
    let sql = format!("SELECT {} FROM agents WHERE id = ?1 AND user_id = ?2", COPY_COLUMNS.join(", "));
    conn.query_row(&sql, params![id, user_id], |row| {
        let mut m = Map::new();
        for (i, col) in COPY_COLUMNS.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i)?;
            let v = match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(n) => json!(n),
                rusqlite::types::Value::Real(f) => json!(f),
                rusqlite::types::Value::Text(s) if JSON_COLUMNS.contains(col) => {
                    serde_json::from_str(&s).unwrap_or(Value::Null)
                }
                rusqlite::types::Value::Text(s) => Value::String(s),
                rusqlite::types::Value::Blob(_) => Value::Null,
            };
            m.insert((*col).to_string(), v);
        }
        if let Some(c) = m.remove("config") {
            m.insert("config".into(), strip_private_config(c));
        }
        Ok(m)
    })
    .optional()
}

fn to_sql(col: &str, v: Option<&Value>) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        None | Some(Value::Null) => S::Null,
        Some(v) if JSON_COLUMNS.contains(&col) => S::Text(v.to_string()),
        Some(Value::String(s)) => S::Text(s.clone()),
        Some(Value::Bool(b)) => S::Integer(*b as i64),
        Some(Value::Number(n)) => n.as_i64().map(S::Integer).or_else(|| n.as_f64().map(S::Real)).unwrap_or(S::Null),
        Some(other) => S::Text(other.to_string()),
    }
}

enum Upsert {
    Saved,
    OwnedByOther,
}

/// Create or update the agent row with this id for this user. The receiver's
/// own private config keys (its channels, sign-ins) are kept on update.
fn upsert_agent(conn: &rusqlite::Connection, id: &str, user_id: &str, agent: &Map<String, Value>) -> rusqlite::Result<Upsert> {
    let existing: Option<(String, Option<String>)> = conn
        .query_row("SELECT user_id, config FROM agents WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let mut fields = agent.clone();
    let incoming_config = strip_private_config(fields.remove("config").unwrap_or(Value::Null));
    let mut config = match incoming_config {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    if let Some((owner, local_config)) = &existing {
        if owner != user_id {
            return Ok(Upsert::OwnedByOther);
        }
        // Keep what only this computer knows (its own channels and sign-ins).
        if let Some(Value::Object(local)) = local_config.as_deref().and_then(|s| serde_json::from_str(s).ok()) {
            for (k, v) in local {
                let lk = k.to_ascii_lowercase();
                if PRIVATE_CONFIG_MARKERS.iter().any(|m| lk.contains(m)) {
                    config.insert(k, v);
                }
            }
        }
    }
    fields.insert("config".into(), Value::Object(config.clone()));
    let is_bot = config.get("isBot").and_then(Value::as_bool).unwrap_or(true);
    let name = fields.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(rusqlite::Error::InvalidParameterName("name".into()));
    }

    let mut values: Vec<rusqlite::types::Value> = COPY_COLUMNS.iter().map(|c| to_sql(c, fields.get(*c))).collect();
    // NOT NULL columns fall back to the table's usual defaults.
    for (i, c) in COPY_COLUMNS.iter().enumerate() {
        if matches!(values[i], rusqlite::types::Value::Null) {
            values[i] = match *c {
                "type" => "worker".to_string().into(),
                "model" | "provider" => "".to_string().into(),
                "max_iterations" => rusqlite::types::Value::Integer(10),
                "temperature" => rusqlite::types::Value::Real(0.7),
                "trust_tier" => "standard".to_string().into(),
                "enabled_modes" => "[\"chat\"]".to_string().into(),
                "mode" => "primary".to_string().into(),
                "owns" => "[]".to_string().into(),
                _ => rusqlite::types::Value::Null,
            };
        }
    }

    if existing.is_some() {
        let sets = COPY_COLUMNS.iter().enumerate().map(|(i, c)| format!("{c} = ?{}", i + 1)).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE agents SET {sets}, is_bot = ?{}, version = version + 1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?{} AND user_id = ?{}",
            COPY_COLUMNS.len() + 1,
            COPY_COLUMNS.len() + 2,
            COPY_COLUMNS.len() + 3
        );
        values.push(rusqlite::types::Value::Integer(is_bot as i64));
        values.push(id.to_string().into());
        values.push(user_id.to_string().into());
        conn.execute(&sql, rusqlite::params_from_iter(values))?;
    } else {
        let cols = COPY_COLUMNS.join(", ");
        let marks = (1..=COPY_COLUMNS.len() + 3).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let sql = format!("INSERT INTO agents ({cols}, is_bot, id, user_id) VALUES ({marks})");
        values.push(rusqlite::types::Value::Integer(is_bot as i64));
        values.push(id.to_string().into());
        values.push(user_id.to_string().into());
        conn.execute(&sql, rusqlite::params_from_iter(values))?;
    }
    Ok(Upsert::Saved)
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

/// `GET /agents/:id/package` — the bot row and workspace text files.
async fn get_package(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let db = state.db.clone();
    let agent_id = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let agent = load_agent_fields(&conn, &agent_id, &user.user_id)?;
        Ok::<_, rusqlite::Error>(agent.map(|a| (a, pack_files(&workspace_dir_for(&agent_id)))))
    })
    .await;
    match result {
        Ok(Ok(Some((agent, files)))) => Json(json!({ "id": id, "agent": agent, "files": files, "version": 1 })).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "agent_not_found", "Bot not found on this computer."),
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "package_failed", "Could not read the bot."),
    }
}

/// `PUT /agents/:id/package` — create or update the bot with this id and write its files.
async fn put_package(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let id = id.trim().to_string();
    if id.is_empty() || id.len() > 64 || id.contains(['/', '\\', '\0']) || id.contains("..") {
        return error(StatusCode::BAD_REQUEST, "invalid_id", "Invalid bot id.");
    }
    let Some(agent) = body.get("agent").and_then(Value::as_object).cloned() else {
        return error(StatusCode::BAD_REQUEST, "invalid_package", "The package has no bot.");
    };
    let files: BTreeMap<String, String> = body
        .get("files")
        .and_then(|f| serde_json::from_value(f.clone()).ok())
        .unwrap_or_default();
    let db = state.db.clone();
    let agent_id = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        match upsert_agent(&conn, &agent_id, &user.user_id, &agent).map_err(|e| e.to_string())? {
            Upsert::OwnedByOther => Ok(None),
            Upsert::Saved => unpack_files(&workspace_dir_for(&agent_id), &files).map(Some).map_err(|e| e.to_string()),
        }
    })
    .await;
    match result {
        Ok(Ok(Some(written))) => Json(json!({ "ok": true, "id": id, "files": written })).into_response(),
        Ok(Ok(None)) => error(StatusCode::CONFLICT, "agent_id_taken", "Another account has a bot with this id on this computer."),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, "package_failed", &format!("Could not save the bot: {e}")),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "package_failed", "Could not save the bot."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_stray_paths_are_skipped() {
        let a = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("SOUL.md"), "soul").unwrap();
        std::fs::write(a.path().join("MEMORY.md"), "memory").unwrap();
        std::fs::create_dir_all(a.path().join(".allternit/bot")).unwrap();
        std::fs::write(a.path().join(".allternit/bot/GOVERNANCE.md"), "gov").unwrap();
        std::fs::write(a.path().join(".allternit/bot/key.pem"), "secret").unwrap();
        std::fs::write(a.path().join("notes.md"), "not copied").unwrap();
        let files = pack_files(a.path());
        assert_eq!(files.keys().cloned().collect::<Vec<_>>(), vec![".allternit/bot/GOVERNANCE.md", "MEMORY.md", "SOUL.md"]);

        let b = tempfile::tempdir().unwrap();
        let mut with_escape = files.clone();
        with_escape.insert("../escape.md".into(), "x".into());
        assert_eq!(unpack_files(b.path(), &with_escape).unwrap(), 3);
        assert_eq!(pack_files(b.path()), files);
        assert!(!b.path().parent().unwrap().join("escape.md").exists());
    }

    #[test]
    fn private_config_keys_never_travel() {
        let c = strip_private_config(json!({
            "botBrain": "claude", "availability": {"alwaysOn": true}, "isBot": true,
            "messagingConfig": {}, "identityChannels": [], "connectorBindings": [], "secretRefs": [],
            "emailAddress": "a@b", "phoneNumber": "+1", "token": "t"
        }));
        assert_eq!(c, json!({"botBrain": "claude", "availability": {"alwaysOn": true}, "isBot": true}));
    }
}
