//! Thread close-out (W3, "threads as war rooms").
//!
//! One hook runs whenever a thread stops being live work:
//!
//! * [`thread_closed`] — resolved (done · failed · declined) or archived
//!   (`POST /threads/:id/resolve`, `PATCH` to a resolved status,
//!   `POST /threads/:id/archive`, the bot's `thread_close` tool).
//! * [`thread_paused`] — the owner paused it (`PATCH status = paused`). A
//!   usage-limit pause (`agent_session_routes::sync_thread_pause`) is not a
//!   close: it does not come through here.
//!
//! Both do the same three things:
//!
//! 1. **Task grants end.** `task_grants::revoke_for_thread` (W2, sync) — "allowed
//!    for this task" never outlives the task.
//! 2. **Reminders stop.** Routines (`routines`) and loops (`loops`) tied to
//!    the thread are set `paused` with `metadata.closedWithThread = <id>`.
//!    "Tied" means: created from one of the thread's sessions
//!    (`metadata.sourceThreadId`, stamped at create time by [`stamp_source_thread`];
//!    or `metadata.sessionId` / `loops.session_id` naming one of its sessions),
//!    or the thread is the routine's own home (`metadata.threadId`, see
//!    `thread_routes::routine_generation`). A bot's other routines are left alone.
//! 3. **gizzi's cron store follows.** `POST {cron}/thread/:id/close` with the
//!    thread's session ids and the tied daemon job ids: gizzi deletes the
//!    sessions' session-scoped loops (`CronService.cleanupSessionJobs`) on a
//!    close, disables them on a pause, and disables the tied persistent jobs.
//!
//! [`thread_reopened`] (Reopen, unarchive, resume from pause) turns back on
//! exactly what this hook stopped (`closedWithThread = <id>`). Session loops a
//! close deleted stay gone.
//!
//! The local scheduler also refuses to claim a routine whose tied thread is
//! resolved or archived ([`TIED_TO_CLOSED_THREAD`]), so a row this hook missed
//! (a race, a failed write) still never fires.

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::future::Future;
use std::time::Duration;
use tracing::{info, warn};

use crate::db::DbHandle;

/// SQL predicate over a `routines` row: it is tied to a thread that is resolved
/// or archived. Used by `routine_local_scheduler::claim_due` as `AND NOT …`.
pub const TIED_TO_CLOSED_THREAD: &str = "EXISTS (
    SELECT 1 FROM bot_threads ct
    WHERE ct.id IN (
        json_extract(CASE WHEN json_valid(routines.metadata) THEN routines.metadata ELSE '{}' END, '$.sourceThreadId'),
        json_extract(CASE WHEN json_valid(routines.metadata) THEN routines.metadata ELSE '{}' END, '$.threadId')
    )
    AND (ct.archived_at IS NOT NULL OR ct.status IN ('done', 'failed', 'declined'))
)";

/// How the thread stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseKind {
    /// Resolved or archived: session loops are deleted.
    Closed,
    /// Paused by its owner: session loops are disabled so a resume can bring them back.
    Paused,
}

/// What a close-out stopped (for logs and tests).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stopped {
    pub routines: Vec<String>,
    pub loops: Vec<String>,
    pub session_ids: Vec<String>,
    /// gizzi cron job ids of the stopped routines and loops.
    pub job_ids: Vec<String>,
}

/// gizzi's cron store, behind a seam so tests run without a daemon.
pub trait CronControl: Send + Sync {
    fn close(&self, thread_id: &str, stopped: &Stopped, kind: CloseKind) -> impl Future<Output = Result<Value, String>> + Send;
    fn reopen(&self, thread_id: &str, job_ids: &[String]) -> impl Future<Output = Result<Value, String>> + Send;
}

/// Production: the gizzi cron HTTP API (`AppConfig::cron_daemon_url`, the same
/// base `automation_routes` uses for routine/loop jobs).
pub struct GizziCron;

fn cron_base() -> String {
    crate::config::AppConfig::load().cron_daemon_url().trim_end_matches('/').to_string()
}

async fn cron_post(path: String, body: Value) -> Result<Value, String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().map_err(|e| e.to_string())?;
    let res = client.post(format!("{}{path}", cron_base())).json(&body).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("gizzi cron {}: {}", res.status(), res.text().await.unwrap_or_default()));
    }
    res.json::<Value>().await.map_err(|e| e.to_string())
}

impl CronControl for GizziCron {
    async fn close(&self, thread_id: &str, stopped: &Stopped, kind: CloseKind) -> Result<Value, String> {
        cron_post(
            format!("/thread/{}/close", urlencoding::encode(thread_id)),
            json!({
                "sessionIds": stopped.session_ids,
                "jobIds": stopped.job_ids,
                "remove": kind == CloseKind::Closed,
            }),
        )
        .await
    }
    async fn reopen(&self, thread_id: &str, job_ids: &[String]) -> Result<Value, String> {
        cron_post(format!("/thread/{}/reopen", urlencoding::encode(thread_id)), json!({ "jobIds": job_ids })).await
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Sessions (every generation) of a thread.
pub fn thread_sessions(conn: &Connection, thread_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation")?;
    let rows = stmt.query_map(params![thread_id], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// The thread a session belongs to, if any.
pub fn thread_for_session(conn: &Connection, session_id: &str) -> Option<String> {
    conn.query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![session_id], |r| r.get(0))
        .ok()
}

/// At create time: a routine or loop made from inside a thread's session
/// remembers that thread (`metadata.sourceThreadId`), so closing the thread
/// stops it. `session_id` is the loop's own session or `metadata.sessionId`.
pub fn stamp_source_thread(db: &DbHandle, metadata: &mut Option<Value>, session_id: Option<&str>) {
    let session = session_id
        .map(str::to_string)
        .or_else(|| metadata.as_ref().and_then(|m| m.get("sessionId")).and_then(Value::as_str).map(str::to_string));
    let Some(session) = session.filter(|s| !s.is_empty()) else { return };
    if metadata.as_ref().and_then(|m| m.get("sourceThreadId")).is_some() {
        return;
    }
    let Some(thread_id) = db.connect().ok().and_then(|c| thread_for_session(&c, &session)) else { return };
    let mut m = match metadata.take() {
        Some(Value::Object(o)) => Value::Object(o),
        _ => json!({}),
    };
    m["sourceThreadId"] = Value::String(thread_id);
    *metadata = Some(m);
}

const META: &str = "CASE WHEN json_valid(metadata) THEN metadata ELSE '{}' END";

/// Pause every routine and loop tied to the thread (DB side of the hook).
pub fn stop_tied_jobs(db: &DbHandle, thread_id: &str) -> rusqlite::Result<Stopped> {
    let conn = db.connect()?;
    let ts = now();
    let sessions = thread_sessions(&conn, thread_id)?;
    let routine_tie = format!(
        "status = 'active' AND (
            json_extract({META}, '$.sourceThreadId') = ?1
         OR json_extract({META}, '$.threadId') = ?1
         OR json_extract({META}, '$.sessionId') IN (SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1))"
    );
    let loop_tie = format!(
        "status = 'active' AND (
            json_extract({META}, '$.sourceThreadId') = ?1
         OR session_id IN (SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1))"
    );
    let pick = |table: &str, tie: &str| -> rusqlite::Result<Vec<(String, Option<String>)>> {
        let mut stmt = conn.prepare(&format!("SELECT id, gizzi_job_id FROM {table} WHERE {tie}"))?;
        let rows = stmt.query_map(params![thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    };
    let routines = pick("routines", &routine_tie)?;
    let loops = pick("loops", &loop_tie)?;
    for (table, rows) in [("routines", &routines), ("loops", &loops)] {
        for (id, _) in rows {
            conn.execute(
                &format!(
                    "UPDATE {table} SET status = 'paused',
                         metadata = json_set({META}, '$.closedWithThread', ?2), updated_at = ?3
                     WHERE id = ?1"
                ),
                params![id, thread_id, ts],
            )?;
        }
    }
    let job_ids = routines.iter().chain(loops.iter()).filter_map(|(_, j)| j.clone()).filter(|j| !j.is_empty()).collect();
    Ok(Stopped {
        routines: routines.into_iter().map(|(id, _)| id).collect(),
        loops: loops.into_iter().map(|(id, _)| id).collect(),
        session_ids: sessions,
        job_ids,
    })
}

/// Turn back on what [`stop_tied_jobs`] stopped for this thread. Local
/// routines get a fresh cursor (the scheduler's backfill computes it), so a
/// reopen doesn't fire every occurrence missed while the thread was closed.
/// Returns the gizzi job ids to re-enable.
pub fn restart_tied_jobs(db: &DbHandle, thread_id: &str) -> rusqlite::Result<Vec<String>> {
    let conn = db.connect()?;
    let ts = now();
    let mut job_ids = Vec::new();
    for table in ["routines", "loops"] {
        let mut stmt = conn.prepare(&format!(
            "SELECT gizzi_job_id FROM {table} WHERE json_extract({META}, '$.closedWithThread') = ?1"
        ))?;
        let ids: Vec<Option<String>> = stmt.query_map(params![thread_id], |r| r.get(0))?.filter_map(Result::ok).collect();
        job_ids.extend(ids.into_iter().flatten().filter(|j| !j.is_empty()));
        let cursor = if table == "routines" { ", next_run_at = NULL" } else { "" };
        conn.execute(
            &format!(
                "UPDATE {table} SET status = 'active', metadata = json_remove({META}, '$.closedWithThread'),
                     updated_at = ?2{cursor}
                 WHERE json_extract({META}, '$.closedWithThread') = ?1"
            ),
            params![thread_id, ts],
        )?;
    }
    Ok(job_ids)
}

/// The close-out hook for a resolved or archived thread.
pub async fn thread_closed(db: &DbHandle, thread_id: &str) -> Stopped {
    close_with(db, &GizziCron, thread_id, CloseKind::Closed).await
}

/// The close-out hook for a thread its owner paused.
pub async fn thread_paused(db: &DbHandle, thread_id: &str) -> Stopped {
    close_with(db, &GizziCron, thread_id, CloseKind::Paused).await
}

/// Reopen, unarchive, or resume: restart what the close-out stopped.
pub async fn thread_reopened(db: &DbHandle, thread_id: &str) {
    reopen_with(db, &GizziCron, thread_id).await
}

pub async fn close_with<C: CronControl>(db: &DbHandle, cron: &C, thread_id: &str, kind: CloseKind) -> Stopped {
    // 1. Task grants (W2) end with the task. 2. Routines and loops tied to the thread.
    let (d, tid) = (db.clone(), thread_id.to_string());
    let stopped = match tokio::task::spawn_blocking(move || {
        crate::task_grants::revoke_for_thread(&d, &tid);
        stop_tied_jobs(&d, &tid)
    })
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            warn!(thread = %thread_id, error = %e, "close-out: stopping tied routines failed");
            Stopped::default()
        }
        Err(e) => {
            warn!(thread = %thread_id, error = %e, "close-out task panicked");
            Stopped::default()
        }
    };

    // 3. gizzi's cron store: session loops + tied daemon jobs.
    if !stopped.session_ids.is_empty() || !stopped.job_ids.is_empty() {
        if let Err(e) = cron.close(thread_id, &stopped, kind).await {
            warn!(thread = %thread_id, error = %e, "close-out: gizzi cron cleanup failed (local scheduler still skips this thread)");
        }
    }
    info!(
        thread = %thread_id,
        kind = ?kind,
        routines = stopped.routines.len(),
        loops = stopped.loops.len(),
        "thread close-out done"
    );
    stopped
}

pub async fn reopen_with<C: CronControl>(db: &DbHandle, cron: &C, thread_id: &str) {
    let (d, tid) = (db.clone(), thread_id.to_string());
    let job_ids = match tokio::task::spawn_blocking(move || restart_tied_jobs(&d, &tid)).await {
        Ok(Ok(ids)) => ids,
        Ok(Err(e)) => {
            warn!(thread = %thread_id, error = %e, "reopen: restarting tied routines failed");
            return;
        }
        Err(_) => return,
    };
    if let Err(e) = cron.reopen(thread_id, &job_ids).await {
        warn!(thread = %thread_id, error = %e, "reopen: gizzi cron re-enable failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_routes::ThreadRuntime;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeCron {
        closes: Mutex<Vec<(String, Stopped, CloseKind)>>,
        reopens: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl CronControl for FakeCron {
        async fn close(&self, thread_id: &str, stopped: &Stopped, kind: CloseKind) -> Result<Value, String> {
            self.closes.lock().unwrap().push((thread_id.to_string(), stopped.clone(), kind));
            Ok(json!({ "deleted": stopped.session_ids.len() }))
        }
        async fn reopen(&self, thread_id: &str, job_ids: &[String]) -> Result<Value, String> {
            self.reopens.lock().unwrap().push((thread_id.to_string(), job_ids.to_vec()));
            Ok(json!({}))
        }
    }

    struct Rt;
    impl ThreadRuntime for Rt {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, _tid: &str) -> Result<String, String> {
            Ok(format!("ses-{}", uuid::Uuid::new_v4()))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Ok((format!("{s}-next"), json!({})))
        }
    }

    async fn setup(tag: &str) -> Arc<crate::AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-closeout-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config)
                 VALUES ('bot-1', 'user-a', 'ledger', 'm', 'p', 1, '{}')",
                [],
            )
            .unwrap();
        state
    }

    fn routine(state: &crate::AppState, id: &str, metadata: Value) {
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO routines (id, user_id, agent_id, name, description, status, schedule_type,
                     schedule_expression, execution_domain, config, metadata, gizzi_job_id)
                 VALUES (?1, 'user-a', 'bot-1', ?1, 'remind me', 'active', 'interval', '1h', 'local', '{}', ?2, ?3)",
                params![id, metadata.to_string(), format!("job-{id}")],
            )
            .unwrap();
    }

    fn status(state: &crate::AppState, table: &str, id: &str) -> String {
        state
            .db
            .connect()
            .unwrap()
            .query_row(&format!("SELECT status FROM {table} WHERE id = ?1"), params![id], |r| r.get(0))
            .unwrap()
    }

    async fn task_thread(state: &crate::AppState) -> crate::thread_routes::ThreadView {
        let body: crate::thread_routes::CreateThreadBody =
            serde_json::from_value(json!({ "botId": "bot-1", "title": "Book the venue", "kind": "task" })).unwrap();
        crate::thread_routes::create(&state.db, &Rt, "user-a", body).await.unwrap()
    }

    #[tokio::test]
    async fn closing_a_thread_revokes_grants_stops_its_reminders_and_leaves_others() {
        let state = setup("close").await;
        let t = task_thread(&state).await;
        let session = t.current_session_id.clone().unwrap();
        let conn = state.db.connect().unwrap();
        // A live grant on this thread (W2 table, V276).
        conn.execute(
            "INSERT INTO task_grants (id, owner, bot_id, thread_id, actions_json, source, source_ref, created_at, expires_at)
             VALUES ('g1', 'user-a', 'bot-1', ?1, '[\"send\"]', 'words', 'm1', ?2, ?3)",
            params![t.id, now(), (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339()],
        )
        .unwrap();
        // A reminder made in the thread, one made from its session, and an unrelated one.
        routine(&state, "r-tied", json!({ "sourceThreadId": t.id }));
        routine(&state, "r-session", json!({ "sessionId": session }));
        routine(&state, "r-other", json!({}));
        conn.execute(
            "INSERT INTO loops (id, user_id, agent_id, session_id, name, status, schedule_type, schedule_expression, config, gizzi_job_id)
             VALUES ('l1', 'user-a', 'bot-1', ?1, 'check in', 'active', 'interval', '10m', '{}', 'job-l1')",
            params![session],
        )
        .unwrap();

        let cron = FakeCron::default();
        let stopped = close_with(&state.db, &cron, &t.id, CloseKind::Closed).await;

        let revoked: Option<String> =
            conn.query_row("SELECT revoked_at FROM task_grants WHERE id = 'g1'", [], |r| r.get(0)).unwrap();
        assert!(revoked.is_some(), "the thread's grant is revoked");
        assert_eq!(status(&state, "routines", "r-tied"), "paused");
        assert_eq!(status(&state, "routines", "r-session"), "paused");
        assert_eq!(status(&state, "routines", "r-other"), "active", "other routines are untouched");
        assert_eq!(status(&state, "loops", "l1"), "paused");
        let closes = cron.closes.lock().unwrap();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].2, CloseKind::Closed);
        assert!(closes[0].1.session_ids.contains(&session));
        let mut jobs = stopped.job_ids.clone();
        jobs.sort();
        assert_eq!(jobs, vec!["job-l1", "job-r-session", "job-r-tied"]);

        // Reopen turns back on only what the close stopped.
        reopen_with(&state.db, &cron, &t.id).await;
        assert_eq!(status(&state, "routines", "r-tied"), "active");
        assert_eq!(status(&state, "loops", "l1"), "active");
        assert_eq!(cron.reopens.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_scheduler_predicate_skips_routines_of_closed_threads() {
        let state = setup("skip").await;
        let t = task_thread(&state).await;
        routine(&state, "r-tied", json!({ "sourceThreadId": t.id }));
        routine(&state, "r-other", json!({}));
        let conn = state.db.connect().unwrap();
        let skipped = |conn: &Connection| -> Vec<String> {
            let mut stmt = conn
                .prepare(&format!("SELECT id FROM routines WHERE {TIED_TO_CLOSED_THREAD} ORDER BY id"))
                .unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert!(skipped(&conn).is_empty(), "open thread: nothing skipped");
        conn.execute("UPDATE bot_threads SET archived_at = ?2 WHERE id = ?1", params![t.id, now()]).unwrap();
        assert_eq!(skipped(&conn), vec!["r-tied"]);
        conn.execute("UPDATE bot_threads SET archived_at = NULL, status = 'done' WHERE id = ?1", params![t.id]).unwrap();
        assert_eq!(skipped(&conn), vec!["r-tied"]);
    }

    #[tokio::test]
    async fn archive_route_closes_and_unarchive_reopens() {
        let state = setup("archive").await;
        let t = task_thread(&state).await;
        routine(&state, "r-tied", json!({ "sourceThreadId": t.id }));
        let archived = crate::thread_routes::set_archived(&state.db, &t.id, true, ("user", "user-a")).await.unwrap();
        assert!(archived.archived);
        assert_eq!(archived.group, "resolved");
        assert_eq!(status(&state, "routines", "r-tied"), "paused");
        let back = crate::thread_routes::set_archived(&state.db, &t.id, false, ("user", "user-a")).await.unwrap();
        assert!(!back.archived);
        assert_eq!(status(&state, "routines", "r-tied"), "active");
    }

    #[test]
    fn stamping_finds_the_thread_of_a_session() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let state = setup("stamp").await;
            let t = task_thread(&state).await;
            let mut meta = Some(json!({ "sessionId": t.current_session_id }));
            stamp_source_thread(&state.db, &mut meta, None);
            assert_eq!(meta.unwrap()["sourceThreadId"], json!(t.id));
            let mut none = None;
            stamp_source_thread(&state.db, &mut none, Some("no-such-session"));
            assert!(none.is_none());
        });
    }
}
