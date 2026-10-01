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

/// JSON Schema of the curation plan (O10). Requested from the model and
/// validated locally on every reply.
pub fn curation_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["merged", "drop"],
        "properties": {
            "merged": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["content", "from"],
                "properties": {
                    "content": { "type": "string", "minLength": 1 },
                    "from": { "type": "array", "minItems": 2, "items": { "type": "integer", "minimum": 1 } }
                }
            } },
            "drop": { "type": "array", "items": { "type": "integer", "minimum": 1 } }
        }
    })
}

#[derive(Debug, Default, PartialEq)]
pub struct CurationPlan {
    pub merged: Vec<(String, Vec<usize>)>,
    pub drop: Vec<usize>,
}

/// Parse a text reply (the fallback path when no structured output came
/// back): schema-validated, tolerant of fences only as a logged fallback.
pub fn parse_curation(raw: &str, n: usize) -> Option<CurationPlan> {
    crate::structured_output::parse_text(raw, &curation_schema(), "memory_curation").map(|v| plan_from_value(&v, n))
}

/// A schema-valid plan value → plan, dropping out-of-range numbers and
/// entries claimed twice.
pub fn plan_from_value(v: &Value, n: usize) -> CurationPlan {
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
    plan
}

/// What one bot's curation needs: its memory principal, entries (id,
/// content, oldest first) and the bot's own model.
pub struct CurationInput {
    pub principal: String,
    pub entries: Vec<(String, String)>,
    pub model: Option<(String, String)>,
}

impl CurationInput {
    pub fn prompt(&self) -> String {
        self.entries.iter().enumerate().map(|(i, (_, c))| format!("{}. {}", i + 1, c.trim())).collect::<Vec<_>>().join("\n")
    }
}

/// Load a bot's curation input; `None` when there is nothing to curate.
pub fn load_curation(db: &DbHandle, user_id: &str, bot_id: &str) -> Result<Option<CurationInput>, String> {
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
    Ok((entries.len() >= 2).then_some(CurationInput { principal, entries, model }))
}

/// Apply a schema-valid plan value; shadow its per-entry choices to S1.
pub fn finish_curation(db: &DbHandle, user_id: &str, input: &CurationInput, value: &Value) -> Result<(usize, usize), String> {
    let plan = plan_from_value(value, input.entries.len());
    apply_curation(db, user_id, &input.principal, &input.entries, &plan).map_err(|e| e.to_string())?;
    s1_shadow_curation(&input.entries, &plan);
    Ok((plan.merged.len(), plan.drop.len()))
}

/// Curate one bot's memory now. Returns (merged, dropped).
pub async fn curate_bot(db: &DbHandle, user_id: &str, bot_id: &str) -> Result<(usize, usize), String> {
    let Some(input) = load_curation(db, user_id, bot_id)? else {
        return Ok((0, 0));
    };
    let schema = curation_schema();
    let reply = crate::structured_output::complete_structured(&input.prompt(), Some(CURATE_SYSTEM), input.model.as_ref(), &schema)
        .await
        .ok_or("the curation model didn't answer")?;
    let value = crate::structured_output::resolve(&reply, &schema, "memory_curation").ok_or("the curation reply wasn't readable")?;
    finish_curation(db, user_id, &input, &value)
}

// ─── S1 shadow (closed-set part of curation) ────────────────────────────────

/// The per-entry action set the curation plan implies.
pub const CURATION_ACTIONS: [&str; 3] = ["KEEP", "MERGE", "DROP"];

/// What the plan did with entry `i` (1-based).
pub fn entry_action(plan: &CurationPlan, i: usize) -> &'static str {
    if plan.merged.iter().any(|(_, from)| from.contains(&i)) {
        "MERGE"
    } else if plan.drop.contains(&i) {
        "DROP"
    } else {
        "KEEP"
    }
}

/// S1 DecisionRequest body for one entry (SHADOW, backend "auto").
pub fn s1_curation_request(entries: &[(String, String)], i: usize) -> Value {
    let others: String = entries
        .iter()
        .enumerate()
        .filter(|(j, _)| j + 1 != i)
        .map(|(j, (_, c))| format!("{}. {}", j + 1, c.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let state: String = format!("Entry {i}: {}\n\nOther entries:\n{others}", entries[i - 1].1.trim()).chars().take(4000).collect();
    let id = format!("curation:{}", entries[i - 1].0);
    json!({ "state": state, "reversible": true, "backend": "auto", "request": {
        "envelope": { "abi_version": "1.0.0", "schema_id": "allternit.kernel.DecisionRequestV1", "schema_version": "1.0.0",
            "run_id": id, "node_id": "memory.curation" },
        "operation": "CHOICE", "state_projection_ref": id,
        "instructions": "should this memory entry be kept, merged with another entry that says the same thing, or dropped as stale or chatter",
        "decision_bank_id": "mem.curation_action.v1",
        "candidates": CURATION_ACTIONS.iter().map(|a| json!({ "candidate_id": a, "label": a })).collect::<Vec<_>>(),
        "calibration_domain": "mem.curation_action" } })
}

/// Shadow the plan's per-entry choices to the S1 runtime and report the S2
/// (structured model) choice as the outcome. Advisory only: detached, every
/// error swallowed, never changes the plan. Capped at 64 entries a run.
/// `ALLTERNIT_S1_CURATION_SHADOW=0` turns it off.
fn s1_shadow_curation(entries: &[(String, String)], plan: &CurationPlan) {
    use allternit_commrails::kernel::{router::DecisionResultView, s1_outcome::OutcomeReporter};
    let reporter = OutcomeReporter::from_env();
    if !reporter.enabled || std::env::var("ALLTERNIT_S1_CURATION_SHADOW").is_ok_and(|v| v == "0") {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let jobs: Vec<(Value, &'static str)> =
        (1..=entries.len().min(64)).map(|i| (s1_curation_request(entries, i), entry_action(plan, i))).collect();
    rt.spawn(async move {
        let Ok(c) = reqwest::Client::builder().timeout(reporter.timeout).build() else { return };
        for (body, truth) in jobs {
            let mut rq = c.post(format!("{}/v1/decision", reporter.base_url)).json(&body);
            if let Some(t) = &reporter.token {
                rq = rq.bearer_auth(t);
            }
            let Ok(r) = rq.send().await else { return }; // runtime down: stop, don't retry per entry
            let Ok(view) = r.json::<DecisionResultView>().await else { continue };
            if let Some(id) = view.decision_id() {
                reporter.report(&id, truth, "memory_curation.s2").await;
            }
        }
    });
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
            let provider = crate::internal_batch::enabled().then(|| crate::internal_batch::provider_from_config(&state.config));
            curate_due(&state.db, due, provider.as_deref()).await;
        }
    });
}

/// Curate every due (bot, user). With a batch provider (O11,
/// `ALLTERNIT_INTERNAL_BATCH=1`) the plans are requested as one batch; any
/// bot the batch didn't settle falls back to a direct structured call.
pub async fn curate_due(db: &DbHandle, due: Vec<(String, String)>, batch: Option<&dyn crate::llm_gateway::batches::BatchProvider>) {
    let direct = match batch {
        Some(provider) => curate_batch(db, due, provider).await,
        None => due,
    };
    for (bot, user) in direct {
        match curate_bot(db, &user, &bot).await {
            Ok((m, d)) => {
                mark_curated(db, &bot);
                info!(bot = %bot, merged = m, dropped = d, "weekly memory curation");
            }
            Err(e) => warn!(bot = %bot, error = %e, "weekly memory curation skipped"),
        }
    }
}

/// The batch half of [`curate_due`]. Returns the bots it didn't settle (they
/// go to direct calls).
pub async fn curate_batch(db: &DbHandle, due: Vec<(String, String)>, provider: &dyn crate::llm_gateway::batches::BatchProvider) -> Vec<(String, String)> {
    let mut direct: Vec<(String, String)> = Vec::new();
    {
        {
            let mut jobs = Vec::new();
            for (bot, user) in due {
                match load_curation(db, &user, &bot) {
                    Ok(Some(input)) => jobs.push((bot, user, input)),
                    Ok(None) => mark_curated(db, &bot),
                    Err(e) => warn!(bot = %bot, error = %e, "weekly memory curation skipped"),
                }
            }
            let schema = curation_schema();
            let requests: Vec<Value> = jobs
                .iter()
                .map(|(_, _, i)| crate::structured_output::chat_request(i.model.as_ref(), CURATE_SYSTEM, &i.prompt(), "curation_plan", &schema))
                .collect();
            let results = if requests.is_empty() {
                Ok(vec![])
            } else {
                crate::internal_batch::submit_and_poll(provider, &requests, std::time::Duration::from_secs(15), std::time::Duration::from_secs(3600)).await
            };
            match results {
                Ok(results) => {
                    for ((bot, user, input), r) in jobs.into_iter().zip(results) {
                        let value = r.ok().and_then(|t| crate::structured_output::parse_text(&t, &schema, "memory_curation.batch"));
                        match value.map(|v| finish_curation(db, &user, &input, &v)) {
                            Some(Ok((m, d))) => {
                                mark_curated(db, &bot);
                                info!(bot = %bot, merged = m, dropped = d, "weekly memory curation (batch)");
                            }
                            _ => direct.push((bot, user)),
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "weekly memory curation batch failed; curating directly");
                    direct.extend(jobs.into_iter().map(|(b, u, _)| (b, u)));
                }
            }
        }
    }
    direct
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
            r#"```json {"merged":[{"content":"Margin target is 35% (Eoj, Sep 27)","from":[1,3]},{"content":"x","from":[2,1]}],"drop":[4, 9, 1]} ```"#,
            4,
        )
        .unwrap();
        assert_eq!(plan.merged, vec![("Margin target is 35% (Eoj, Sep 27)".to_string(), vec![1, 3])]);
        assert_eq!(plan.drop, vec![4]);
        assert!(parse_curation("no json", 3).is_none());
        assert!(parse_curation(r#"{"merged":[{"content":"x","from":[1]}],"drop":[]}"#, 3).is_none(), "schema: a merge needs 2+ entries");
        assert!(parse_curation(r#"{"merged":[],"drop":["2"]}"#, 3).is_none(), "schema: numbers are integers");
        assert_eq!(bot_of_principal("a://workspace/ws/bot/ledger").as_deref(), Some("ledger"));
    }

    #[test]
    fn schema_accepts_a_plan_and_rejects_extras() {
        let ok = json!({ "merged": [{ "content": "a", "from": [1, 2] }], "drop": [3] });
        assert!(crate::structured_output::validate(&curation_schema(), &ok).is_ok());
        let extra = json!({ "merged": [], "drop": [], "why": "x" });
        assert!(crate::structured_output::validate(&curation_schema(), &extra).is_err());
        assert!(crate::structured_output::validate(&curation_schema(), &json!({ "merged": [] })).is_err());
    }

    #[test]
    fn s1_shadow_requests_cover_the_closed_set() {
        let entries = vec![("a".to_string(), "x".to_string()), ("b".to_string(), "y".to_string()), ("c".to_string(), "z".to_string())];
        let plan = CurationPlan { merged: vec![("xz".into(), vec![1, 3])], drop: vec![2] };
        assert_eq!((entry_action(&plan, 1), entry_action(&plan, 2), entry_action(&plan, 3)), ("MERGE", "DROP", "MERGE"));
        assert_eq!(entry_action(&CurationPlan::default(), 1), "KEEP");
        let r = s1_curation_request(&entries, 2);
        assert_eq!(r["backend"], "auto");
        assert_eq!(r["request"]["operation"], "CHOICE");
        assert_eq!(r["request"]["candidates"].as_array().unwrap().len(), 3);
        assert!(r["state"].as_str().unwrap().starts_with("Entry 2: y"));
    }

    fn seed_bot(db: &DbHandle, n: usize) {
        let conn = db.connect().unwrap();
        for i in 0..n {
            conn.execute(
                "INSERT INTO cowork_memory_entries (id, user_id, content, type, owner_principal, grants) VALUES (?1, 'u1', ?2, 'fact', 'a://local/bot/ledger', '[]')",
                params![format!("e{i}"), format!("fact {i}")],
            )
            .unwrap();
        }
    }

    fn bot_contents(db: &DbHandle) -> Vec<String> {
        db.connect().unwrap()
            .prepare("SELECT content FROM cowork_memory_entries WHERE owner_principal = 'a://local/bot/ledger' ORDER BY content")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[tokio::test]
    async fn weekly_curation_through_a_mock_batch() {
        std::env::set_var("ALLTERNIT_S1_CURATION_SHADOW", "0");
        let db = db();
        seed_bot(&db, 3);
        let mock = crate::internal_batch::tests::MockBatch::new(vec![crate::internal_batch::tests::chat(
            r#"{"merged":[{"content":"facts 0+1","from":[1,2]}],"drop":[3]}"#,
        )]);
        let left = curate_batch(&db, vec![("ledger".into(), "u1".into())], &mock).await;
        assert!(left.is_empty());
        assert_eq!(bot_contents(&db), vec!["facts 0+1".to_string()]);
        let sub = mock.submitted.lock().unwrap();
        let req = &sub[0].1[0];
        assert_eq!(req["model"], "claude-cli/sonnet");
        assert_eq!(req["response_format"]["type"], "json_schema");
        assert_eq!(req["response_format"]["json_schema"]["schema"], curation_schema());
        let curated: Option<String> = db.connect().unwrap()
            .query_row("SELECT json_extract(config, '$.memoryCuratedAt') FROM agents WHERE id = 'ledger'", [], |r| r.get(0))
            .unwrap();
        assert!(curated.is_some());
    }

    #[tokio::test]
    async fn an_invalid_batch_reply_falls_back_to_a_direct_call() {
        std::env::set_var("ALLTERNIT_S1_CURATION_SHADOW", "0");
        let db = db();
        seed_bot(&db, 3);
        // Schema-invalid reply → nothing applied, the bot is handed back for
        // a direct call and isn't marked curated.
        let mock = crate::internal_batch::tests::MockBatch::new(vec![crate::internal_batch::tests::chat(r#"{"merged":"nope"}"#)]);
        let left = curate_batch(&db, vec![("ledger".into(), "u1".into())], &mock).await;
        assert_eq!(left, vec![("ledger".to_string(), "u1".to_string())]);
        let mut down = crate::internal_batch::tests::MockBatch::new(vec![]);
        down.fail_submit = true;
        assert_eq!(curate_batch(&db, vec![("ledger".into(), "u1".into())], &down).await.len(), 1, "batch down → direct");
        assert_eq!(bot_contents(&db).len(), 3);
        let curated: Option<String> = db.connect().unwrap()
            .query_row("SELECT json_extract(config, '$.memoryCuratedAt') FROM agents WHERE id = 'ledger'", [], |r| r.get(0))
            .unwrap();
        assert!(curated.is_none());
    }
}
