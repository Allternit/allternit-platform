//! `GET /bot-hub/:id/summary` - the four tiles on a bot's page (Soul, Memory,
//! Skills, Usage) plus its model. Every value with no real source is null so
//! clients hide it. The data lives in allternit-api's SQLite, the same plane
//! as `/cowork/memory`; cloud-api is not involved.

use axum::{
    extract::{Extension, Path, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::agent_workspace_paths::workspace_dir_for;
use crate::agent_workspace_routes::{authorize_agent, unify_legacy_files};
use crate::auth::AuthUser;
use crate::AppState;

pub fn bot_hub_router() -> Router<Arc<AppState>> {
    Router::new().route("/bot-hub/:id/summary", get(bot_summary))
}

async fn bot_summary(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(resp) = authorize_agent(&state.db, user.user_id.clone(), id.clone()).await {
        return resp;
    }
    let db = state.db.clone();
    let user_id = user.user_id;
    let out = tokio::task::spawn_blocking(move || {
        let conn = db.connect().ok();
        summary(conn.as_ref(), &user_id, &id)
    })
    .await
    .unwrap_or_else(|_| summary(None, "", ""));
    Json(out).into_response()
}

/// Skills installed on this machine: folders under the skills root with a
/// SKILL.md. Names are the folder names.
pub(crate) fn installed_skill_names() -> Vec<String> {
    let Some(root) = crate::skill_catalog_install::skills_root() else {
        return Vec::new();
    };
    let mut names: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("SKILL.md").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The bot's allow-list when it has one (`agents.allowed_skills` JSON array);
/// None means every installed skill is on.
pub(crate) fn allowed_skills(conn: &Connection, user_id: &str, bot: &str) -> Option<Vec<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT allowed_skills FROM agents WHERE id = ?1 AND user_id = ?2",
            params![bot, user_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten();
    match serde_json::from_str::<Value>(raw.as_deref()?).ok()? {
        Value::Array(a) => Some(a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()),
        _ => None,
    }
}

pub(crate) fn summary(conn: Option<&Connection>, user_id: &str, bot: &str) -> Value {
    let dir = workspace_dir_for(bot);
    unify_legacy_files(&dir);
    let soul = std::fs::metadata(dir.join("SOUL.md")).ok();
    let soul_updated = soul
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
    let soul_bytes = soul.map(|m| m.len()).unwrap_or(0);

    let installed = installed_skill_names();
    let skills = match conn {
        Some(c) => {
            let on = match allowed_skills(c, user_id, bot) {
                Some(list) => list.iter().filter(|s| installed.contains(s)).count(),
                None => installed.len(),
            };
            json!({"on": on, "total": installed.len()})
        }
        None => json!({"on": 0, "total": installed.len()}),
    };

    let model = conn.and_then(|c| {
        c.query_row(
            "SELECT model FROM agents WHERE id = ?1 AND user_id = ?2",
            params![bot, user_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
    });

    let count = |sql: &str| -> Option<i64> {
        conn?.query_row(sql, params![user_id, bot], |r| r.get(0)).ok()
    };
    let memory = conn.and_then(|c| memory_count(c, user_id, bot));
    // A "session" is a bot thread active in the window; an "API call" is one
    // recorded agent run (one chat-bridge round trip). Spend is not recorded
    // anywhere today, so it stays null.
    let sessions = count(
        "SELECT COUNT(*) FROM bot_threads WHERE user_id = ?1 AND bot_id = ?2 AND last_activity_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-30 days')",
    );
    let calls = count(
        "SELECT COUNT(*) FROM agent_runs WHERE user_id = ?1 AND agent_id = ?2 AND created_at >= datetime('now','-30 days')",
    );

    json!({
        "model": model,
        "soul": {"updated_at": soul_updated, "bytes": soul_bytes},
        "memory": {"count": memory},
        "skills": skills,
        "usage": {"sessions_30d": sessions, "api_calls_30d": calls, "spend_usd_30d": Value::Null},
    })
}

/// Entries this bot owns (same scoping as `GET /cowork/memory?bot=`).
fn memory_count(conn: &Connection, user_id: &str, bot: &str) -> Option<i64> {
    let principal = crate::cowork_routes::bot_memory_principal(conn, user_id, bot).ok()??;
    let rows = allternit_cowork_runtime::sqlite_store::search_memory_entries(
        conn,
        user_id,
        Some(&principal),
        None,
        100_000,
        0,
    )
    .ok()?;
    Some(
        rows.iter()
            .filter(|m| m["owner_principal"].as_str() == Some(principal.as_str()))
            .count() as i64,
    )
}
