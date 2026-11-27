//! Sleep routes under `/api/v1/memory/...` (contract: PLAN section 4, docs:
//! `surfaces/docs/api/memory-sleep.mdx`).
//!
//! Reads answer from the real tables, so on a new database they return real
//! empty states (`available: true, enabled: false`, empty lists, nulls).
//! Writes that need a later stage validate their input, then refuse with
//! `{"accepted": false, "reason": ...}`: `sleep_disabled` when Sleep or the
//! stage's flag is off, `stage_not_built` when the flags are on but that stage
//! has not landed yet. User actions (`resolve` with the `disputes` stage off,
//! `pages/load`) refuse with 409
//! (`run` queues a manual nap or daydream with 202 once the `orchestrator`
//! stage is on, step 5.1; `cycles/:id/undo` is real since step 5.5); `signal` answers 200, because the runtime sends it on every
//! idle, compaction and session end whether Sleep is on or not.
//! `disputes/:id/resolve` is real with the `disputes` stage on (step 6.1).
//! `PUT settings` and `PUT config` really write `sleep_config` rows (owner).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::config::{self, ConfigError, Group, Resolved};
use super::types::{
    ConfigUpdate, ContextQuery, CycleSummary, DisputeView, ListQuery,
    LoadPageRequest, PageSummary, ResolveDisputeRequest, RunRequest, SettingsUpdate,
    SignalRequest, SleepStatus, Stage, Tracks, ClaimView,
};
use crate::auth::get_user;
use crate::AppState;

type Reply = (StatusCode, Json<Value>);

pub fn sleep_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory/sleep/status", get(status))
        .route("/memory/sleep/cycles", get(list_cycles))
        .route("/memory/sleep/cycles/:id", get(get_cycle))
        .route("/memory/sleep/cycles/:id/undo", post(undo_cycle))
        .route("/memory/sleep/run", post(run))
        .route("/memory/sleep/signal", post(signal))
        .route("/memory/sleep/context", get(context))
        .route("/memory/sleep/pages/load", post(load_page))
        .route("/memory/sleep/pages", get(list_pages))
        .route("/memory/sleep/pages/:topic", get(get_topic_pages))
        .route("/memory/disputes", get(list_disputes))
        .route("/memory/disputes/:id/resolve", post(resolve_dispute))
        .route("/memory/sleep/settings", get(get_settings).put(put_settings))
        .route("/memory/sleep/config", get(get_config).put(put_config))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn err(status: StatusCode, msg: impl Into<String>) -> Reply {
    (status, Json(json!({ "error": msg.into() })))
}

fn db_err(e: impl std::fmt::Display) -> Reply {
    tracing::warn!("sleep route database error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Same identity as the other `/memory` handlers. An empty id would address
/// the global `sleep_config` row, so it is refused too.
fn user_id(headers: &HeaderMap) -> Result<String, Reply> {
    match get_user(headers) {
        Some(u) if !u.user_id.is_empty() => Ok(u.user_id),
        _ => Err(err(StatusCode::UNAUTHORIZED, "Unauthorized")),
    }
}

fn open(state: &AppState, user: &str) -> Result<(Connection, Resolved), Reply> {
    let conn = state.db.connect().map_err(db_err)?;
    let cfg = config::load(&conn, user).map_err(db_err)?;
    Ok((conn, cfg))
}

fn limit(q: &ListQuery) -> i64 {
    q.limit.unwrap_or(50).clamp(1, 200)
}

/// The refusal for a write whose stage is not live. No stage is built in this
/// step, so with the flags on the answer is `stage_not_built`.
fn refused(cfg: &Resolved, stage: Stage, status: StatusCode) -> Reply {
    let reason = if cfg.stage_on(stage) { "stage_not_built" } else { "sleep_disabled" };
    let msg = if reason == "sleep_disabled" {
        format!("Sleep is off (stage `{}`). Turn it on in Memory settings.", stage.as_str())
    } else {
        format!("The `{}` stage is not available in this version yet.", stage.as_str())
    };
    (status, Json(json!({ "accepted": false, "reason": reason, "stage": stage.as_str(), "error": msg })))
}

/// `report_json` is the last column: it feeds the one-line `summary`.
const CYCLE_COLS: &str = "id, trigger, kind, state, stage, session_id, spend_usd, manifest_before,
     manifest_after, started_at, finished_at, undone_at, report_json";

fn cycle_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CycleSummary> {
    let (kind, state, spend_usd, undone_at): (String, String, f64, Option<String>) = (r.get(2)?, r.get(3)?, r.get(6)?, r.get(11)?);
    let report: Option<String> = r.get(12)?;
    Ok(CycleSummary {
        id: r.get(0)?,
        trigger: r.get(1)?,
        summary: super::report::summary(&kind, &state, spend_usd, undone_at.is_some(), report.as_deref()),
        kind,
        state,
        stage: r.get(4)?,
        session_id: r.get(5)?,
        spend_usd,
        manifest_before: r.get(7)?,
        manifest_after: r.get(8)?,
        started_at: r.get(9)?,
        finished_at: r.get(10)?,
        undone_at,
    })
}

fn latest_manifest(conn: &Connection, user: &str) -> rusqlite::Result<Option<(String, Option<String>)>> {
    conn.query_row(
        "SELECT id, signature FROM memory_manifests WHERE user_id = ?1
         ORDER BY created_at DESC, rowid DESC LIMIT 1",
        params![user],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

fn disputes(conn: &Connection, user: &str, state: Option<&str>, limit: i64) -> rusqlite::Result<Vec<DisputeView>> {
    super::disputes::list(conn, user, state, limit)
}

fn page_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PageSummary> {
    Ok(PageSummary {
        id: r.get(0)?,
        topic_id: r.get(1)?,
        version: r.get(2)?,
        sha256: r.get(3)?,
        manifest_id: r.get(4)?,
        provenance_tier: r.get(5)?,
        tokens: r.get(6)?,
        created_at: r.get(7)?,
    })
}

const PAGE_COLS: &str = "id, topic_id, version, sha256, manifest_id, provenance_tier, tokens, created_at";

// ── Status ──────────────────────────────────────────────────────────────────

fn build_status(conn: &Connection, cfg: &Resolved, user: &str) -> rusqlite::Result<SleepStatus> {
    let last_cycle = conn
        .query_row(
            &format!(
                "SELECT {CYCLE_COLS} FROM sleep_cycles WHERE user_id = ?1
                 ORDER BY started_at DESC, rowid DESC LIMIT 1"
            ),
            params![user],
            cycle_row,
        )
        .optional()?;
    let spend_today_usd: f64 = conn.query_row(
        "SELECT COALESCE(SUM(spend_usd), 0.0) FROM sleep_cycles
         WHERE user_id = ?1 AND started_at >= date('now')",
        params![user],
        |r| r.get(0),
    )?;
    let open_disputes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_disputes WHERE user_id = ?1 AND state != 'resolved'",
        params![user],
        |r| r.get(0),
    )?;
    let manifest_id = latest_manifest(conn, user)?.map(|(id, _)| id);
    Ok(SleepStatus {
        available: true,
        enabled: cfg.enabled(),
        stages: cfg.stage_flags(),
        debt: super::cycle::debt(conn, cfg, user, chrono::Utc::now())?,
        last_cycle,
        spend_today_usd,
        daily_spend_cap_usd: cfg.daily_spend_cap_usd(),
        manifest_id,
        open_disputes,
        tracks: Tracks {
            track1: if cfg.stage_on(Stage::Track1) { "on" } else { "off" },
            track2: if cfg.stage_on(Stage::Pages) { "on" } else { "off" },
        },
    })
}

async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let (conn, cfg) = match open(&state, &user) { Ok(v) => v, Err(e) => return e };
    match build_status(&conn, &cfg, &user) {
        Ok(s) => (StatusCode::OK, Json(json!(s))),
        Err(e) => db_err(e),
    }
}

// ── Cycles ──────────────────────────────────────────────────────────────────

async fn list_cycles(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    let rows = conn
        .prepare(&format!(
            "SELECT {CYCLE_COLS} FROM sleep_cycles WHERE user_id = ?1
             ORDER BY started_at DESC, rowid DESC LIMIT ?2"
        ))
        .and_then(|mut s| {
            let rows = s.query_map(params![user, limit(&q)], cycle_row)?.collect::<rusqlite::Result<Vec<_>>>();
            rows
        });
    match rows {
        Ok(cycles) => (StatusCode::OK, Json(json!({ "cycles": cycles, "count": cycles.len() }))),
        Err(e) => db_err(e),
    }
}

fn find_cycle(conn: &Connection, user: &str, id: &str) -> rusqlite::Result<Option<(CycleSummary, Option<String>)>> {
    conn.query_row(
        &format!("SELECT {CYCLE_COLS} FROM sleep_cycles WHERE user_id = ?1 AND id = ?2"),
        params![user, id],
        |r| Ok((cycle_row(r)?, r.get::<_, Option<String>>(12)?)),
    )
    .optional()
}

async fn get_cycle(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    match find_cycle(&conn, &user, &id) {
        Ok(Some((mut cycle, report))) => match super::report::build(&conn, &user, &mut cycle, report.as_deref()) {
            Ok(report) => (StatusCode::OK, Json(json!({ "cycle": cycle, "report": report }))),
            Err(e) => db_err(e),
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "Cycle not found"),
        Err(e) => db_err(e),
    }
}

/// Undoes a finished or stopped cycle: the Dream part through Dream's own undo
/// (later edits are kept), the manifest rollback when the cycle wrote one.
/// Needs no stage flag: a cycle only exists because Sleep ran, and the user
/// can always take its changes back.
async fn undo_cycle(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Reply {
    use super::report::UndoError;
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let (db, uid, cid) = (state.db.clone(), user.clone(), id.clone());
    let out = tokio::task::spawn_blocking(move || super::report::undo(&db, &uid, &cid)).await;
    match out {
        Ok(Ok(r)) => {
            let cycle = state.db.connect().ok().and_then(|c| find_cycle(&c, &user, &id).ok().flatten()).map(|(c, _)| c);
            (StatusCode::OK, Json(json!({
                "undone": true, "cycle": cycle, "undone_at": r.undone_at, "parts": r.parts, "cannot_undo": r.cannot_undo,
            })))
        }
        Ok(Err(UndoError::NotFound)) => err(StatusCode::NOT_FOUND, "Cycle not found"),
        Ok(Err(UndoError::AlreadyUndone)) => (StatusCode::CONFLICT, Json(json!({ "undone": false, "error": "already undone" }))),
        Ok(Err(UndoError::NotUndoable(why))) => (StatusCode::CONFLICT, Json(json!({ "undone": false, "error": why }))),
        Ok(Err(UndoError::Conflict(paths))) => (StatusCode::CONFLICT, Json(json!({
            "undone": false, "conflicts": paths,
            "error": "You edited lines the Dream changed after it ran, so this cycle was not undone. Nothing was changed.",
        }))),
        Ok(Err(UndoError::Failed(e))) => db_err(e),
        Err(e) => db_err(e),
    }
}

// ── Run and signal ──────────────────────────────────────────────────────────

const RUN_KINDS: [&str; 4] = ["daydream", "nap", "deep", "rem"];

async fn run(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Option<Json<RunRequest>>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let req = body.map(|Json(b)| b).unwrap_or_default();
    if let Some(kind) = req.kind.as_deref() {
        if !RUN_KINDS.contains(&kind) {
            return err(StatusCode::BAD_REQUEST, "kind must be daydream, nap, deep or rem");
        }
    }
    let (conn, cfg) = match open(&state, &user) { Ok(v) => v, Err(e) => return e };
    if !cfg.stage_on(Stage::Orchestrator) {
        return refused(&cfg, Stage::Orchestrator, StatusCode::CONFLICT);
    }
    // The orchestrator runs naps and daydreams; deep and REM come later (5.4).
    let kind = req.kind.as_deref().unwrap_or("nap");
    if !matches!(kind, "nap" | "daydream") {
        return (StatusCode::CONFLICT, Json(json!({
            "accepted": false, "reason": "stage_not_built", "stage": kind,
            "error": format!("A `{kind}` run is not available in this version yet."),
        })));
    }
    match super::queue::queue_manual(&conn, &user, kind) {
        // None: a manual run of this kind is already queued or running.
        Ok(id) => (StatusCode::ACCEPTED, Json(json!({ "accepted": true, "kind": kind, "queued": id.into_iter().collect::<Vec<_>>() }))),
        Err(e) => db_err(e),
    }
}

async fn signal(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<SignalRequest>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    if req.session_id.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "session_id is required");
    }
    if matches!(req.cache_ttl_s, Some(0)) {
        return err(StatusCode::BAD_REQUEST, "cache_ttl_s must be above 0");
    }
    let (conn, cfg) = match open(&state, &user) { Ok(v) => v, Err(e) => return e };
    if !cfg.stage_on(Stage::Orchestrator) {
        return refused(&cfg, Stage::Orchestrator, StatusCode::OK);
    }
    let out = super::trigger::handle(&conn, &cfg, &user, req.session_id.trim(), req.event);
    (StatusCode::OK, Json(json!({
        "accepted": true, "event": req.event, "queued": out.queued, "cancelled": out.cancelled,
    })))
}

// ── Context and pages ───────────────────────────────────────────────────────

fn build_context(conn: &Connection, cfg: &Resolved, user: &str) -> rusqlite::Result<Value> {
    let manifest = latest_manifest(conn, user)?;
    let open: Vec<DisputeView> = disputes(conn, user, None, 50)?
        .into_iter()
        .filter(|d| d.state != "resolved")
        .collect();
    // Fired reminders arrive once (then `done`); the rest only count.
    let (intentions, intentions_open) = if cfg.stage_on(Stage::Intentions) {
        super::intentions::deliver(conn, user, chrono::Utc::now())?
    } else {
        (Vec::new(), 0)
    };
    let mut stmt = conn.prepare(
        "SELECT id, topic_id, claim_id, correction, as_of FROM memory_overlays
         WHERE user_id = ?1 AND applied_at IS NULL ORDER BY created_at ASC LIMIT 200",
    )?;
    let overlay = stmt
        .query_map(params![user], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "topic_id": r.get::<_, Option<String>>(1)?,
                "claim_id": r.get::<_, Option<String>>(2)?,
                "correction": r.get::<_, String>(3)?,
                "as_of": r.get::<_, String>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (manifest_id, signature) = match manifest {
        Some((id, sig)) => (Some(id), sig),
        None => (None, None),
    };
    // Core, index and preloaded pages come with the router (step 4.4).
    Ok(json!({
        "available": true,
        "enabled": cfg.stage_on(Stage::Router),
        "core": null,
        "index": [],
        "pages": [],
        "overlay": overlay,
        "disputes": open,
        "intentions": intentions,
        "intentions_open": intentions_open,
        "budget": { "share": cfg.number("page_budget_share"), "tokens": null },
        "manifest_id": manifest_id,
        "signature": signature,
    }))
}

async fn context(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(_q): Query<ContextQuery>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let (conn, cfg) = match open(&state, &user) { Ok(v) => v, Err(e) => return e };
    match build_context(&conn, &cfg, &user) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => db_err(e),
    }
}

async fn load_page(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<LoadPageRequest>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    if req.topic_id.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "topic_id is required");
    }
    let (_conn, cfg) = match open(&state, &user) { Ok(v) => v, Err(e) => return e };
    refused(&cfg, Stage::Router, StatusCode::CONFLICT)
}

async fn list_pages(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    let pages = conn
        .prepare(&format!(
            "SELECT {PAGE_COLS} FROM memory_pages WHERE user_id = ?1
             ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))
        .and_then(|mut s| {
            let rows = s.query_map(params![user, limit(&q)], page_row)?.collect::<rusqlite::Result<Vec<_>>>();
            rows
        });
    let manifest = latest_manifest(&conn, &user);
    match (pages, manifest) {
        (Ok(pages), Ok(m)) => (
            StatusCode::OK,
            Json(json!({ "pages": pages, "count": pages.len(), "manifest_id": m.map(|(id, _)| id) })),
        ),
        (Err(e), _) | (_, Err(e)) => db_err(e),
    }
}

fn topic_pages(conn: &Connection, user: &str, topic: &str) -> rusqlite::Result<Option<Value>> {
    let Some((id, name, state)) = conn
        .query_row(
            "SELECT id, name, state FROM memory_topics WHERE user_id = ?1 AND id = ?2",
            params![user, topic],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let versions = conn
        .prepare(&format!(
            "SELECT {PAGE_COLS} FROM memory_pages WHERE user_id = ?1 AND topic_id = ?2 ORDER BY version DESC"
        ))?
        .query_map(params![user, topic], page_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let claims = conn
        .prepare(
            "SELECT id, claim, source_type, observation_id, fact_id, as_of, superseded_by
             FROM memory_page_claims WHERE user_id = ?1 AND topic_id = ?2 ORDER BY as_of DESC",
        )?
        .query_map(params![user, topic], |r| {
            Ok(ClaimView {
                id: r.get(0)?,
                claim: r.get(1)?,
                source_type: r.get(2)?,
                observation_id: r.get(3)?,
                fact_id: r.get(4)?,
                as_of: r.get(5)?,
                superseded_by: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(json!({
        "topic": { "id": id, "name": name, "state": state },
        "versions": versions,
        "claims": claims,
    })))
}

async fn get_topic_pages(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(topic): Path<String>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    match topic_pages(&conn, &user, &topic) {
        Ok(Some(v)) => (StatusCode::OK, Json(v)),
        Ok(None) => err(StatusCode::NOT_FOUND, "Topic not found"),
        Err(e) => db_err(e),
    }
}

// ── Disputes ────────────────────────────────────────────────────────────────

const DISPUTE_STATES: [&str; 3] = ["open", "needs_user", "resolved"];

async fn list_disputes(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    if let Some(s) = q.state.as_deref() {
        if !DISPUTE_STATES.contains(&s) {
            return err(StatusCode::BAD_REQUEST, "state must be open, needs_user or resolved");
        }
    }
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    match disputes(&conn, &user, q.state.as_deref(), limit(&q)) {
        Ok(list) => (StatusCode::OK, Json(json!({ "disputes": list, "count": list.len() }))),
        Err(e) => db_err(e),
    }
}

async fn resolve_dispute(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ResolveDisputeRequest>,
) -> Reply {
    use super::disputes::{resolve_by_user, ResolveError};
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let (conn, cfg) = match open(&state, &user) { Ok(v) => v, Err(e) => return e };
    let found = conn
        .query_row(
            "SELECT 1 FROM memory_disputes WHERE user_id = ?1 AND id = ?2",
            params![user, id],
            |_| Ok(()),
        )
        .optional();
    match found {
        Ok(Some(())) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "Dispute not found"),
        Err(e) => return db_err(e),
    }
    if !cfg.bool(&config::stage_key(Stage::Disputes)) {
        return refused(&cfg, Stage::Disputes, StatusCode::CONFLICT);
    }
    drop(conn);
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || resolve_by_user(&db, &user, &id, req.choice, req.reason.as_deref())).await;
    match res {
        Ok(Ok(d)) => (StatusCode::OK, Json(json!({ "accepted": true, "dispute": d }))),
        Ok(Err(ResolveError::NotFound)) => err(StatusCode::NOT_FOUND, "Dispute not found"),
        Ok(Err(e @ ResolveError::AlreadySettled)) => (
            StatusCode::CONFLICT,
            Json(json!({ "accepted": false, "reason": "already_settled", "error": e.to_string() })),
        ),
        Ok(Err(e)) => db_err(e),
        Err(e) => db_err(e),
    }
}

// ── Settings and config ─────────────────────────────────────────────────────

fn settings_body(cfg: &Resolved) -> Value {
    json!({
        "enabled": cfg.enabled(),
        "stages": cfg.stage_flags(),
        "daily_spend_cap_usd": cfg.daily_spend_cap_usd(),
        "entries": cfg.group(Group::Setting),
    })
}

fn config_err(e: ConfigError) -> Reply {
    match e {
        ConfigError::Db(e) => db_err(e),
        other => err(StatusCode::BAD_REQUEST, other.to_string()),
    }
}

async fn get_settings(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    match open(&state, &user) {
        Ok((_, cfg)) => (StatusCode::OK, Json(settings_body(&cfg))),
        Err(e) => e,
    }
}

async fn put_settings(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<SettingsUpdate>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let mut values: Vec<(String, Value)> = Vec::new();
    if let Some(on) = req.enabled {
        values.push((config::KEY_ENABLED.to_string(), Value::Bool(on)));
    }
    for (name, on) in req.stages.unwrap_or_default() {
        let Some(stage) = Stage::parse(&name) else {
            return err(StatusCode::BAD_REQUEST, format!("unknown stage: {name}"));
        };
        values.push((config::stage_key(stage), Value::Bool(on)));
    }
    if let Some(cap) = req.daily_spend_cap_usd {
        values.push((config::KEY_DAILY_SPEND_CAP.to_string(), json!(cap)));
    }
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    if let Err(e) = config::set_owner(&conn, &user, Group::Setting, &values) {
        return config_err(e);
    }
    match config::load(&conn, &user) {
        Ok(cfg) => (StatusCode::OK, Json(settings_body(&cfg))),
        Err(e) => db_err(e),
    }
}

async fn get_config(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    match open(&state, &user) {
        Ok((_, cfg)) => (StatusCode::OK, Json(json!({ "values": cfg.group(Group::Threshold) }))),
        Err(e) => e,
    }
}

async fn put_config(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<ConfigUpdate>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let values: Vec<(String, Value)> = req.values.into_iter().collect();
    let conn = match state.db.connect() { Ok(c) => c, Err(e) => return db_err(e) };
    if let Err(e) = config::set_owner(&conn, &user, Group::Threshold, &values) {
        return config_err(e);
    }
    match config::load(&conn, &user) {
        Ok(cfg) => (StatusCode::OK, Json(json!({ "values": cfg.group(Group::Threshold) }))),
        Err(e) => db_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn call(app: &Router, method: &str, uri: &str, who: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-allternit-user-id", who)
            .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn status_is_available_and_off_on_a_fresh_db_and_settings_flip_it() {
        let dir = std::env::temp_dir().join(format!("allternit-sleep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        // Mounted the way main.rs mounts the v1 routes.
        let app: Router = Router::new().nest("/api/v1", sleep_router()).with_state(st);
        const STATUS: &str = "/api/v1/memory/sleep/status";

        let (s, v) = call(&app, "GET", STATUS, "user-a", None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!((v["available"].clone(), v["enabled"].clone()), (json!(true), json!(false)));
        // Every stage is off by default except disputes (it deletes nothing).
        assert!(v["stages"].as_object().unwrap().iter().all(|(k, f)| f == &json!(k == "disputes")), "{v}");
        assert_eq!(v["stages"].as_object().unwrap().len(), Stage::ALL.len());
        assert_eq!((v["last_cycle"].clone(), v["manifest_id"].clone()), (Value::Null, Value::Null));
        assert_eq!((v["spend_today_usd"].as_f64(), v["daily_spend_cap_usd"].as_f64()), (Some(0.0), Some(0.0)));

        // Signed out is refused.
        let req = Request::builder().uri(STATUS).body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);

        // The owner turns Sleep and one stage on; status shows it, for this user only.
        let (s, v) = call(&app, "PUT", "/api/v1/memory/sleep/settings", "user-a",
            Some(json!({ "enabled": true, "stages": { "pages": true } }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["entries"]["stage.pages"]["source"], json!("owner"));
        let (_, v) = call(&app, "GET", STATUS, "user-a", None).await;
        assert_eq!((v["enabled"].clone(), v["stages"]["pages"].clone(), v["stages"]["router"].clone()),
            (json!(true), json!(true), json!(false)));
        assert_eq!(v["tracks"]["track2"], json!("on"));
        assert_eq!(call(&app, "GET", STATUS, "user-b", None).await.1["enabled"], json!(false));

        // An unknown stage is refused and writes nothing.
        let (s, _) = call(&app, "PUT", "/api/v1/memory/sleep/settings", "user-a",
            Some(json!({ "enabled": false, "stages": { "nope": true } }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(call(&app, "GET", STATUS, "user-a", None).await.1["enabled"], json!(true));

        // A run before the orchestrator exists is refused, not a panic or a 501.
        let (s, v) = call(&app, "POST", "/api/v1/memory/sleep/run", "user-b", Some(json!({}))).await;
        assert_eq!((s, v["accepted"].clone(), v["reason"].clone()),
            (StatusCode::CONFLICT, json!(false), json!("sleep_disabled")));
    }

    #[tokio::test]
    async fn signal_queues_units_only_with_the_orchestrator_on() {
        let dir = std::env::temp_dir().join(format!("allternit-sleep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let app: Router = Router::new().nest("/api/v1", sleep_router()).with_state(st.clone());
        const SIGNAL: &str = "/api/v1/memory/sleep/signal";
        let sig = |s: &str, e: &str| json!({ "session_id": s, "event": e, "cache_ttl_s": 300 });
        let units = |user: &str| -> Vec<(String, String)> {
            let conn = st.db.connect().unwrap();
            let mut stmt = conn.prepare("SELECT stage, state FROM sleep_units WHERE user_id = ?1 ORDER BY created_at, rowid").unwrap();
            stmt.query_map([user], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect()
        };
        let on = |who: &'static str| {
            let app = app.clone();
            async move {
                let (s, _) = call(&app, "PUT", "/api/v1/memory/sleep/settings", who,
                    Some(json!({ "enabled": true, "stages": { "orchestrator": true } }))).await;
                assert_eq!(s, StatusCode::OK);
            }
        };

        // Off: refused exactly as before, nothing written.
        let (s, v) = call(&app, "POST", SIGNAL, "user-off", Some(sig("s1", "idle"))).await;
        assert_eq!((s, v["accepted"].clone(), v["reason"].clone()), (StatusCode::OK, json!(false), json!("sleep_disabled")));
        assert!(units("user-off").is_empty());

        // An idle at the TTL queues a nap; a repeat does not double-queue; resume cancels it.
        on("user-a").await;
        let (s, v) = call(&app, "POST", SIGNAL, "user-a", Some(sig("s1", "idle"))).await;
        assert_eq!((s, v["accepted"].clone(), v["queued"].as_array().unwrap().len()), (StatusCode::OK, json!(true), 1), "{v}");
        let id = v["queued"][0].clone();
        assert_eq!(units("user-a"), vec![("nap".to_string(), "queued".to_string())]);
        let (_, v) = call(&app, "POST", SIGNAL, "user-a", Some(sig("s1", "idle"))).await;
        assert_eq!(v["queued"], json!([]));
        assert_eq!(units("user-a").len(), 1);
        let (_, v) = call(&app, "POST", SIGNAL, "user-a", Some(sig("s1", "resume"))).await;
        assert_eq!(v["cancelled"], json!([id]));
        assert_eq!(units("user-a"), vec![("nap".to_string(), "cancelled".to_string())]);

        // A leased unit gets the stop mark instead of being cancelled.
        let (_, v) = call(&app, "POST", SIGNAL, "user-a", Some(sig("s1", "session_end"))).await;
        let leased = v["queued"][0].as_str().unwrap().to_string();
        st.db.connect().unwrap().execute("UPDATE sleep_units SET state = 'leased' WHERE id = ?1", [&leased]).unwrap();
        let (_, v) = call(&app, "POST", SIGNAL, "user-a", Some(sig("s1", "resume"))).await;
        assert_eq!(v["cancelled"], json!([]));
        let p: String = st.db.connect().unwrap()
            .query_row("SELECT payload_json FROM sleep_units WHERE id = ?1", [&leased], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&p).unwrap()["stop_requested"], json!(true));

        // pre_compact: 200, capture first, then nap.
        on("user-b").await;
        let (s, v) = call(&app, "POST", SIGNAL, "user-b", Some(sig("s9", "pre_compact"))).await;
        assert_eq!((s, v["queued"].as_array().unwrap().len()), (StatusCode::OK, 2), "{v}");
        assert_eq!(units("user-b"), vec![
            ("capture".to_string(), "queued".to_string()), ("nap".to_string(), "queued".to_string())]);

        // With the orchestrator on, a manual run queues a nap (202); deep/REM are not built.
        let (s, v) = call(&app, "POST", "/api/v1/memory/sleep/run", "user-b", Some(json!({}))).await;
        assert_eq!((s, v["accepted"].clone(), v["kind"].clone()), (StatusCode::ACCEPTED, json!(true), json!("nap")), "{v}");
        assert_eq!(units("user-b").len(), 3);
        let (s, v) = call(&app, "POST", "/api/v1/memory/sleep/run", "user-b", Some(json!({ "kind": "rem" }))).await;
        assert_eq!((s, v["reason"].clone()), (StatusCode::CONFLICT, json!("stage_not_built")));
    }

    #[tokio::test]
    async fn disputes_list_with_side_texts_and_resolve_needs_the_stage() {
        let dir = std::env::temp_dir().join(format!("allternit-sleep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let conn = st.db.connect().unwrap();
        for (id, text) in [("f_a", "User lives in Austin."), ("f_b", "User lives in Denver.")] {
            conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, 'user-a', ?2, 0.9)", params![id, text]).unwrap();
        }
        conn.execute(
            "INSERT INTO memory_disputes (id, user_id, side_a_id, side_b_id, pick, reason, picked_by, state)
             VALUES ('dsp_1', 'user-a', 'f_a', 'f_b', 'b', 'It is the more recent of the two.', 'arbiter', 'resolved')",
            [],
        ).unwrap();
        let app: Router = Router::new().nest("/api/v1", sleep_router()).with_state(st);
        const RESOLVE: &str = "/api/v1/memory/disputes/dsp_1/resolve";

        let (s, v) = call(&app, "GET", "/api/v1/memory/disputes", "user-a", None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!((v["disputes"][0]["side_a"]["text"].clone(), v["disputes"][0]["side_b"]["text"].clone()),
            (json!("User lives in Austin."), json!("User lives in Denver.")));
        assert_eq!(v["disputes"][0]["picked_by"], json!("arbiter"));

        // Disputes are on by default; the owner switches the stage off here.
        let (s, _) = call(&app, "PUT", "/api/v1/memory/sleep/settings", "user-a", Some(json!({ "stages": { "disputes": false } }))).await;
        assert_eq!(s, StatusCode::OK);
        // Stage off: refused, nothing changes. Someone else's dispute is not found.
        let (s, v) = call(&app, "POST", RESOLVE, "user-a", Some(json!({ "choice": "a" }))).await;
        assert_eq!((s, v["reason"].clone()), (StatusCode::CONFLICT, json!("sleep_disabled")));
        assert_eq!(call(&app, "POST", RESOLVE, "user-b", Some(json!({ "choice": "a" }))).await.0, StatusCode::NOT_FOUND);

        let (s, _) = call(&app, "PUT", "/api/v1/memory/sleep/settings", "user-a",
            Some(json!({ "enabled": true, "stages": { "disputes": true } }))).await;
        assert_eq!(s, StatusCode::OK);
        let (s, v) = call(&app, "POST", RESOLVE, "user-a", Some(json!({ "choice": "a" }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!((v["dispute"]["pick"].clone(), v["dispute"]["picked_by"].clone(), v["dispute"]["side_b"]["current"].clone()),
            (json!("a"), json!("user"), json!(false)));
        let (s, v) = call(&app, "POST", RESOLVE, "user-a", Some(json!({ "choice": "b" }))).await;
        assert_eq!((s, v["reason"].clone()), (StatusCode::CONFLICT, json!("already_settled")));
    }

    #[tokio::test]
    async fn the_cycle_report_and_undo_routes_answer_for_the_owner_only() {
        let dir = std::env::temp_dir().join(format!("allternit-sleep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        st.db.connect().unwrap().execute(
            "INSERT INTO sleep_cycles (id, user_id, trigger, kind, state, finished_at, report_json)
             VALUES ('cyc_1', 'user-a', 'idle', 'daydream', 'finished', '2026-11-19 12:00:09',
                     '{\"state\":\"finished\",\"steps\":[{\"step\":\"consolidation\",\"status\":\"done\",\"spend_usd\":0.0,\"detail\":{\"merged\":2,\"decayed\":0}}]}')",
            [],
        ).unwrap();
        let app: Router = Router::new().nest("/api/v1", sleep_router()).with_state(st);

        let (s, v) = call(&app, "GET", "/api/v1/memory/sleep/cycles", "user-a", None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v["cycles"][0]["summary"].as_str().unwrap().contains("consolidation merged 2"), "{v}");
        let (_, v) = call(&app, "GET", "/api/v1/memory/sleep/status", "user-a", None).await;
        assert!(v["last_cycle"]["summary"].as_str().is_some(), "{v}");
        let (_, v) = call(&app, "GET", "/api/v1/memory/sleep/cycles/cyc_1", "user-a", None).await;
        assert_eq!(v["report"]["stages"].as_array().unwrap().len(), 2, "a daydream lists its two stages: {v}");

        assert_eq!(call(&app, "POST", "/api/v1/memory/sleep/cycles/cyc_1/undo", "user-b", None).await.0, StatusCode::NOT_FOUND);
        let (s, v) = call(&app, "POST", "/api/v1/memory/sleep/cycles/cyc_1/undo", "user-a", None).await;
        assert_eq!((s, v["undone"].clone()), (StatusCode::OK, json!(true)), "{v}");
        let (s, v) = call(&app, "POST", "/api/v1/memory/sleep/cycles/cyc_1/undo", "user-a", None).await;
        assert_eq!((s, v["error"].clone()), (StatusCode::CONFLICT, json!("already undone")));
    }
}
