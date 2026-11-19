//! The Sleep unit queue (step 5.1): leases, attempts and deferral over
//! `sleep_units` (V291), following Dream's claim pattern (`memory_dream.rs`
//! `claim`: a conditional UPDATE is the lease, attempts are bounded).
//!
//! Unit kinds (`sleep_units.stage`): `daydream` and `nap` run as cycles
//! (`cycle.rs`); `capture` units are records the cycle's capture stage marks
//! done. States:
//!
//! - `queued`: waiting for the worker.
//! - `leased`: a worker runs it until `lease_expires_at`; an expired lease is
//!   reclaimable and counts as one failed attempt.
//! - `done`, `cancelled` (resume before it started, or the orchestrator stage
//!   was turned off), `failed` is not used for units.
//! - `deferred`: it failed `unit_max_attempts` times in this cycle. It is not
//!   retried until the user's next cycle opens; then it goes back to `queued`
//!   with `attempts = 0`. `payload_json.deferrals` counts how often, and
//!   `payload_json.deferred_at` says when.
//! - `held`: the rest of a cycle that stopped cleanly (resume, or the
//!   per-cycle stop rule). `payload_json.resume_from_stage` names the next
//!   stage; the session's next signal puts it back to `queued`
//!   (`trigger::queue`), so the next idle continues from there.
//!
//! No migration: every state is a value of the existing `state` column.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::config;
use super::trigger::{self, KIND_NAP};
use super::types::Stage;
use crate::db::DbHandle;

pub const STATE_QUEUED: &str = "queued";
pub const STATE_LEASED: &str = "leased";
pub const STATE_DONE: &str = "done";
pub const STATE_CANCELLED: &str = "cancelled";
pub const STATE_DEFERRED: &str = "deferred";
pub const STATE_HELD: &str = "held";

/// Session id of units queued by the old clocks.
pub const SESSION_CLOCK: &str = "clock";
/// Session id of units queued by `POST /memory/sleep/run`.
pub const SESSION_MANUAL: &str = "manual";

/// SQLite's own timestamp format, so injected clocks compare with
/// `CURRENT_TIMESTAMP` values.
pub fn ts(t: DateTime<Utc>) -> String {
    super::strength::fmt_ts(t)
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    pub user_id: String,
    /// `daydream` or `nap`.
    pub kind: String,
    pub payload: Value,
    pub attempts: i64,
}

impl Unit {
    pub fn session(&self) -> &str {
        self.payload.get("session_id").and_then(Value::as_str).unwrap_or("")
    }

    pub fn resume_from(&self) -> Option<&str> {
        self.payload.get("resume_from_stage").and_then(Value::as_str)
    }

    /// `sleep_cycles.trigger`: `clock`, `manual`, or the signal that queued
    /// it (`idle`, `pre_compact`, `session_end`; `payload_json.event`).
    pub fn trigger(&self) -> String {
        match self.session() {
            SESSION_CLOCK => "clock".into(),
            SESSION_MANUAL => "manual".into(),
            _ => self.payload.get("event").and_then(Value::as_str).unwrap_or("idle").to_string(),
        }
    }
}

fn load(conn: &Connection, id: &str) -> rusqlite::Result<Option<Unit>> {
    conn.query_row(
        "SELECT id, user_id, stage, payload_json, attempts FROM sleep_units WHERE id = ?1",
        params![id],
        |r| {
            let payload: Option<String> = r.get(3)?;
            Ok(Unit {
                id: r.get(0)?,
                user_id: r.get(1)?,
                kind: r.get(2)?,
                payload: payload.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_else(|| json!({})),
                attempts: r.get(4)?,
            })
        },
    )
    .optional()
}

const DEFER_PAYLOAD: &str = "json_set(COALESCE(payload_json, '{}'), '$.deferred_at', ?2,
     '$.deferrals', COALESCE(json_extract(payload_json, '$.deferrals'), 0) + 1)";

/// Claims the oldest runnable `daydream`/`nap` unit for `worker`: `queued`, or
/// `leased` with an expired lease. A user with a live lease is skipped, so one
/// user's units run one at a time even with several workers. An expired lease
/// that reaches `max_attempts` is deferred instead of claimed.
pub fn claim(conn: &Connection, worker: &str, now: DateTime<Utc>, lease_s: i64, max_attempts: i64) -> rusqlite::Result<Option<Unit>> {
    let now_s = ts(now);
    let expires = ts(now + Duration::seconds(lease_s.max(1)));
    // A few rounds: another worker may win a race, or a unit may be deferred.
    for _ in 0..16 {
        let cand: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT u.id, u.state, u.attempts FROM sleep_units u
                 WHERE u.stage IN ('daydream','nap')
                   AND (u.state = 'queued' OR (u.state = 'leased' AND u.lease_expires_at < ?1))
                   AND NOT EXISTS (SELECT 1 FROM sleep_units o WHERE o.user_id = u.user_id AND o.id <> u.id
                                   AND o.state = 'leased' AND o.lease_expires_at >= ?1)
                 ORDER BY u.created_at ASC, u.rowid ASC LIMIT 1",
                params![now_s],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, state, attempts)) = cand else { return Ok(None) };
        // An expired lease means the worker died mid-cycle: one failed attempt.
        let bump: i64 = if state == STATE_LEASED { 1 } else { 0 };
        if bump == 1 && attempts + 1 >= max_attempts {
            conn.execute(
                &format!(
                    "UPDATE sleep_units SET state = 'deferred', attempts = attempts + 1, lease_owner = NULL,
                       lease_expires_at = NULL, last_error = 'lease expired', payload_json = {DEFER_PAYLOAD},
                       updated_at = CURRENT_TIMESTAMP
                     WHERE id = ?1 AND state = 'leased' AND lease_expires_at < ?3"
                ),
                params![id, now_s, now_s],
            )?;
            continue;
        }
        let n = conn.execute(
            "UPDATE sleep_units SET state = 'leased', lease_owner = ?2, lease_expires_at = ?3,
               attempts = attempts + ?4, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = ?5 AND (state = 'queued' OR lease_expires_at < ?6)",
            params![id, worker, expires, bump, state, now_s],
        )?;
        if n == 1 {
            return load(conn, &id);
        }
    }
    Ok(None)
}

/// Extends the lease while a cycle moves from stage to stage.
pub fn renew(conn: &Connection, id: &str, worker: &str, now: DateTime<Utc>, lease_s: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sleep_units SET lease_expires_at = ?3, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND state = 'leased' AND lease_owner = ?2",
        params![id, worker, ts(now + Duration::seconds(lease_s.max(1)))],
    )?;
    Ok(())
}

/// The stop mark `trigger::resume` sets on a leased unit.
pub fn stop_requested(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT json_extract(payload_json, '$.stop_requested') FROM sleep_units WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    Ok(v == Some(1))
}

/// One failed attempt: back to `queued`, or `deferred` once it has failed
/// `max_attempts` times. Returns the new state.
pub fn fail(conn: &Connection, id: &str, error: &str, max_attempts: i64, now: DateTime<Utc>) -> rusqlite::Result<String> {
    conn.execute(
        &format!(
            "UPDATE sleep_units SET attempts = attempts + 1, last_error = ?3, lease_owner = NULL, lease_expires_at = NULL,
               state = CASE WHEN attempts + 1 >= ?4 THEN 'deferred' ELSE 'queued' END,
               payload_json = CASE WHEN attempts + 1 >= ?4 THEN {DEFER_PAYLOAD} ELSE payload_json END,
               updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1"
        ),
        params![id, ts(now), error, max_attempts],
    )?;
    conn.query_row("SELECT state FROM sleep_units WHERE id = ?1", params![id], |r| r.get(0))
}

pub fn done(conn: &Connection, id: &str, cycle_id: &str, spend_usd: f64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sleep_units SET state = 'done', cycle_id = ?2, spend_usd = spend_usd + ?3, lease_owner = NULL,
           lease_expires_at = NULL, last_error = NULL, finished_at = ?4, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1",
        params![id, cycle_id, spend_usd, ts(now)],
    )?;
    Ok(())
}

pub fn cancel(conn: &Connection, id: &str, reason: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sleep_units SET state = 'cancelled', last_error = ?2, lease_owner = NULL, lease_expires_at = NULL,
           finished_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        params![id, reason],
    )?;
    Ok(())
}

/// A new cycle opened for `user`: units deferred in an earlier cycle get
/// another `max_attempts` tries after it.
pub fn release_deferred(conn: &Connection, user: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE sleep_units SET state = 'queued', attempts = 0, updated_at = CURRENT_TIMESTAMP
         WHERE user_id = ?1 AND state = 'deferred'",
        params![user],
    )
}

/// The rest of a stopped cycle, held until the session's next signal.
pub fn hold_rest(conn: &Connection, unit: &Unit, from_stage: &str, reason: &str) -> rusqlite::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut payload = unit.payload.clone();
    if let Some(p) = payload.as_object_mut() {
        p.remove("stop_requested");
        p.remove("deferred_at");
        p.insert("resume_from_stage".into(), json!(from_stage));
        p.insert("continued_from".into(), json!(unit.id));
        p.insert("stopped_by".into(), json!(reason));
    }
    conn.execute(
        "INSERT INTO sleep_units (id, user_id, stage, payload_json, state) VALUES (?1, ?2, ?3, ?4, 'held')",
        params![id, unit.user_id, unit.kind, payload.to_string()],
    )?;
    Ok(id)
}

/// Capture stage: marks the user's queued `capture` units done and returns
/// (id, what to capture) for each. The real capture of scratch values and
/// unsaved high-attention items lands with a later step; for now the cycle
/// records what was asked for.
pub fn finish_captures(conn: &Connection, user: &str, cycle_id: &str, now: DateTime<Utc>) -> rusqlite::Result<Vec<(String, Value)>> {
    let mut stmt = conn.prepare(
        "SELECT id, json_extract(payload_json, '$.capture') FROM sleep_units
         WHERE user_id = ?1 AND stage = 'capture' AND state = 'queued' ORDER BY created_at, rowid",
    )?;
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, what) in rows {
        conn.execute(
            "UPDATE sleep_units SET state = 'done', cycle_id = ?2, finished_at = ?3, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'queued'",
            params![id, cycle_id, ts(now)],
        )?;
        out.push((id, what.and_then(|w| serde_json::from_str(&w).ok()).unwrap_or(Value::Null)));
    }
    Ok(out)
}

/// `POST /memory/sleep/run`: queue a manual `nap` or `daydream`.
pub fn queue_manual(conn: &Connection, user: &str, kind: &str) -> rusqlite::Result<Option<String>> {
    trigger::queue(conn, user, SESSION_MANUAL, kind, json!({ "source": "manual" }))
}

/// For the old clocks (consolidation tick, nightly Dream, weekly curation).
/// When the user's `orchestrator` stage is on, a clock `nap` is queued (one
/// open per user) and this returns true: the caller must not run its job,
/// the Sleep cycle runs it. Off, or on any database error, it returns false
/// and the clock behaves exactly as before.
pub fn clock_feeds_queue(db: &DbHandle, user: &str, source: &str) -> bool {
    let Ok(conn) = db.connect() else { return false };
    match config::load(&conn, user) {
        Ok(cfg) if cfg.stage_on(Stage::Orchestrator) => {}
        _ => return false,
    }
    if let Err(e) = trigger::queue(&conn, user, SESSION_CLOCK, KIND_NAP, json!({ "source": source })) {
        tracing::warn!("sleep: {source} clock could not queue a nap: {e}");
    }
    true
}
