//! Scopes office-engine workbook sessions to the person who opened them.
//!
//! The engine knows nothing about users: anyone holding a session id can read
//! and rewrite that workbook. Every gateway in front of it records who opened
//! each session and answers "unknown session" for anyone else, so an id can't
//! be used, or even probed, across accounts. Forgetting a session is safe: the
//! client reopens its file and carries on.
//!
//! This file is shared: `allternit-cloud-api` includes it by path, because
//! api.allternit.com is served by that binary, not this one.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

pub const SESSION_OPS: &[&str] = &["open", "range", "formulas", "session-recalc", "save", "close"];

/// A 48 MB workbook in base64, in JSON. `open` and `save` are the big ones.
pub const OPEN_BODY_LIMIT: usize = 64 * 1024 * 1024;
const OTHER_BODY_LIMIT: usize = 16 * 1024 * 1024;
const SESSIONS_PER_USER: usize = 8;
/// Idle sessions go. The engine's own sweep runs later, as a backstop.
const SESSION_IDLE: Duration = Duration::from_secs(2 * 60 * 60);
/// Opening and saving parse whole files: don't let them pile up in memory.
const CONCURRENT_HEAVY_OPS: usize = 4;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(120);

struct Session {
    owner: String,
    last_used: Instant,
}

#[derive(Default)]
pub struct SessionGuard {
    sessions: HashMap<String, Session>,
}

impl SessionGuard {
    /// Records a new session. Returns the owner's oldest session if this one
    /// took them over the limit; the caller closes it.
    pub fn claim(&mut self, owner: &str, id: &str, now: Instant) -> Option<String> {
        self.sessions.insert(id.to_string(), Session { owner: owner.to_string(), last_used: now });
        let owned: Vec<(&String, &Session)> =
            self.sessions.iter().filter(|(_, session)| session.owner == owner).collect();
        if owned.len() <= SESSIONS_PER_USER {
            return None;
        }
        let oldest = owned
            .into_iter()
            .filter(|(session_id, _)| session_id.as_str() != id)
            .min_by_key(|(_, session)| session.last_used)
            .map(|(session_id, _)| session_id.clone())?;
        self.sessions.remove(&oldest);
        Some(oldest)
    }

    /// True only for the session's owner, and only while it hasn't expired.
    pub fn authorize(&mut self, owner: &str, id: &str, now: Instant) -> bool {
        match self.sessions.get_mut(id) {
            Some(session)
                if session.owner == owner && now.duration_since(session.last_used) < SESSION_IDLE =>
            {
                session.last_used = now;
                true
            }
            _ => false,
        }
    }

    pub fn release(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    /// Removes and returns idle sessions, for the caller to close.
    pub fn take_expired(&mut self, now: Instant) -> Vec<String> {
        let expired: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, session)| now.duration_since(session.last_used) >= SESSION_IDLE)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.sessions.remove(id);
        }
        expired
    }
}

static GUARD: OnceLock<Mutex<SessionGuard>> = OnceLock::new();
static HEAVY_OPS: OnceLock<Semaphore> = OnceLock::new();
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn guard() -> std::sync::MutexGuard<'static, SessionGuard> {
    GUARD
        .get_or_init(|| Mutex::new(SessionGuard::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

#[derive(Deserialize)]
struct RequestSession {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct OpenedFile {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[derive(Deserialize)]
struct SaveResponse {
    file: Option<OpenedFile>,
}

pub struct EngineReply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl EngineReply {
    fn error(status: u16, error: &str) -> Self {
        Self {
            status,
            content_type: Some("application/json".to_string()),
            body: format!("{{\"error\":\"{error}\"}}").into_bytes(),
        }
    }
}

/// Whether a request may reach the engine at all, before any session lookup.
pub fn precheck(op: &str, body_len: usize) -> Result<(), EngineReply> {
    if !SESSION_OPS.contains(&op) {
        return Err(EngineReply::error(404, "unknown_session_op"));
    }
    let limit = if op == "open" || op == "save" { OPEN_BODY_LIMIT } else { OTHER_BODY_LIMIT };
    if body_len > limit {
        return Err(EngineReply::error(413, "workbook_too_large"));
    }
    Ok(())
}

/// The session a request names, read the way the engine reads it (JSON's last
/// duplicate key wins in both). `open` names none.
pub fn requested_session(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<RequestSession>(body).ok()?.session_id
}

async fn close_quietly(engine_url: &str, id: String) {
    let _ = client()
        .post(format!("{engine_url}/xlsx/session/close"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::json!({ "sessionId": id }).to_string())
        .send()
        .await;
}

/// Forwards one session request for `owner`, enforcing all of the above.
pub async fn forward_session_op(engine_url: &str, owner: &str, op: &str, body: Vec<u8>) -> EngineReply {
    let engine_url = engine_url.trim_end_matches('/').to_string();
    if let Err(reply) = precheck(op, body.len()) {
        return reply;
    }

    let expired = guard().take_expired(Instant::now());
    for id in expired {
        close_quietly(&engine_url, id).await;
    }

    let session = if op == "open" {
        None
    } else {
        let Some(id) = requested_session(&body) else {
            return EngineReply::error(400, "expected_session_id");
        };
        // The same answer for someone else's session as for one that doesn't exist.
        if !guard().authorize(owner, &id, Instant::now()) {
            return EngineReply::error(404, "unknown session");
        }
        Some(id)
    };

    let heavy_slot = if op == "open" || op == "save" {
        match HEAVY_OPS.get_or_init(|| Semaphore::new(CONCURRENT_HEAVY_OPS)).acquire().await {
            Ok(permit) => Some(permit),
            Err(_) => return EngineReply::error(503, "office_engine_unavailable"),
        }
    } else {
        None
    };
    let upstream = client()
        .post(format!("{engine_url}/xlsx/session/{op}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await;
    let response = match upstream {
        Ok(response) => response,
        Err(_) => return EngineReply::error(503, "office_engine_unavailable"),
    };
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes.to_vec(),
        Err(_) => return EngineReply::error(503, "office_engine_unavailable"),
    };
    drop(heavy_slot);

    let now = Instant::now();
    let ok = (200..300).contains(&status);
    let evicted = match (op, ok) {
        ("open", true) => serde_json::from_slice::<OpenedFile>(&bytes)
            .ok()
            .and_then(|file| guard().claim(owner, &file.session_id, now)),
        // A save moves the workbook onto a new session id.
        ("save", true) => {
            let next = serde_json::from_slice::<SaveResponse>(&bytes).ok().and_then(|saved| saved.file);
            let mut sessions = guard();
            if let Some(old) = &session {
                sessions.release(old);
            }
            next.and_then(|file| sessions.claim(owner, &file.session_id, now))
        }
        ("close", _) => {
            if let Some(old) = &session {
                guard().release(old);
            }
            None
        }
        _ => None,
    };
    if let Some(id) = evicted {
        close_quietly(&engine_url, id).await;
    }
    EngineReply { status, content_type, body: bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nobody_reaches_another_persons_session() {
        let now = Instant::now();
        let mut sessions = SessionGuard::default();
        sessions.claim("alice", "s1", now);
        assert!(sessions.authorize("alice", "s1", now));
        assert!(!sessions.authorize("bob", "s1", now));
        assert!(!sessions.authorize("bob", "never-opened", now));
        // A refused attempt doesn't take the session over or keep it alive.
        assert!(sessions.authorize("alice", "s1", now));
    }

    #[test]
    fn idle_sessions_expire_and_use_keeps_them() {
        let start = Instant::now();
        let mut sessions = SessionGuard::default();
        sessions.claim("alice", "active", start);
        sessions.claim("alice", "idle", start);
        let later = start + SESSION_IDLE / 2;
        assert!(sessions.authorize("alice", "active", later));
        let past = start + SESSION_IDLE + Duration::from_secs(1);
        assert_eq!(sessions.take_expired(past), vec!["idle".to_string()]);
        assert!(!sessions.authorize("alice", "idle", past));
        assert!(sessions.authorize("alice", "active", past));
    }

    #[test]
    fn an_expired_session_is_refused_before_any_sweep() {
        let start = Instant::now();
        let mut sessions = SessionGuard::default();
        sessions.claim("alice", "s1", start);
        assert!(!sessions.authorize("alice", "s1", start + SESSION_IDLE));
    }

    #[test]
    fn one_person_cannot_hold_unlimited_sessions() {
        let start = Instant::now();
        let mut sessions = SessionGuard::default();
        for n in 0..SESSIONS_PER_USER {
            let at = start + Duration::from_secs(n as u64);
            assert_eq!(sessions.claim("alice", &format!("s{n}"), at), None);
        }
        let at = start + Duration::from_secs(100);
        // Their oldest goes, and nobody else's session is touched.
        sessions.claim("bob", "b0", start);
        assert_eq!(sessions.claim("alice", "new", at), Some("s0".to_string()));
        assert!(!sessions.authorize("alice", "s0", at));
        assert!(sessions.authorize("alice", "new", at));
        assert!(sessions.authorize("bob", "b0", at));
    }

    #[test]
    fn bodies_are_limited_and_only_known_ops_pass() {
        assert!(precheck("open", OPEN_BODY_LIMIT).is_ok());
        assert_eq!(precheck("open", OPEN_BODY_LIMIT + 1).err().map(|r| r.status), Some(413));
        assert_eq!(precheck("range", OTHER_BODY_LIMIT + 1).err().map(|r| r.status), Some(413));
        assert_eq!(precheck("../parse", 10).err().map(|r| r.status), Some(404));
    }

    #[test]
    fn the_session_is_read_the_way_the_engine_reads_it() {
        assert_eq!(requested_session(br#"{"sessionId":"a","sheetId":"b"}"#), Some("a".to_string()));
        assert_eq!(requested_session(b"not json"), None);
        assert_eq!(requested_session(br#"{"sheetId":"b"}"#), None);
    }

    #[tokio::test]
    async fn a_request_for_someone_elses_session_never_reaches_the_engine() {
        guard().claim("owner-a", "guarded-session", Instant::now());
        // Nothing listens here: reaching the engine would answer 503, not 404.
        let unreachable = "http://127.0.0.1:9";
        let reply = forward_session_op(unreachable, "owner-b", "range", br#"{"sessionId":"guarded-session"}"#.to_vec()).await;
        assert_eq!(reply.status, 404);
        let reply = forward_session_op(unreachable, "owner-a", "range", br#"{"sessionId":"guarded-session"}"#.to_vec()).await;
        assert_eq!(reply.status, 503);
        let reply = forward_session_op(unreachable, "owner-b", "close", br#"{"sessionId":"guarded-session"}"#.to_vec()).await;
        assert_eq!(reply.status, 404);
        guard().release("guarded-session");
    }
}
