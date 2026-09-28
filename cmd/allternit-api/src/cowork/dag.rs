//! Cowork session ↔ A:// DAG integration (A_PROTOCOL §7/§16).
//!
//! The cowork chat path (a human talking to Gizzi through the agent-chat
//! bridge) now participates in the canonical coordination lifecycle instead
//! of being a parallel transcript:
//!
//! - Session start submits a canonical IntentEnvelope (§5/§6), creating the
//!   session's run. The linkage lives in the session row's `metadata` JSON
//!   (`a_intent_id` / `a_run_id` / `a_native_session_id`) so both the REST
//!   session surface and the agent-chat bridge (which only knows the native
//!   chat id) can resolve it.
//! - Gizzi tool executions during a turn are recorded as lightweight job
//!   rows on the session run (attributed `job_created`/`job.completed`/
//!   `job.failed` events). A_PROTOCOL defines lease semantics for
//!   Fabric-Transport workers; conversational tool calls are not leased
//!   work (§8.2 honesty — nobody holds an execution lease), so the jobs are
//!   recorded directly in `running`→terminal states without a claim. This
//!   is the minimal DAG-visible representation; it does not invent
//!   transport semantics.
//! - Turn completion writes a typed Result (`turn.completed`, idempotent on
//!   the assistant message id) and an A-T2 memory entry owned by the user's
//!   principal with an explicit grant to Al.
//! - Session completion finalizes the run (`run.completed`, idempotent on
//!   the session id).

use rusqlite::{params, Connection, OptionalExtension};

use allternit_cowork_runtime::sqlite_store;

/// Capability required by the session run's canonical job. Conversational
/// cowork execution is not Fabric-Transport-leased work, so the job names a
/// capability no transport worker declares — a fabric worker must never
/// claim a chat session's placeholder job (§8.2, §10).
pub const COWORK_CHAT_CAPABILITY: &str = "cowork.chat";

/// Linkage between a cowork session row and its A:// run/intent.
#[derive(Debug, Clone)]
pub struct SessionRunLink {
    pub session_id: String,
    pub run_id: String,
    pub intent_id: String,
    /// True when this call created the session row + intent + run.
    pub created: bool,
}

fn gizzi_principal(workspace: &str) -> String {
    format!("a://workspace/{workspace}/principal/gizzi")
}

fn al_principal(workspace: &str) -> String {
    format!("a://workspace/{workspace}/principal/al")
}

/// The user's workspace-scoped address (A_PROTOCOL §6 addressing example).
fn user_principal(workspace: &str, user_id: &str) -> String {
    format!("a://workspace/{workspace}/user/{user_id}")
}

fn link_from_metadata(session_id: &str, metadata: Option<&str>) -> Option<SessionRunLink> {
    let meta: serde_json::Value = serde_json::from_str(metadata?).ok()?;
    Some(SessionRunLink {
        session_id: session_id.to_string(),
        run_id: meta.get("a_run_id")?.as_str()?.to_string(),
        intent_id: meta.get("a_intent_id")?.as_str()?.to_string(),
        created: false,
    })
}

/// Resolve an existing session row's A:// linkage (pure lookup).
///
/// Lookup: a session row whose `metadata.a_native_session_id` equals
/// `native_session_id` (the UI path — the mode-session id is the chat key),
/// or whose row id equals it (API callers that use the row id as chatId).
pub fn find_session_run(
    conn: &Connection,
    user_id: &str,
    native_session_id: &str,
) -> Result<Option<SessionRunLink>, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT id, metadata FROM cowork_sessions
             WHERE user_id = ?1
               AND (id = ?2 OR json_extract(metadata, '$.a_native_session_id') = ?2)
             ORDER BY created_at DESC LIMIT 1",
            params![user_id, native_session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(id, meta)| link_from_metadata(&id, meta.as_deref())))
}

/// Submit the canonical intent for an EXISTING session row and stamp the
/// linkage into its metadata (merging with metadata already present).
pub fn link_run_to_session(
    conn: &mut Connection,
    user_id: &str,
    session_id: &str,
    native_session_id: Option<&str>,
    title: Option<&str>,
) -> Result<SessionRunLink, rusqlite::Error> {
    let workspace = format!("ws-{user_id}");
    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: format!("intent_cowork_{session_id}"),
        workspace: format!("a://workspace/{workspace}"),
        initiator: user_id.to_string(),
        delegator: None,
        target: Some(gizzi_principal(&workspace)),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "cowork.session".to_string(),
            description: title.unwrap_or("Cowork session").to_string(),
            payload: Some(serde_json::json!({
                "session_id": session_id,
                "native_session_id": native_session_id,
            })),
        },
        permissions: vec![],
        compute: None,
        model: None,
        approval: None,
        return_channel: Some(serde_json::json!("cowork")),
        causation_chain: vec![user_id.to_string()],
    };
    let submission = sqlite_store::submit_intent_for_user(conn, &envelope, Some(user_id))
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

    // Conversational execution is not fabric-leased work: require a
    // capability no transport worker declares so the placeholder job is
    // never claimable (see module docs).
    conn.execute(
        "UPDATE cowork_jobs SET required_capabilities = ?1
         WHERE run_id = ?2 AND json_extract(required_capabilities, '$') = json('[]')",
        params![
            serde_json::json!([COWORK_CHAT_CAPABILITY]).to_string(),
            submission.run_id
        ],
    )?;

    // Merge the linkage keys into the metadata the row already carries.
    let existing: Option<String> = conn
        .query_row(
            "SELECT metadata FROM cowork_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let mut meta: serde_json::Value = existing
        .as_deref()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !meta.is_object() {
        meta = serde_json::json!({});
    }
    if let Some(n) = native_session_id {
        meta["a_native_session_id"] = serde_json::json!(n);
    }
    meta["a_intent_id"] = serde_json::json!(submission.intent_id);
    meta["a_run_id"] = serde_json::json!(submission.run_id);
    conn.execute(
        "UPDATE cowork_sessions SET metadata = ?2 WHERE id = ?1",
        params![session_id, meta.to_string()],
    )?;

    Ok(SessionRunLink {
        session_id: session_id.to_string(),
        run_id: submission.run_id,
        intent_id: submission.intent_id,
        created: true,
    })
}

/// Resolve (or create) the A:// run backing a cowork session — the
/// agent-chat bridge entry point, which only knows the native chat id.
///
/// An existing linked row resolves to its link; an unlinked row is
/// backfilled; a missing row is materialized (session row + intent + run)
/// so chats that never POSTed `/cowork/sessions` still participate.
pub fn ensure_session_run(
    conn: &mut Connection,
    user_id: &str,
    native_session_id: Option<&str>,
    title: Option<&str>,
) -> Result<SessionRunLink, rusqlite::Error> {
    if let Some(native) = native_session_id.filter(|s| !s.is_empty()) {
        if let Some(link) = find_session_run(conn, user_id, native)? {
            return Ok(link);
        }
        // Row exists without linkage (pre-integration session): backfill.
        let row = conn
            .query_row(
                "SELECT id FROM cowork_sessions
                 WHERE user_id = ?1 AND id = ?2
                 ORDER BY created_at DESC LIMIT 1",
                params![user_id, native],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(session_id) = row {
            return link_run_to_session(conn, user_id, &session_id, Some(native), title);
        }
    }

    // No session row at all — materialize one so the linkage has an anchor.
    let session_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO cowork_sessions (id, user_id, title, status, mode, metadata)
         VALUES (?1, ?2, ?3, 'active', 'agent', NULL)",
        params![session_id, user_id, title.unwrap_or("Cowork Session")],
    )?;
    link_run_to_session(conn, user_id, &session_id, native_session_id, title)
}

/// Record a Gizzi tool execution as a lightweight job on the session run.
/// Idempotent on the tool call id. Returns the job id when this call created
/// the row.
pub fn record_tool_job(
    conn: &mut Connection,
    run_id: &str,
    user_id: &str,
    tool: &str,
    call_id: &str,
    message_id: &str,
    approval_request_id: Option<&str>,
) -> Result<Option<String>, rusqlite::Error> {
    let job_id = format!("tooljob_{call_id}");
    let payload = serde_json::json!({
        "tool": tool,
        "callID": call_id,
        "messageID": message_id,
        "approvalRequestID": approval_request_id,
    });
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO cowork_jobs
            (id, run_id, dag_node_id, job_type, state, payload, initiator, user_id)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?6)",
        params![
            job_id,
            run_id,
            call_id,
            format!("tool.{tool}"),
            payload.to_string(),
            user_id
        ],
    )?;
    if inserted == 0 {
        return Ok(None);
    }
    // Attribution: the human asked, Gizzi executed (A_PROTOCOL §13).
    let gizzi = run_workspace_principal(conn, run_id);
    sqlite_store::insert_event_idempotent(
        conn,
        run_id,
        "job_created",
        serde_json::json!({
            "job_id": job_id,
            "job_type": format!("tool.{tool}"),
            "tool": tool,
            "call_id": call_id,
            "message_id": message_id,
        }),
        Some(user_id),
        None,
        gizzi.as_deref(),
        None,
    )
    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(Some(job_id))
}

/// Complete (or fail) a previously recorded tool job with a typed Result.
/// Idempotent — only a `running` job transitions. Returns true on transition.
pub fn complete_tool_job(
    conn: &mut Connection,
    run_id: &str,
    user_id: &str,
    call_id: &str,
    success: bool,
    error: Option<&str>,
) -> Result<bool, rusqlite::Error> {
    let state = if success { "completed" } else { "failed" };
    let result = typed_tool_result(call_id, success, error);
    let updated = conn.execute(
        "UPDATE cowork_jobs SET state = ?1, result = ?2, completed_at = CURRENT_TIMESTAMP
         WHERE run_id = ?3 AND dag_node_id = ?4 AND state = 'running'",
        params![state, result.to_string(), run_id, call_id],
    )?;
    if updated == 0 {
        return Ok(false);
    }
    let gizzi = run_workspace_principal(conn, run_id);
    sqlite_store::insert_event_idempotent(
        conn,
        run_id,
        if success { "job.completed" } else { "job.failed" },
        serde_json::json!({
            "call_id": call_id,
            "result": result,
        }),
        Some(user_id),
        None,
        gizzi.as_deref(),
        None,
    )
    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(true)
}

/// Settle every tool job on the run still `running` — called when a turn's
/// event loop exits and when the session finalizes. The bridge stops reading
/// the bus once the session goes idle, so a tool's final update that lands
/// after that is never seen and its job would stay `running` forever (the
/// Progress panel's endless spinners). `success` follows the turn: a turn
/// that completed settles its leftovers `completed`, an errored one `failed`.
/// Only `tool.*` jobs are touched — the `cowork.session` placeholder is not
/// tool work. Returns the number of jobs settled.
pub fn settle_running_tool_jobs(
    conn: &mut Connection,
    run_id: &str,
    user_id: &str,
    success: bool,
) -> Result<usize, rusqlite::Error> {
    let call_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT dag_node_id FROM cowork_jobs
             WHERE run_id = ?1 AND state = 'running' AND job_type LIKE 'tool.%'
               AND dag_node_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let error = (!success).then_some("turn ended before the tool reported a result");
    let mut settled = 0;
    for call_id in call_ids {
        if complete_tool_job(conn, run_id, user_id, &call_id, success, error)? {
            settled += 1;
        }
    }
    Ok(settled)
}

fn typed_tool_result(call_id: &str, success: bool, error: Option<&str>) -> serde_json::Value {
    let mut r = serde_json::json!({
        "result_id": format!("result_{}", uuid::Uuid::new_v4()),
        "status": if success { "completed" } else { "failed" },
        "summary": format!("gizzi tool call {call_id}"),
        "call_id": call_id,
    });
    if let Some(e) = error {
        r["error"] = serde_json::json!(e);
    }
    r
}

/// The gizzi principal for the run's workspace, for event attribution.
fn run_workspace_principal(conn: &Connection, run_id: &str) -> Option<String> {
    let ws: Option<String> = conn
        .query_row(
            "SELECT workspace_id FROM cowork_runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .ok()?;
    ws.map(|w| gizzi_principal(&w))
}

/// Typed Result for a completed turn. `client_event_id` (the assistant
/// message id) makes the event idempotent across SSE replays/reconnects.
pub fn record_turn_result(
    conn: &mut Connection,
    run_id: &str,
    user_id: &str,
    assistant_message_id: &str,
    status: &str,
    usage: serde_json::Value,
    permission_mode: &str,
) -> Result<(), rusqlite::Error> {
    let result = serde_json::json!({
        "result_id": format!("result_{}", uuid::Uuid::new_v4()),
        "status": status,
        "summary": format!("cowork turn {assistant_message_id}"),
        "message_id": assistant_message_id,
        "usage": usage,
        "permission_mode": permission_mode,
    });
    sqlite_store::insert_event_idempotent(
        conn,
        run_id,
        "turn.completed",
        serde_json::json!({ "result": result }),
        Some(user_id),
        None,
        run_workspace_principal(conn, run_id).as_deref(),
        Some(assistant_message_id),
    )
    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(())
}

/// A-T2 memory grant for a completed turn: the entry is owned by the user's
/// workspace principal and explicitly grants Al read access (default-deny
/// cross-principal otherwise).
pub fn record_turn_memory(
    conn: &mut Connection,
    user_id: &str,
    run_id: &str,
    session_id: &str,
    summary: &str,
) -> Result<String, rusqlite::Error> {
    let workspace = run_workspace(conn, run_id).unwrap_or_else(|| format!("ws-{user_id}"));
    let owner = user_principal(&workspace, user_id);
    let grants = vec![al_principal(&workspace)];
    sqlite_store::store_memory_entry(
        conn,
        user_id,
        None,
        Some(session_id),
        summary,
        "turn-summary",
        Some("cowork"),
        Some("cowork-turn"),
        Some(&owner),
        &grants,
    )
    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn run_workspace(conn: &Connection, run_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT workspace_id FROM cowork_runs WHERE id = ?1",
        params![run_id],
        |row| row.get(0),
    )
    .ok()
}

/// Finalize the session's run when the session completes (PATCH status or
/// DELETE). Idempotent on a per-session client event id. Returns the run id
/// when finalized here.
pub fn finalize_session_run(
    conn: &mut Connection,
    session_id: &str,
    user_id: &str,
) -> Result<Option<String>, rusqlite::Error> {
    let link = match session_link(conn, user_id, session_id)? {
        Some(l) => l,
        None => return Ok(None),
    };
    // No-op when the run already reached a terminal state.
    let state: Option<String> = conn
        .query_row(
            "SELECT state FROM cowork_runs WHERE id = ?1",
            params![link.run_id],
            |row| row.get(0),
        )
        .optional()?;
    match state.as_deref() {
        Some("completed") | Some("failed") | Some("cancelled") => return Ok(None),
        _ => {}
    }
    // A completed run cannot hold running tool jobs.
    settle_running_tool_jobs(conn, &link.run_id, user_id, true)?;
    sqlite_store::update_run_state_record(conn, &link.run_id, "completed", Some(chrono_now()))
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    sqlite_store::insert_event_idempotent(
        conn,
        &link.run_id,
        "run.completed",
        serde_json::json!({
            "session_id": session_id,
            "result": {
                "result_id": format!("result_{}", uuid::Uuid::new_v4()),
                "status": "completed",
                "summary": format!("cowork session {session_id} completed"),
            },
        }),
        Some(user_id),
        None,
        run_workspace_principal(conn, &link.run_id).as_deref(),
        Some(&format!("session-final:{session_id}")),
    )
    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(Some(link.run_id))
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Resolve the link for an existing session row (user-scoped).
pub fn session_link(
    conn: &Connection,
    user_id: &str,
    session_id: &str,
) -> Result<Option<SessionRunLink>, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT metadata FROM cowork_sessions WHERE id = ?1 AND user_id = ?2",
            params![session_id, user_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?;
    Ok(row.and_then(|m| link_from_metadata(session_id, m.as_deref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Store DDL + the product tables the integration touches.
    const SESSION_DDL: &str = "
        CREATE TABLE cowork_sessions (
            id TEXT PRIMARY KEY, user_id TEXT NOT NULL, project_id TEXT,
            title TEXT, status TEXT NOT NULL DEFAULT 'idle',
            mode TEXT NOT NULL DEFAULT 'agent', checkpoint TEXT, metadata TEXT,
            started_at DATETIME, completed_at DATETIME,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
    ";

    fn scratch() -> Connection {
        let mut conn = Connection::open_in_memory().expect("db");
        sqlite_store::apply_store_ddl(&mut conn).expect("store ddl");
        conn.execute_batch(SESSION_DDL).expect("session ddl");
        conn
    }

    #[test]
    fn ensure_session_run_creates_session_intent_and_run() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), Some("T")).unwrap();
        assert!(link.created);

        // Idempotent: second call resolves the same link.
        let again = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();
        assert!(!again.created);
        assert_eq!(again.run_id, link.run_id);
        assert_eq!(again.session_id, link.session_id);

        // One run, one intent, both owned by the user (defect 1 contract).
        let (runs, ): (i64,) = conn
            .query_row("SELECT COUNT(*) FROM cowork_runs WHERE user_id = 'user-a'", [], |r| {
                Ok((r.get(0)?,))
            })
            .unwrap();
        assert_eq!(runs, 1);
        let run_user: String = conn
            .query_row(
                "SELECT user_id FROM cowork_runs WHERE id = ?1",
                params![link.run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run_user, "user-a");

        // Session metadata carries the linkage.
        let meta: String = conn
            .query_row(
                "SELECT metadata FROM cowork_sessions WHERE id = ?1",
                params![link.session_id],
                |r| r.get(0),
            )
            .unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
        assert_eq!(meta["a_native_session_id"], "chat-1");
        assert_eq!(meta["a_run_id"], link.run_id);
    }

    #[test]
    fn ensure_session_run_scoped_to_user() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();
        // Another user with the same native id gets their own session/run.
        let other = ensure_session_run(&mut conn, "user-b", Some("chat-1"), None).unwrap();
        assert_ne!(other.session_id, link.session_id);
        assert_ne!(other.run_id, link.run_id);
    }

    #[test]
    fn session_run_job_is_not_fabric_claimable() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();
        let caps: String = conn
            .query_row(
                "SELECT required_capabilities FROM cowork_jobs WHERE run_id = ?1",
                params![link.run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(caps, serde_json::json!([COWORK_CHAT_CAPABILITY]).to_string());
    }

    #[test]
    fn tool_jobs_record_and_complete_idempotently() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();

        let first = record_tool_job(&mut conn, &link.run_id, "user-a", "bash", "call-1", "msg-1", Some("req-9")).unwrap();
        assert!(first.is_some());
        // Duplicate create is a no-op.
        assert!(record_tool_job(&mut conn, &link.run_id, "user-a", "bash", "call-1", "msg-1", None)
            .unwrap()
            .is_none());

        // Approval binding landed in the payload (Part 2.5).
        let payload: String = conn
            .query_row(
                "SELECT payload FROM cowork_jobs WHERE dag_node_id = 'call-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["approvalRequestID"], "req-9");

        assert!(complete_tool_job(&mut conn, &link.run_id, "user-a", "call-1", true, None).unwrap());
        // Second completion is refused (idempotent).
        assert!(!complete_tool_job(&mut conn, &link.run_id, "user-a", "call-1", true, None).unwrap());

        let state: String = conn
            .query_row(
                "SELECT state FROM cowork_jobs WHERE dag_node_id = 'call-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "completed");

        // Events: job_created + job.completed, attributed.
        let events: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT event_type FROM cowork_run_events WHERE run_id = ?1 ORDER BY created_at")
                .unwrap();
            stmt.query_map(params![link.run_id], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(events.contains(&"job_created".to_string()));
        assert!(events.contains(&"job.completed".to_string()));
        let exec: String = conn
            .query_row(
                "SELECT executor FROM cowork_run_events WHERE event_type = 'job.completed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(exec.ends_with("/principal/gizzi"));
    }

    #[test]
    fn settle_running_tool_jobs_closes_leftovers_only() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();
        record_tool_job(&mut conn, &link.run_id, "user-a", "bash", "call-1", "msg-1", None).unwrap();
        record_tool_job(&mut conn, &link.run_id, "user-a", "read", "call-2", "msg-1", None).unwrap();
        record_tool_job(&mut conn, &link.run_id, "user-a", "edit", "call-3", "msg-1", None).unwrap();
        complete_tool_job(&mut conn, &link.run_id, "user-a", "call-1", true, None).unwrap();
        complete_tool_job(&mut conn, &link.run_id, "user-a", "call-2", false, Some("boom")).unwrap();

        // Only the job whose final update never arrived is settled.
        assert_eq!(settle_running_tool_jobs(&mut conn, &link.run_id, "user-a", false).unwrap(), 1);
        assert_eq!(settle_running_tool_jobs(&mut conn, &link.run_id, "user-a", false).unwrap(), 0);

        let state_of = |conn: &Connection, call: &str| -> String {
            conn.query_row(
                "SELECT state FROM cowork_jobs WHERE dag_node_id = ?1",
                params![call],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(state_of(&conn, "call-1"), "completed");
        assert_eq!(state_of(&conn, "call-2"), "failed");
        assert_eq!(state_of(&conn, "call-3"), "failed");
        // The session placeholder job is not tool work and stays as it was.
        let placeholder: String = conn
            .query_row(
                "SELECT state FROM cowork_jobs WHERE run_id = ?1 AND job_type = 'cowork.session'",
                params![link.run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(placeholder, "failed");
    }

    #[test]
    fn finalize_session_run_settles_running_tool_jobs() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();
        record_tool_job(&mut conn, &link.run_id, "user-a", "bash", "call-1", "msg-1", None).unwrap();
        finalize_session_run(&mut conn, &link.session_id, "user-a").unwrap();
        let state: String = conn
            .query_row("SELECT state FROM cowork_jobs WHERE dag_node_id = 'call-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "completed");
    }

    #[test]
    fn turn_result_and_memory_grant() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();

        record_turn_result(
            &mut conn,
            &link.run_id,
            "user-a",
            "msg-final",
            "complete",
            serde_json::json!({"outputTokens": 12}),
            "default",
        )
        .unwrap();
        // Idempotent on the assistant message id.
        record_turn_result(
            &mut conn,
            &link.run_id,
            "user-a",
            "msg-final",
            "complete",
            serde_json::json!({"outputTokens": 12}),
            "default",
        )
        .unwrap();
        let turns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cowork_run_events WHERE event_type = 'turn.completed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(turns, 1);

        let mem_id = record_turn_memory(&mut conn, "user-a", &link.run_id, &link.session_id, "did a thing").unwrap();
        let (owner, grants): (Option<String>, String) = conn
            .query_row(
                "SELECT owner_principal, grants FROM cowork_memory_entries WHERE id = ?1",
                params![mem_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let owner = owner.unwrap();
        assert!(owner.starts_with("a://workspace/") && owner.ends_with("/user/user-a"));
        let grants: serde_json::Value = serde_json::from_str(&grants).unwrap();
        assert!(grants.as_array().unwrap().iter().any(|g| g.as_str().unwrap().ends_with("/principal/al")));
    }

    #[test]
    fn finalize_session_run_is_idempotent() {
        let mut conn = scratch();
        let link = ensure_session_run(&mut conn, "user-a", Some("chat-1"), None).unwrap();

        let first = finalize_session_run(&mut conn, &link.session_id, "user-a").unwrap();
        assert_eq!(first.as_deref(), Some(link.run_id.as_str()));
        assert!(finalize_session_run(&mut conn, &link.session_id, "user-a").unwrap().is_none());

        let state: String = conn
            .query_row(
                "SELECT state FROM cowork_runs WHERE id = ?1",
                params![link.run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "completed");

        let completions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cowork_run_events WHERE event_type = 'run.completed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(completions, 1);

        // Unknown / other-user session: no-op, not an error.
        assert!(finalize_session_run(&mut conn, "nope", "user-a").unwrap().is_none());
        assert!(finalize_session_run(&mut conn, &link.session_id, "user-b").unwrap().is_none());
    }
}
