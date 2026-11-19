//! Runtime signals to unit queue (step 5.2).
//!
//! `POST /memory/sleep/signal` calls [`handle`] when the `orchestrator` stage
//! is on. This module only records work in `sleep_units`; the orchestrator
//! (5.1) leases and runs it. `sleep_units.stage` holds the unit kind:
//! `daydream` (light pass), `nap` (full pass) or `capture` (a record that
//! scratch values and unsaved high-attention items must be captured first).
//!
//! States used: `queued`, `cancelled` (this module); `leased` units are never
//! touched except for the stop mark. Stop mark: `payload_json.stop_requested`
//! is set to `true` on a leased daydream/nap unit by `resume`; the cycle that
//! holds the lease checks it between steps and stops cleanly.
//!
//! One open (`queued` or `leased`) unit per (user, session, kind), so a
//! repeated signal never double-queues.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::config::Resolved;
use super::types::SignalEvent;

pub const KIND_DAYDREAM: &str = "daydream";
pub const KIND_NAP: &str = "nap";
pub const KIND_CAPTURE: &str = "capture";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub queued: Vec<String>,
    pub cancelled: Vec<String>,
}

/// An idle signal arrives at the cache TTL, so the session has been idle for
/// `cache_ttl_s`. A daydream runs while that is under `nap_after_ttl_multiple`
/// times the TTL, otherwise a nap. With the default 1.0 an idle at the TTL
/// starts a nap; a multiple above 1.0 makes the first idle a daydream.
pub fn idle_kind(nap_after_ttl_multiple: f64) -> &'static str {
    if 1.0 >= nap_after_ttl_multiple { KIND_NAP } else { KIND_DAYDREAM }
}

fn open_unit(conn: &Connection, user: &str, session: &str, kind: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM sleep_units WHERE user_id = ?1 AND stage = ?2
           AND state IN ('queued','leased')
           AND json_extract(payload_json, '$.session_id') = ?3 LIMIT 1",
    )?;
    let mut rows = stmt.query(params![user, kind, session])?;
    match rows.next()? {
        Some(r) => Ok(Some(r.get(0)?)),
        None => Ok(None),
    }
}

/// Queue one unit unless one is already open. Returns the new id, if any.
fn queue(conn: &Connection, user: &str, session: &str, kind: &str, extra: Value) -> rusqlite::Result<Option<String>> {
    if open_unit(conn, user, session, kind)?.is_some() {
        return Ok(None);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut payload = json!({ "session_id": session, "last_signal_at": chrono::Utc::now().to_rfc3339() });
    if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            p.insert(k.clone(), v.clone());
        }
    }
    conn.execute(
        "INSERT INTO sleep_units (id, user_id, stage, payload_json, state) VALUES (?1, ?2, ?3, ?4, 'queued')",
        params![id, user, kind, payload.to_string()],
    )?;
    Ok(Some(id))
}

fn resume(conn: &Connection, user: &str, session: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM sleep_units WHERE user_id = ?1 AND state = 'queued'
           AND stage IN ('daydream','nap') AND json_extract(payload_json, '$.session_id') = ?2",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![user, session], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &ids {
        conn.execute(
            "UPDATE sleep_units SET state = 'cancelled', updated_at = CURRENT_TIMESTAMP,
               finished_at = CURRENT_TIMESTAMP WHERE id = ?1 AND state = 'queued'",
            params![id],
        )?;
    }
    conn.execute(
        "UPDATE sleep_units SET payload_json = json_set(COALESCE(payload_json, '{}'), '$.stop_requested', json('true')),
           updated_at = CURRENT_TIMESTAMP
         WHERE user_id = ?1 AND state = 'leased' AND stage IN ('daydream','nap')
           AND json_extract(payload_json, '$.session_id') = ?2",
        params![user, session],
    )?;
    Ok(ids)
}

fn apply(conn: &Connection, cfg: &Resolved, user: &str, session: &str, event: SignalEvent) -> rusqlite::Result<Outcome> {
    let mut out = Outcome::default();
    match event {
        SignalEvent::Idle => {
            let kind = idle_kind(cfg.number("nap_after_ttl_multiple"));
            out.queued.extend(queue(conn, user, session, kind, json!({}))?);
        }
        SignalEvent::PreCompact => {
            out.queued.extend(queue(
                conn, user, session, KIND_CAPTURE,
                json!({ "capture": ["scratch_values", "unsaved_high_attention"] }),
            )?);
            out.queued.extend(queue(conn, user, session, KIND_NAP, json!({}))?);
        }
        SignalEvent::SessionEnd => {
            out.queued.extend(queue(conn, user, session, KIND_NAP, json!({}))?);
        }
        SignalEvent::Resume => out.cancelled = resume(conn, user, session)?,
    }
    Ok(out)
}

/// Never fails the caller: compaction must not wait on or break because of
/// Sleep. An internal error is logged and answers an empty outcome.
pub fn handle(conn: &Connection, cfg: &Resolved, user: &str, session: &str, event: SignalEvent) -> Outcome {
    match apply(conn, cfg, user, session, event) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("sleep signal {event:?} for session {session} failed: {e}");
            Outcome::default()
        }
    }
}
