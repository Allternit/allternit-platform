//! Memory scopes, promotion and curation (spec P7.3, P7.4).
//!
//! Scopes live on `cowork_memory_entries`:
//!   global  — no owner, no project, no session (every bot and chat sees it)
//!   project — `project_id`                     (the project's team sees it)
//!   bot     — `owner_principal` = the bot       (only that bot)
//!   thread  — `session_id` of a thread window   (only that thread)
//! Promotion moves an entry up (thread → bot → project → global) and records
//! `memory.promoted` on the bot's ledger. Curation tidies a bot's memory on
//! its own model — merge duplicates, drop what's stale — weekly and on demand.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::{auth::AuthUser, db::DbHandle, AppState};

pub fn memory_curation_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cowork/memory/:id/promote", post(promote))
        .route("/cowork/memory/curate", post(curate_now))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteBody {
    /// "bot" | "project" | "global"
    pub scope: String,
    pub bot_id: Option<String>,
    pub project_id: Option<String>,
}

/// Move one entry to a wider scope. Returns the scope it landed in.
pub fn promote_entry(db: &DbHandle, user_id: &str, id: &str, body: &PromoteBody) -> Result<String, (StatusCode, String)> {
    let conn = db.connect().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (owner, content): (Option<String>, String) = conn
        .query_row(
            "SELECT owner_principal, content FROM cowork_memory_entries WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "memory not found".to_string()))?;
    let (principal, project): (Option<String>, Option<String>) = match body.scope.as_str() {
        "bot" => {
            let bot = body.bot_id.as_deref().ok_or((StatusCode::BAD_REQUEST, "botId is required".to_string()))?;
            let p = crate::cowork_routes::bot_memory_principal(&conn, user_id, bot)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .ok_or((StatusCode::NOT_FOUND, "bot not found".to_string()))?;
            (Some(p), None)
        }
        "project" => (None, Some(body.project_id.clone().ok_or((StatusCode::BAD_REQUEST, "projectId is required".to_string()))?)),
        "global" => (None, None),
        _ => return Err((StatusCode::BAD_REQUEST, "scope must be bot, project or global".to_string())),
    };
    conn.execute(
        "UPDATE cowork_memory_entries SET owner_principal = ?3, project_id = ?4, session_id = NULL
         WHERE id = ?1 AND user_id = ?2",
        params![id, user_id, principal, project],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Ledger: on the bot it came from or went to.
    let bot = body.bot_id.clone().or_else(|| owner.as_deref().and_then(bot_of_principal));
    if let Some(bot) = bot {
        let event = crate::bot_event_routes::AppendEventBody {
            event_type: "memory.promoted".into(),
            actor: crate::bot_event_routes::ActorBody { r#type: "user".into(), id: user_id.into() },
            payload: json!({ "memoryId": id, "scope": body.scope, "projectId": body.project_id, "content": content }),
            occurred_at: None,
            session_id: None,
            goal_id: None,
            wih_id: None,
            task_id: None,
            run_id: None,
            idempotency_key: Some(format!("memory.promoted:{id}:{}", body.scope)),
        };
        let _ = crate::bot_event_routes::append_event(db, &bot, &event, &chrono::Utc::now().to_rfc3339());
    }
    Ok(body.scope.clone())
}

/// "a://…/bot/<id>" → "<id>".
fn bot_of_principal(p: &str) -> Option<String> {
    p.rsplit_once("/bot/").map(|(_, id)| id.to_string())
}

async fn promote(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<PromoteBody>,
) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || promote_entry(&db, &user.user_id, &id, &body)).await {
        Ok(Ok(scope)) => Json(json!({ "scope": scope })).into_response(),
        Ok(Err((code, msg))) => (code, Json(json!({ "error": msg }))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal error" }))).into_response(),
    }
}

// ─── Curation ──────────────────────────────────────────────────────────────

pub const CURATE_SYSTEM: &str = "You tidy a bot's long-term memory. You get numbered entries. \
Merge entries that say the same thing (keep every fact, number and name exactly), and drop entries that a newer entry makes wrong or that are only chatter. \
Reply with ONE JSON object and nothing else: {\"merged\":[{\"content\":\"<merged entry>\",\"from\":[<numbers>]}],\"drop\":[<numbers>]}. \
Entries you don't mention stay as they are. When in doubt, keep.";

#[derive(Debug, Default, PartialEq)]
pub struct CurationPlan {
    pub merged: Vec<(String, Vec<usize>)>,
    pub drop: Vec<usize>,
}

pub fn parse_curation(raw: &str, n: usize) -> Option<CurationPlan> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let v: Value = serde_json::from_str(&raw[start..=end]).ok()?;
    let idx = |x: &Value| x.as_u64().map(|i| i as usize).filter(|i| *i >= 1 && *i <= n);
    let mut used = std::collections::HashSet::new();
    let mut plan = CurationPlan::default();
    for m in v.get("merged").and_then(Value::as_array).cloned().unwrap_or_default() {
        let content = m.get("content").and_then(Value::as_str).map(str::trim).unwrap_or("");
        let from: Vec<usize> = m.get("from").and_then(Value::as_array).map(|a| a.iter().filter_map(idx).collect()).unwrap_or_default();
        if content.is_empty() || from.len() < 2 || from.iter().any(|i| used.contains(i)) {
            continue;
        }
        used.extend(from.iter().copied());
        plan.merged.push((content.to_string(), from));
    }
    for d in v.get("drop").and_then(Value::as_array).cloned().unwrap_or_default() {
        if let Some(i) = idx(&d) {
            if used.insert(i) {
                plan.drop.push(i);
            }
        }
    }
    Some(plan)
}

/// Curate one bot's memory now. Returns (merged, dropped).
pub async fn curate_bot(db: &DbHandle, user_id: &str, bot_id: &str) -> Result<(usize, usize), String> {
    let (principal, entries, model) = {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let principal = crate::cowork_routes::bot_memory_principal(&conn, user_id, bot_id)
            .map_err(|e| e.to_string())?
            .ok_or("bot not found")?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content FROM cowork_memory_entries WHERE user_id = ?1 AND owner_principal = ?2
                 ORDER BY created_at ASC LIMIT 200",
            )
            .map_err(|e| e.to_string())?;
        let entries: Vec<(String, String)> = stmt
            .query_map(params![user_id, principal], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        let model = conn
            .query_row("SELECT provider, model FROM agents WHERE id = ?1", params![bot_id], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .ok()
            .and_then(|(p, m)| p.zip(m))
            .filter(|(p, m)| !p.is_empty() && !m.is_empty());
        (principal, entries, model)
    };
    if entries.len() < 2 {
        return Ok((0, 0));
    }
    let prompt = entries
        .iter()
        .enumerate()
        .map(|(i, (_, c))| format!("{}. {}", i + 1, c.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let ctx = crate::usage_ledger::LedgerCtx::surface("memory").tenant(None, Some(user_id));
    let raw = crate::usage_ledger::scope(ctx, crate::gizzi_completion::complete_ephemeral(&prompt, Some(CURATE_SYSTEM), model.as_ref()))
        .await
        .ok_or("the curation model didn't answer")?;
    let plan = parse_curation(&raw, entries.len()).ok_or("the curation reply wasn't readable")?;
    apply_curation(db, user_id, &principal, &entries, &plan).map_err(|e| e.to_string())?;
    Ok((plan.merged.len(), plan.drop.len()))
}

pub fn apply_curation(
    db: &DbHandle,
    user_id: &str,
    principal: &str,
    entries: &[(String, String)],
    plan: &CurationPlan,
) -> rusqlite::Result<()> {
    let mut conn = db.connect()?;
    let tx = conn.transaction()?;
    for (content, from) in &plan.merged {
        tx.execute(
            "INSERT INTO cowork_memory_entries (id, user_id, content, type, source, owner_principal, grants)
             VALUES (?1, ?2, ?3, 'fact', 'curation', ?4, '[]')",
            params![uuid::Uuid::new_v4().to_string(), user_id, content, principal],
        )?;
        for i in from {
            tx.execute("DELETE FROM cowork_memory_entries WHERE id = ?1 AND user_id = ?2", params![entries[i - 1].0, user_id])?;
        }
    }
    for i in &plan.drop {
        tx.execute("DELETE FROM cowork_memory_entries WHERE id = ?1 AND user_id = ?2", params![entries[i - 1].0, user_id])?;
    }
    tx.commit()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurateBody {
    bot_id: String,
}

async fn curate_now(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<CurateBody>) -> Response {
    match curate_bot(&state.db, &user.user_id, &body.bot_id).await {
        Ok((merged, dropped)) => {
            mark_curated(&state.db, &body.bot_id);
            Json(json!({ "merged": merged, "dropped": dropped })).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

fn mark_curated(db: &DbHandle, bot_id: &str) {
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "UPDATE agents SET config = json_set(COALESCE(config, '{}'), '$.memoryCuratedAt', ?2) WHERE id = ?1",
            params![bot_id, chrono::Utc::now().to_rfc3339()],
        );
    }
}

/// Weekly curation (P7.4): bots with at least 20 saved entries whose memory
/// wasn't tidied in the last 7 days. Checked every 6 hours.
pub fn spawn_weekly(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        loop {
            tick.tick().await;
            let due: Vec<(String, String)> = state
                .db
                .connect()
                .ok()
                .and_then(|conn| {
                    conn.prepare(
                        "SELECT a.id, a.user_id FROM agents a
                         WHERE COALESCE(json_extract(a.config, '$.memoryCuratedAt'), '') < ?1
                           AND (SELECT COUNT(*) FROM cowork_memory_entries m
                                WHERE m.user_id = a.user_id AND m.owner_principal IN
                                  (COALESCE(a.principal_id, ''), 'a://local/bot/' || a.id)) >= 20",
                    )
                    .and_then(|mut st| {
                        let week_ago = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
                        st.query_map(params![week_ago], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.filter_map(Result::ok).collect())
                    })
                    .ok()
                })
                .unwrap_or_default();
            for (bot, user) in due {
                match curate_bot(&state.db, &user, &bot).await {
                    Ok((m, d)) => {
                        mark_curated(&state.db, &bot);
                        info!(bot = %bot, merged = m, dropped = d, "weekly memory curation");
                    }
                    Err(e) => warn!(bot = %bot, error = %e, "weekly memory curation skipped"),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> DbHandle {
        let temp = std::env::temp_dir().join(format!("mem-cur-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let db = DbHandle::new(temp.join("t.db")).unwrap();
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO agents (id, user_id, name, model, provider) VALUES ('ledger', 'u1', 'Ledger', 'sonnet', 'claude-cli')", []).unwrap();
        conn.execute(
            "INSERT INTO cowork_memory_entries (id, user_id, session_id, content, type, grants) VALUES
             ('m1', 'u1', 'ses_t1', 'Use 35% margin on H100', 'fact', '[]')",
            [],
        )
        .unwrap();
        db
    }

    #[test]
    fn promoting_a_thread_note_up_to_the_bot_then_the_project() {
        let db = db();
        let body = PromoteBody { scope: "bot".into(), bot_id: Some("ledger".into()), project_id: None };
        assert_eq!(promote_entry(&db, "u1", "m1", &body).unwrap(), "bot");
        let conn = db.connect().unwrap();
        let (owner, session): (Option<String>, Option<String>) = conn
            .query_row("SELECT owner_principal, session_id FROM cowork_memory_entries WHERE id = 'm1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(owner.as_deref(), Some("a://local/bot/ledger"));
        assert!(session.is_none());
        let promoted: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_events WHERE bot_id = 'ledger' AND event_type = 'memory.promoted'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(promoted, 1);

        let body = PromoteBody { scope: "project".into(), bot_id: None, project_id: Some("p1".into()) };
        promote_entry(&db, "u1", "m1", &body).unwrap();
        let (owner, project): (Option<String>, Option<String>) = conn
            .query_row("SELECT owner_principal, project_id FROM cowork_memory_entries WHERE id = 'm1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(owner.is_none());
        assert_eq!(project.as_deref(), Some("p1"));
        assert!(promote_entry(&db, "u2", "m1", &body).is_err(), "not your memory");
    }

    #[test]
    fn applying_a_curation_merges_and_drops() {
        let db = db();
        let entries = vec![
            ("a".to_string(), "Margin 35%".to_string()),
            ("b".to_string(), "Old margin 30%".to_string()),
            ("c".to_string(), "Margin target is 35%".to_string()),
        ];
        let conn = db.connect().unwrap();
        for (id, c) in &entries {
            conn.execute(
                "INSERT INTO cowork_memory_entries (id, user_id, content, type, owner_principal, grants) VALUES (?1, 'u1', ?2, 'fact', 'a://local/bot/ledger', '[]')",
                params![id, c],
            )
            .unwrap();
        }
        let plan = CurationPlan { merged: vec![("Margin target is 35%".into(), vec![1, 3])], drop: vec![2] };
        apply_curation(&db, "u1", "a://local/bot/ledger", &entries, &plan).unwrap();
        let left: Vec<String> = conn
            .prepare("SELECT content FROM cowork_memory_entries WHERE owner_principal = 'a://local/bot/ledger'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(left, vec!["Margin target is 35%".to_string()]);
    }

    #[test]
    fn reads_a_curation_plan_and_ignores_bad_numbers() {
        let plan = parse_curation(
            r#"```json {"merged":[{"content":"Margin target is 35% (Eoj, Sep 27)","from":[1,3]},{"content":"x","from":[2]}],"drop":[4, 9, 1]} ```"#,
            4,
        )
        .unwrap();
        assert_eq!(plan.merged, vec![("Margin target is 35% (Eoj, Sep 27)".to_string(), vec![1, 3])]);
        assert_eq!(plan.drop, vec![4]);
        assert!(parse_curation("no json", 3).is_none());
        assert_eq!(bot_of_principal("a://workspace/ws/bot/ledger").as_deref(), Some("ledger"));
    }
}
