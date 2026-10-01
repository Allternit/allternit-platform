//! WP-C3a: the `thread:` effect connector (THREAD_WORK).
//!
//! Posts the run's reply into an Allternit thread as a `thread.message` event
//! on the thread's bot ledger (the same ledger `GET /threads/:id/events`
//! reads), as the run's owner, and touches the thread's activity clock.
//!
//! Called only from inside `Exec::effect_with` (P1's fenced, prepared →
//! committed effect journal), so a committed post is never re-applied. The
//! ledger write carries a stable idempotency key as well, so a takeover after
//! a crash between "applied" and "committed" replays the same row instead of
//! posting twice. Ownership is checked against the run's owner; an incognito
//! thread has no ledger and fails closed.

use crate::bot_event_routes::{append_event, ActorBody, AppendEventBody};
use crate::db::DbHandle;
use anyhow::{anyhow, bail, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::json;

pub const MESSAGE_EVENT: &str = "thread.message";
const MAX_TEXT: usize = 64 * 1024;

/// Post `text` to `thread_id` as `owner`. Returns the effect reference
/// `thread:<id>:event:<event id>` (same id on every replay of `key`).
pub fn post(db: &DbHandle, owner: &str, run_id: &str, node: &str, thread_id: &str, key: &str, text: &str) -> Result<String> {
    if thread_id.is_empty() || thread_id.len() > 128 || !thread_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
        bail!("bad thread id `{thread_id}`");
    }
    if text.trim().is_empty() { bail!("empty thread reply (nothing to post)"); }
    if text.len() > MAX_TEXT { bail!("thread reply exceeds {MAX_TEXT} bytes"); }
    let conn = db.connect()?;
    let row: Option<(String, String, bool, Option<String>)> = conn
        .query_row("SELECT user_id, bot_id, COALESCE(incognito, 0), current_session_id FROM bot_threads WHERE id = ?1", params![thread_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)))
        .optional()?;
    let Some((user_id, bot_id, incognito, session)) = row.filter(|(u, ..)| u == owner) else {
        bail!("thread {thread_id} not found for the run's owner (fail closed)");
    };
    if incognito { bail!("thread {thread_id} is incognito: no ledger to post to (fail closed)"); }
    let idem = format!("agency:{key}");
    let body = AppendEventBody {
        event_type: MESSAGE_EVENT.to_string(),
        actor: ActorBody { r#type: "user".to_string(), id: user_id },
        payload: json!({ "text": text, "via": "agency", "run_id": run_id, "node_id": node }),
        occurred_at: None,
        session_id: session,
        goal_id: None,
        wih_id: None,
        task_id: None,
        run_id: Some(run_id.to_string()),
        idempotency_key: Some(idem.clone()),
    };
    let ts = chrono::Utc::now().to_rfc3339();
    let (_, fresh) = append_event(db, &bot_id, &body, &ts).map_err(|e| anyhow!("thread post: {e}"))?;
    conn.execute("UPDATE bot_events SET thread_id = ?1 WHERE bot_id = ?2 AND idempotency_key = ?3", params![thread_id, bot_id, idem])?;
    if fresh {
        conn.execute("UPDATE bot_threads SET last_activity_at = ?2, updated_at = ?2 WHERE id = ?1", params![thread_id, ts])?;
    }
    let ev: String = conn.query_row("SELECT id FROM bot_events WHERE bot_id = ?1 AND idempotency_key = ?2", params![bot_id, idem], |r| r.get(0))?;
    Ok(format!("thread:{thread_id}:event:{ev}"))
}

/// Test fixture: an owned thread on a bot (shared with the executor e2e).
#[cfg(test)]
pub(crate) fn seed(db: &DbHandle, id: &str, owner: &str, incognito: bool) {
    let c = db.connect().unwrap();
    c.execute("INSERT OR IGNORE INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot_c3a', ?1, 'b', 'm', 'p', 1, '{}')", params![owner]).unwrap();
    c.execute("INSERT INTO bot_threads (id, user_id, bot_id, kind, title, status, incognito, created_at, updated_at, last_activity_at)
               VALUES (?1, ?2, 'bot_c3a', 'task', 't', 'working', ?3, 'x', 'x', 'x')", params![id, owner, incognito as i64]).unwrap();
}

/// Messages the agency posted to a thread (test helper).
#[cfg(test)]
pub(crate) fn count(db: &DbHandle, id: &str) -> i64 {
    db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = ?1 AND event_type = ?2", params![id, MESSAGE_EVENT], |r| r.get(0)).unwrap()
}

/// Test helper: P1's fenced effect path around a connector call, the way
/// `Exec::effect_with` uses it (prepare under the epoch → apply → commit).
/// Returns None when the fence refused (stale epoch): the connector never ran.
#[cfg(test)]
pub(crate) fn fenced(db: &DbHandle, run: &str, key: &str, epoch: i64, f: impl FnOnce() -> Result<String>) -> Option<String> {
    use crate::agency_api::safety::{commit, prepare, Prepared};
    match prepare(db, key, run, "N", "tool.execute", "h", epoch, false).unwrap() {
        Prepared::Stale => None,
        Prepared::Committed(prev) => Some(prev),
        Prepared::Fresh { .. } | Prepared::Takeover { .. } => {
            let r = f().unwrap();
            assert!(commit(db, key, run, epoch, &r).unwrap(), "commit under the fence");
            Some(r)
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn posts_once_per_key_and_fails_closed() {
        let t = crate::agency_api::tests::setup().await;
        let db = &t.st.db;
        seed(db, "th_c3a", "u1", false);
        seed(db, "th_inc", "u1", true);
        let a = post(db, "u1", "run_1", "T06", "th_c3a", "run_1:T06:thread:th_c3a", "hello").unwrap();
        let b = post(db, "u1", "run_1", "T06", "th_c3a", "run_1:T06:thread:th_c3a", "hello").unwrap();
        assert_eq!(a, b, "replay returns the same event");
        assert_eq!(count(db, "th_c3a"), 1, "no double post");
        post(db, "u1", "run_2", "T06", "th_c3a", "run_2:T06:thread:th_c3a", "again").unwrap();
        assert_eq!(count(db, "th_c3a"), 2);
        assert!(post(db, "u2", "run_3", "T06", "th_c3a", "k3", "x").is_err(), "someone else's thread");
        assert!(post(db, "u1", "run_3", "T06", "th_nope", "k4", "x").is_err(), "missing thread");
        assert!(post(db, "u1", "run_3", "T06", "th_inc", "k5", "x").is_err(), "incognito");
        assert!(post(db, "u1", "run_3", "T06", "../x", "k6", "x").is_err(), "bad id");
        assert!(post(db, "u1", "run_3", "T06", "th_c3a", "k7", "  ").is_err(), "empty");
    }

    #[tokio::test]
    async fn fenced_replay_never_reposts_and_stale_fence_refuses() {
        use crate::agency_api::safety::acquire_fence;
        let t = crate::agency_api::tests::setup().await;
        let db = &t.st.db;
        seed(db, "th_f", "u1", false);
        let key = "run_f:T06:tool.execute:1";
        let e1 = acquire_fence(db, "run_f", "w1").unwrap();
        let calls = std::cell::Cell::new(0);
        let apply = || { calls.set(calls.get() + 1); post(db, "u1", "run_f", "T06", "th_f", key, "hi") };
        let a = fenced(db, "run_f", key, e1, apply).unwrap();
        let b = fenced(db, "run_f", key, e1, apply).unwrap();
        assert_eq!((a, calls.get(), count(db, "th_f")), (b, 1, 1), "committed effect replays, never re-applied");
        let _e2 = acquire_fence(db, "run_f", "w2").unwrap();
        assert!(fenced(db, "run_f", "run_f:T06:tool.execute:2", e1, apply).is_none(), "stale fence refused");
        assert_eq!((calls.get(), count(db, "th_f")), (1, 1), "connector never ran under a stale fence");
    }
}
