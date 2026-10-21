//! Shared core behind the MCP and A2A façades over AAI. AAI (the thread /
//! turn / event / approval model) is canonical; these functions are the one
//! owner-scoped path the façades share, so a vendor bot always goes through
//! `gateway_runner::run_turn` and nothing here can answer an approval.

use crate::agent_gateway_routes::{rows, s};
use crate::gateway_runner::{self, RunErr, TurnOpts};
use crate::AppState;
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;

/// A façade-level failure: keeps the AAI code and approval id so callers can
/// surface them (MCP tool error text, A2A task status) instead of a bare string.
#[derive(Debug, Clone)]
pub struct FacadeErr {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub approval_id: Option<String>,
    pub retry_after_ms: Option<u64>,
}

impl FacadeErr {
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        FacadeErr { status, code: code.into(), message: message.into(), approval_id: None, retry_after_ms: None }
    }
    pub fn to_json(&self) -> Value {
        json!({ "error": self.message, "code": self.code, "status": self.status, "approvalId": self.approval_id, "retryAfterMs": self.retry_after_ms })
    }
}

impl From<RunErr> for FacadeErr {
    fn from(e: RunErr) -> Self {
        FacadeErr { status: e.status, code: e.code, message: e.message, approval_id: e.approval_id, retry_after_ms: e.retry_after_ms }
    }
}

impl From<rusqlite::Error> for FacadeErr {
    fn from(_: rusqlite::Error) -> Self {
        FacadeErr::new(500, "DB_ERROR", "database error")
    }
}

/// Provenance of a bot's execution: native, or which vendor/lane/guarantee it runs on.
pub fn provenance(row: &Value) -> Value {
    if s(row, "bindingType") == "vendor" {
        let caps = &row["capabilities"];
        json!({
            "kind": "vendor",
            "vendor": s(row, "vendor"),
            "lane": s(row, "lane"),
            "guarantee": caps["guarantee"].as_str().unwrap_or("unspecified"),
            "bindingState": s(row, "bindingState"),
        })
    } else if s(row, "bindingType") == "terminal" {
        // A CLI harness in an engine pane: never a vendor, so no lane or guarantee.
        json!({
            "kind": "terminal",
            "harness": s(row, "harness"),
            "machine": row.get("machine").cloned().unwrap_or(Value::Null),
            "paneId": row.get("paneId").cloned().unwrap_or(Value::Null),
            "bindingState": s(row, "bindingState"),
        })
    } else {
        json!({ "kind": "native" })
    }
}

pub fn agents_list(state: &AppState, user_id: &str, only: Option<&str>) -> Result<Vec<Value>, FacadeErr> {
    let conn = state.db.connect()?;
    let filter = only.unwrap_or("%");
    let mut out = rows(
        &conn,
        "SELECT a.id AS id, a.name AS name, e.type AS binding_type, e.vendor AS vendor, e.preferred_lane AS lane, \
                e.state AS binding_state, e.capabilities_json AS capabilities_json, \
                e.harness AS harness, e.machine AS machine, e.pane_id AS pane_id \
         FROM agents a LEFT JOIN bot_execution_bindings e ON e.bot_id = a.id AND e.owner = a.user_id \
         WHERE a.user_id = ?1 AND a.is_bot = 1 AND a.id LIKE ?2 ORDER BY a.name, a.id",
        &[&user_id, &filter],
    )?;
    for r in out.iter_mut() {
        let p = provenance(r);
        r["provenance"] = p;
    }
    Ok(out)
}

fn owned_thread(state: &AppState, user_id: &str, thread_id: &str) -> Result<(String, String, Option<String>), FacadeErr> {
    let conn = state.db.connect()?;
    conn.query_row(
        "SELECT bot_id, status, status_line FROM bot_threads WHERE id = ?1 AND user_id = ?2",
        params![thread_id, user_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .map_err(|_| FacadeErr::new(404, "NOT_FOUND", "thread not found"))
}

/// Server-derived status of a thread (never anything the caller said).
pub fn thread_status(state: &AppState, user_id: &str, thread_id: &str) -> Result<(String, Option<String>), FacadeErr> {
    let (_, status, line) = owned_thread(state, user_id, thread_id)?;
    Ok((status, line))
}

pub fn thread_events(state: &AppState, user_id: &str, thread_id: &str, after: Option<i64>, limit: Option<i64>) -> Result<Value, FacadeErr> {
    let (bot_id, _, _) = owned_thread(state, user_id, thread_id)?;
    let limit = limit.unwrap_or(100).clamp(1, 500);
    let conn = state.db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, seq, event_type, actor_type, actor_id, payload, session_id, occurred_at
         FROM bot_events WHERE bot_id = ?1 AND thread_id = ?2 AND (?4 IS NULL OR seq > ?4)
         ORDER BY CASE WHEN ?4 IS NULL THEN -seq ELSE seq END LIMIT ?3",
    )?;
    let events = stmt
        .query_map(params![bot_id, thread_id, limit, after], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "sequence": r.get::<_, i64>(1)?,
                "type": r.get::<_, String>(2)?,
                "actor": { "type": r.get::<_, String>(3)?, "id": r.get::<_, String>(4)? },
                "payload": r.get::<_, String>(5).ok().and_then(|p| serde_json::from_str::<Value>(&p).ok()).unwrap_or(Value::Null),
                "sessionId": r.get::<_, Option<String>>(6)?,
                "occurredAt": r.get::<_, String>(7)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<Value>>>()?;
    let cursor = events.iter().filter_map(|e| e["sequence"].as_i64()).max();
    Ok(json!({ "events": events, "cursor": cursor }))
}

pub fn approvals_list(state: &AppState, user_id: &str, thread_id: &str, st: Option<&str>) -> Result<Vec<Value>, FacadeErr> {
    owned_thread(state, user_id, thread_id)?;
    Ok(gateway_runner::list_approvals(&state.db, user_id, thread_id, st)?)
}

pub struct SendIn<'a> {
    pub bot_id: &'a str,
    pub thread_id: Option<&'a str>,
    pub text: &'a str,
    pub correlation_id: Option<String>,
    pub consequential: bool,
    pub allternit_approval_id: Option<String>,
    /// Which façade opened a new thread (recorded as its origin).
    pub via: &'a str,
}

/// One thread turn, through the same runner path as the REST message route.
pub async fn send(state: &Arc<AppState>, user_id: &str, i: SendIn<'_>) -> Result<Value, FacadeErr> {
    let conn = state.db.connect()?;
    let bot_ok: i64 = conn.query_row("SELECT COUNT(*) FROM agents WHERE id = ?1 AND user_id = ?2", params![i.bot_id, user_id], |r| r.get(0))?;
    if bot_ok == 0 {
        return Err(FacadeErr::new(404, "NOT_FOUND", "bot not found"));
    }
    let thread_id = match i.thread_id {
        Some(t) => {
            let (bot, _, _) = owned_thread(state, user_id, t)?;
            if bot != i.bot_id {
                return Err(FacadeErr::new(404, "NOT_FOUND", "thread not found for this bot"));
            }
            t.to_string()
        }
        None => {
            let title: String = i.text.chars().take(60).collect();
            let body: crate::thread_routes::CreateThreadBody = serde_json::from_value(json!({
                "botId": i.bot_id, "title": if title.is_empty() { "Task".to_string() } else { title },
                "kind": "task", "createdBy": "user", "origin": { "via": i.via },
            }))
            .map_err(|e| FacadeErr::new(400, "BAD_REQUEST", e.to_string()))?;
            let rt = crate::thread_routes::GizziRuntime { db: state.db.clone() };
            crate::thread_routes::create(&state.db, &rt, user_id, body).await.map_err(|e| FacadeErr::new(502, "THREAD_CREATE_FAILED", e))?.id
        }
    };
    let session: String = conn.query_row(
        "SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1",
        params![thread_id],
        |r| r.get(0),
    )?;
    drop(conn);
    let opts = TurnOpts { correlation_id: i.correlation_id, consequential: i.consequential, allternit_approval_id: i.allternit_approval_id };
    let tx = gateway_runner::transport(state);
    let rt = crate::thread_routes::GizziRuntime { db: state.db.clone() };
    match gateway_runner::run_turn(&state.db, tx.as_ref(), &rt, &session, i.text, opts).await? {
        Some(r) => Ok(json!({ "threadId": thread_id, "reply": r.reply, "pending": r.reply.is_none(), "events": r.events })),
        None => {
            let reply = crate::agent_session_routes::send_bot_turn(&state.db, &session, i.bot_id, i.text)
                .await
                .map_err(|e| FacadeErr::new(502, "TURN_FAILED", e))?;
            Ok(json!({ "threadId": thread_id, "reply": reply, "pending": false, "events": 0 }))
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use rusqlite::params;

    /// user-a owns bot-native and bot-vendor (vendor acme, lane api, given binding state); one thread each.
    pub async fn setup(tag: &str, vendor_state: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-facade-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        for (b, n) in [("bot-native", "Native"), ("bot-vendor", "Vendor")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'user-a', ?2, 'm', 'p', 1, '{}')", params![b, n]).unwrap();
        }
        for (t, b) in [("th-native", "bot-native"), ("th-vendor", "bot-vendor")] {
            c.execute(
                "INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES (?1,'user-a',?2,'T','working','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                params![t, b],
            )
            .unwrap();
            c.execute("INSERT INTO bot_thread_sessions (thread_id, generation, session_id, started_at) VALUES (?1, 1, ?2, '2026-01-01T00:00:00Z')", params![t, format!("s-{t}")]).unwrap();
        }
        c.execute(
            "INSERT INTO bot_execution_bindings (id, owner, bot_id, type, vendor, adapter_id, preferred_lane, external_agent_id, capabilities_json, state)
             VALUES ('eb-1','user-a','bot-vendor','vendor','acme','acme-adapter','api','agent-9','{\"resume\":true,\"guarantee\":\"exact\"}',?1)",
            params![vendor_state],
        )
        .unwrap();
        state
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::setup;
    use super::*;

    #[tokio::test]
    async fn agents_carry_provenance_and_are_owner_scoped() {
        let st = setup("prov", "READY").await;
        let a = agents_list(&st, "user-a", None).unwrap();
        assert_eq!(a.len(), 2);
        let v = a.iter().find(|x| x["id"] == "bot-vendor").unwrap();
        assert_eq!(v["provenance"]["vendor"], "acme");
        assert_eq!(v["provenance"]["lane"], "api");
        assert_eq!(v["provenance"]["guarantee"], "exact");
        assert_eq!(a.iter().find(|x| x["id"] == "bot-native").unwrap()["provenance"]["kind"], "native");
        assert!(agents_list(&st, "user-b", None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn send_surfaces_409_and_428_and_hides_other_users_threads() {
        let st = setup("send", "PAUSED").await;
        let e = send(&st, "user-a", SendIn { bot_id: "bot-vendor", thread_id: Some("th-vendor"), text: "hi", correlation_id: None, consequential: false, allternit_approval_id: None, via: "mcp" }).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (409, "BINDING_NOT_READY"));
        st.db.connect().unwrap().execute("UPDATE bot_execution_bindings SET state='READY'", []).unwrap();
        let e = send(&st, "user-a", SendIn { bot_id: "bot-vendor", thread_id: Some("th-vendor"), text: "delete it", correlation_id: Some("k1".into()), consequential: true, allternit_approval_id: None, via: "mcp" }).await.unwrap_err();
        assert_eq!(e.status, 428);
        assert!(e.approval_id.is_some());
        let e = send(&st, "user-b", SendIn { bot_id: "bot-vendor", thread_id: Some("th-vendor"), text: "hi", correlation_id: None, consequential: false, allternit_approval_id: None, via: "mcp" }).await.unwrap_err();
        assert_eq!(e.status, 404);
        // The pending approval is listed, and events page by cursor.
        assert_eq!(approvals_list(&st, "user-a", "th-vendor", Some("pending")).unwrap().len(), 1);
        let ev = thread_events(&st, "user-a", "th-vendor", None, Some(50)).unwrap();
        assert!(ev["events"].as_array().unwrap().len() >= 1);
        let cur = ev["cursor"].as_i64().unwrap();
        assert!(thread_events(&st, "user-a", "th-vendor", Some(cur), None).unwrap()["events"].as_array().unwrap().is_empty());
        assert!(thread_events(&st, "user-b", "th-vendor", None, None).is_err());
    }
}
