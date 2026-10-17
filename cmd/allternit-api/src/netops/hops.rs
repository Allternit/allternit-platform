//! Agent hops and trace context (contract C3): W3C `traceparent`
//! (`00-<32 hex trace id>-<16 hex span id>-01`), hop receipts, the loop guard
//! and the thread timeline.
//!
//! Every bot-to-bot hand-off is one `agent_hops` row. [`admit`] decides whether
//! it may go (12 hops per bot pair per 10 minutes, depth 8, no cycle back to an
//! ancestor with the same text) and records it as `stored`, or as `held` with a
//! `loop_guard` attention item. [`advance`] moves the receipt
//! `stored → delivered → read → answered` (or `failed`). A replayed idempotency
//! key returns the earlier hop. [`timeline`] folds `bot_events` and the hops of a
//! thread's traces into a span tree; it works with OTel off.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::auth::{AuthUser, INTERNAL_SERVICE_USER_ID};
use crate::netops::alerts;
use crate::AppState;

/// Hops allowed per unordered bot pair in [`PAIR_WINDOW_MINS`].
pub const PAIR_LIMIT: i64 = 12;
pub const PAIR_WINDOW_MINS: i64 = 10;
/// Deepest hand-off chain on one trace.
pub const MAX_DEPTH: i64 = 8;

/// A parsed W3C trace context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceParent {
    pub trace_id: String,
    pub span_id: String,
    pub sampled: bool,
}

impl TraceParent {
    /// The header form, `00-<trace>-<span>-<flags>`.
    pub fn header(&self) -> String {
        format!("00-{}-{}-{}", self.trace_id, self.span_id, if self.sampled { "01" } else { "00" })
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && s.bytes().any(|b| b != b'0')
}

/// Parse a `traceparent` header (version 00, lowercase hex, non-zero ids).
pub fn parse(header: &str) -> Option<TraceParent> {
    let mut it = header.trim().split('-');
    let (ver, trace, span, flags) = (it.next()?, it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || ver != "00" || !is_hex(trace, 32) || !is_hex(span, 16) || flags.len() != 2 {
        return None;
    }
    let f = u8::from_str_radix(flags, 16).ok()?;
    Some(TraceParent { trace_id: trace.into(), span_id: span.into(), sampled: f & 1 == 1 })
}

fn new_span_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// A fresh trace.
pub fn new_trace() -> TraceParent {
    TraceParent { trace_id: uuid::Uuid::new_v4().simple().to_string(), span_id: new_span_id(), sampled: true }
}

/// A child span of `parent` (same trace, new span id).
pub fn child(parent: &TraceParent) -> TraceParent {
    TraceParent { trace_id: parent.trace_id.clone(), span_id: new_span_id(), sampled: parent.sampled }
}

fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// A stable key for a send: `sha256(parts joined)`, hex.
pub fn derive_key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

fn text_hash(text: &str) -> String {
    derive_key(&[&text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()])
}

/// The active trace of a thread, as a `traceparent` header value: the newest
/// hop that delivered into it. `None` when no hop ever did.
pub fn traceparent_for_thread(conn: &Connection, thread_id: &str) -> Option<String> {
    let (trace, span): (String, String) = conn
        .query_row(
            "SELECT trace_id, span_id FROM agent_hops WHERE thread_id = ?1 AND state NOT IN ('held', 'refused')
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![thread_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    Some(TraceParent { trace_id: trace, span_id: span, sampled: true }.header())
}

/// The trace a session's events belong to: `(trace_id, new span id, hop span)`.
/// `None` when the session's thread was never reached by a hop.
pub fn trace_for_session(conn: &Connection, session_id: &str) -> Option<(String, String, String)> {
    let thread: String = conn
        .query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1 LIMIT 1", params![session_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    let tp = parse(&traceparent_for_thread(conn, &thread)?)?;
    Some((tp.trace_id, new_span_id(), tp.span_id))
}

// ─── Admission and receipts ─────────────────────────────────────────────────

/// One hand-off asking to go.
#[derive(Debug, Clone, Default)]
pub struct HopIn {
    pub owner: String,
    /// `message_agent`, `gizzi_inbox`, `vendor_mcp` or `a2a`.
    pub via: String,
    /// `bot`, `vendor_bot` or `stranger`.
    pub from_kind: String,
    pub from_id: String,
    pub to_bot_id: String,
    pub thread_id: Option<String>,
    pub idempotency_key: Option<String>,
    /// The caller's `traceparent` header value, when it sent one.
    pub traceparent: Option<String>,
    pub text: String,
}

/// What [`admit`] decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Admit {
    /// Go ahead. `trace` is this hop's own span.
    Go { hop_id: String, trace: TraceParent, depth: i64 },
    /// The key was seen before: the earlier hop's id, state and stored result.
    Replay { hop_id: String, state: String, result: Option<Value>, thread_id: Option<String> },
    /// The loop guard stopped it. `sentence` is what the caller's tool shows.
    Held { hop_id: String, reason: String, sentence: String },
}

fn parent_of(conn: &Connection, trace_id: &str, span_id: &str) -> Option<(Option<String>, i64, String, String)> {
    conn.query_row(
        "SELECT parent_span_id, depth, to_bot_id, COALESCE(text_hash, '') FROM agent_hops WHERE trace_id = ?1 AND span_id = ?2 LIMIT 1",
        params![trace_id, span_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
    .ok()
    .flatten()
}

fn find_by_key(conn: &Connection, owner: &str, key: &str) -> Option<Admit> {
    conn.query_row(
        "SELECT id, state, result_json, thread_id FROM agent_hops WHERE owner = ?1 AND idempotency_key = ?2",
        params![owner, key],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)),
    )
    .optional()
    .ok()
    .flatten()
    .map(|(hop_id, state, result, thread_id)| Admit::Replay { hop_id, state, result: result.and_then(|s| serde_json::from_str(&s).ok()), thread_id })
}

/// The loop guard and the first receipt (`stored`). Never blocks on anything
/// but the database.
pub fn admit(conn: &Connection, hop: &HopIn) -> Admit {
    if let Some(key) = hop.idempotency_key.as_deref() {
        if let Some(prev) = find_by_key(conn, &hop.owner, key) {
            return prev;
        }
    }
    let parent = hop.traceparent.as_deref().and_then(parse);
    let hash = text_hash(&hop.text);

    // Depth and ancestry along the trace.
    let mut depth = 1;
    let mut cycle = false;
    if let Some(p) = &parent {
        let mut cursor = Some(p.span_id.clone());
        let mut seen = 0;
        while let Some(span) = cursor.take() {
            let Some((up, d, to_bot, h)) = parent_of(conn, &p.trace_id, &span) else { break };
            if seen == 0 {
                depth = d + 1;
            }
            if to_bot == hop.to_bot_id && h == hash {
                cycle = true;
            }
            seen += 1;
            if seen > 32 {
                break;
            }
            cursor = up;
        }
    }

    let since = (Utc::now() - Duration::minutes(PAIR_WINDOW_MINS)).to_rfc3339_opts(SecondsFormat::Millis, true);
    let recent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_hops WHERE owner = ?1 AND state != 'held' AND created_at >= ?4
               AND ((from_id = ?2 AND to_bot_id = ?3) OR (from_id = ?3 AND to_bot_id = ?2))",
            params![hop.owner, hop.from_id, hop.to_bot_id, since],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let held = if recent >= PAIR_LIMIT {
        Some(("pair_limit", format!("Held: these two bots have already passed {PAIR_LIMIT} messages in {PAIR_WINDOW_MINS} minutes. It may be a loop; check in before they continue.")))
    } else if depth > MAX_DEPTH {
        Some(("depth", format!("Held: this hand-off is {MAX_DEPTH} bots deep. Stop and ask the owner before going further.")))
    } else if cycle {
        Some(("cycle", "Held: this sends the same request back to a bot that already has it on this chain.".to_string()))
    } else {
        None
    };

    let trace = match &parent {
        Some(p) => child(p),
        None => new_trace(),
    };
    let hop_id = format!("hop_{}", uuid::Uuid::new_v4().simple());
    let (state, reason) = match &held {
        Some((r, _)) => ("held", Some(*r)),
        None => ("stored", None),
    };
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO agent_hops (id, owner, trace_id, span_id, parent_span_id, depth, via, from_kind, from_id, to_bot_id,
                 thread_id, idempotency_key, state, state_reason, text_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                hop_id, hop.owner, trace.trace_id, trace.span_id, parent.as_ref().map(|p| p.span_id.clone()), depth, hop.via, hop.from_kind,
                hop.from_id, hop.to_bot_id, hop.thread_id, hop.idempotency_key, state, reason, hash, now_ts()
            ],
        )
        .unwrap_or(0);
    if inserted == 0 {
        // Lost a race on the idempotency key: the other writer's hop is the answer.
        if let Some(prev) = hop.idempotency_key.as_deref().and_then(|k| find_by_key(conn, &hop.owner, k)) {
            return prev;
        }
    }
    if let Some((reason, sentence)) = held {
        let (a, b) = if hop.from_id <= hop.to_bot_id { (&hop.from_id, &hop.to_bot_id) } else { (&hop.to_bot_id, &hop.from_id) };
        alerts::open(
            conn,
            &alerts::Item {
                owner: hop.owner.clone(),
                kind: "loop_guard".into(),
                severity: "warn".into(),
                bot_id: Some(hop.to_bot_id.clone()),
                thread_id: hop.thread_id.clone(),
                source: "hop".into(),
                source_id: format!("{a}:{b}"),
                summary: sentence.clone(),
                detail: json!({ "hopId": hop_id, "reason": reason, "via": hop.via, "from": hop.from_id, "to": hop.to_bot_id, "depth": depth, "traceId": trace.trace_id }),
                provenance: None,
                retry: None,
            },
        );
        return Admit::Held { hop_id, reason: reason.to_string(), sentence };
    }
    Admit::Go { hop_id, trace, depth }
}

fn rank(state: &str) -> i32 {
    match state {
        "stored" => 1,
        "delivered" => 2,
        "read" => 3,
        "answered" => 4,
        _ => 0,
    }
}

/// Move a hop's receipt forward. `failed` can follow any state; the others only
/// move forward and never leave a terminal state. `thread_id` and `result` are
/// stored when given.
pub fn advance(conn: &Connection, hop_id: &str, state: &str, reason: Option<&str>, thread_id: Option<&str>, result: Option<&Value>) {
    let Some(cur) = conn.query_row("SELECT state FROM agent_hops WHERE id = ?1", params![hop_id], |r| r.get::<_, String>(0)).optional().ok().flatten() else {
        return;
    };
    let terminal = matches!(cur.as_str(), "answered" | "failed" | "held" | "refused");
    let ok = match state {
        "failed" | "refused" => !matches!(cur.as_str(), "held"),
        s => !terminal && rank(s) > rank(&cur),
    };
    let ts = now_ts();
    if ok {
        let col = match state {
            "delivered" => "delivered_at",
            "read" => "read_at",
            "answered" => "answered_at",
            _ => "answered_at",
        };
        let _ = conn.execute(
            &format!("UPDATE agent_hops SET state = ?2, state_reason = ?3, {col} = COALESCE({col}, ?4) WHERE id = ?1"),
            params![hop_id, state, reason, ts],
        );
    }
    if let Some(t) = thread_id {
        let _ = conn.execute("UPDATE agent_hops SET thread_id = ?2 WHERE id = ?1", params![hop_id, t]);
    }
    if let Some(r) = result {
        let _ = conn.execute("UPDATE agent_hops SET result_json = ?2 WHERE id = ?1", params![hop_id, r.to_string()]);
    }
}

/// `{state, hopId, reason?}`: what the tool result carries as `receipt`.
pub fn receipt(conn: &Connection, hop_id: &str) -> Value {
    conn.query_row("SELECT state, state_reason FROM agent_hops WHERE id = ?1", params![hop_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
        .map(|(state, reason)| json!({ "state": state, "hopId": hop_id, "reason": reason }))
        .unwrap_or_else(|_| json!({ "state": "stored", "hopId": hop_id }))
}

// ─── Timeline ───────────────────────────────────────────────────────────────

fn ms_between(a: &str, b: &str) -> Option<i64> {
    let (a, b) = (DateTime::parse_from_rfc3339(a).ok()?, DateTime::parse_from_rfc3339(b).ok()?);
    Some((b - a).num_milliseconds().max(0))
}

struct Node {
    key: String,
    parent: Option<String>,
    at: String,
    value: Value,
}

/// The span tree of a thread: its hops (and every hop on the same traces, so a
/// hand-off through three bots is one tree), its events and its model windows.
pub fn timeline(conn: &Connection, thread_id: &str) -> Value {
    let mut nodes: Vec<Node> = vec![];
    let mut traces: Vec<String> = vec![];

    if let Ok(mut st) = conn.prepare(
        "SELECT DISTINCT trace_id FROM agent_hops WHERE thread_id = ?1
         UNION SELECT DISTINCT trace_id FROM bot_events WHERE (thread_id = ?1 OR session_id IN (SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1)) AND trace_id IS NOT NULL",
    ) {
        traces = st.query_map(params![thread_id], |r| r.get::<_, String>(0)).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_default();
    }

    let mut hop_span_for_thread: Option<String> = None;
    if let Ok(mut st) = conn.prepare(
        "SELECT id, trace_id, span_id, parent_span_id, depth, via, from_kind, from_id, to_bot_id, thread_id, state, state_reason,
                created_at, answered_at, model, tokens_in, tokens_out, cost_cents
         FROM agent_hops WHERE thread_id = ?1 OR trace_id IN (SELECT DISTINCT trace_id FROM agent_hops WHERE thread_id = ?1)
         ORDER BY created_at, rowid LIMIT 500",
    ) {
        let rows = st.query_map(params![thread_id], |r| {
            let g = |i| r.get::<_, Option<String>>(i);
            Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, g(3)?, r.get::<_, i64>(4)?, r.get::<_, String>(5)?,
                r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?, g(9)?, r.get::<_, String>(10)?, g(11)?,
                r.get::<_, String>(12)?, g(13)?, g(14)?, r.get::<_, Option<i64>>(15)?, r.get::<_, Option<i64>>(16)?, r.get::<_, Option<f64>>(17)?,
            ))
        });
        for (id, trace, span, parent, depth, via, fk, fid, to, th, state, reason, created, answered, model, tin, tout, cost) in rows.map(|r| r.filter_map(Result::ok).collect::<Vec<_>>()).unwrap_or_default() {
            if th.as_deref() == Some(thread_id) {
                hop_span_for_thread = Some(span.clone());
            }
            let latency = answered.as_deref().and_then(|a| ms_between(&created, a));
            nodes.push(Node {
                key: span.clone(),
                parent: parent.clone(),
                at: created.clone(),
                value: json!({
                    "kind": "hop", "id": id, "traceId": trace, "spanId": span, "parentSpanId": parent, "depth": depth, "via": via,
                    "from": { "kind": fk, "id": fid }, "toBotId": to, "threadId": th, "state": state, "reason": reason,
                    "at": created, "latencyMs": latency, "model": model, "tokensIn": tin, "tokensOut": tout, "costCents": cost,
                }),
            });
        }
    }

    if let Ok(mut st) = conn.prepare(
        "SELECT id, event_type, occurred_at, trace_id, span_id, parent_span_id, actor_type FROM bot_events
         WHERE thread_id = ?1 OR session_id IN (SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1)
         ORDER BY occurred_at, seq LIMIT 200",
    ) {
        let rows = st.query_map(params![thread_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, String>(6)?))
        });
        for (id, ty, at, trace, span, parent, actor) in rows.map(|r| r.filter_map(Result::ok).collect::<Vec<_>>()).unwrap_or_default() {
            let attach = span.clone().or(parent.clone()).or_else(|| hop_span_for_thread.clone());
            nodes.push(Node {
                key: format!("event:{id}"),
                parent: attach,
                at: at.clone(),
                value: json!({ "kind": "event", "id": id, "eventType": ty, "actor": actor, "traceId": trace, "at": at }),
            });
        }
    }

    if let Ok(mut st) = conn.prepare("SELECT generation, model, tokens_used, started_at, ended_at FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation") {
        let rows = st.query_map(params![thread_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?))
        });
        for (generation, model, tokens, started, ended) in rows.map(|r| r.filter_map(Result::ok).collect::<Vec<_>>()).unwrap_or_default() {
            let at = started.clone().unwrap_or_default();
            let latency = match (&started, &ended) {
                (Some(a), Some(b)) => ms_between(a, b),
                _ => None,
            };
            nodes.push(Node {
                key: format!("gen:{generation}"),
                parent: hop_span_for_thread.clone(),
                at: at.clone(),
                value: json!({ "kind": "generation", "id": format!("gen-{generation}"), "generation": generation, "model": model, "tokens": tokens, "at": at, "latencyMs": latency }),
            });
        }
    }

    // Link children to parents that exist; the rest are roots.
    let known: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.key.as_str(), i)).collect();
    let mut kids: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = vec![];
    for (i, n) in nodes.iter().enumerate() {
        match n.parent.as_deref().and_then(|p| known.get(p)).copied().filter(|&p| p != i) {
            Some(p) => kids.entry(p).or_default().push(i),
            None => roots.push(i),
        }
    }
    fn render(i: usize, nodes: &[Node], kids: &HashMap<usize, Vec<usize>>, depth: usize) -> Value {
        let mut v = nodes[i].value.clone();
        let mut c = kids.get(&i).cloned().unwrap_or_default();
        c.sort_by(|a, b| nodes[*a].at.cmp(&nodes[*b].at));
        v["children"] = if depth > 40 { json!([]) } else { Value::Array(c.into_iter().map(|k| render(k, nodes, kids, depth + 1)).collect()) };
        v
    }
    roots.sort_by(|a, b| nodes[*a].at.cmp(&nodes[*b].at));
    let spans: Vec<Value> = roots.into_iter().map(|i| render(i, &nodes, &kids, 0)).collect();
    json!({ "threadId": thread_id, "traceIds": traces, "spans": spans })
}

async fn timeline_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let owner: Option<String> = conn.query_row("SELECT user_id FROM bot_threads WHERE id = ?1", params![id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        match owner {
            Some(o) if o == user.user_id || user.user_id == INTERNAL_SERVICE_USER_ID => Ok::<_, String>(Some(timeline(&conn, &id))),
            _ => Ok(None),
        }
    })
    .await;
    match res {
        Ok(Ok(Some(v))) => Json(v).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "thread not found" }))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// `GET /threads/:id/timeline`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/threads/:id/timeline", get(timeline_route))
}

/// Nothing runs in the background.
pub fn spawn(_state: Arc<AppState>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../migrations/V258__attention_items.sql").split("-- Why a run").next().unwrap()).unwrap();
        c.execute_batch(
            "CREATE TABLE runtime_user_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT, user_id TEXT, event_type TEXT, thread_id TEXT, payload TEXT, idempotency_key TEXT, occurred_at TEXT, UNIQUE(user_id, idempotency_key));
             CREATE TABLE agents (id TEXT PRIMARY KEY, name TEXT);
             CREATE TABLE bot_events (id TEXT PRIMARY KEY, bot_id TEXT, session_id TEXT, seq INTEGER, event_type TEXT, actor_type TEXT, actor_id TEXT, occurred_at TEXT, thread_id TEXT);
             CREATE TABLE bot_thread_sessions (thread_id TEXT, generation INTEGER, session_id TEXT, model TEXT, tokens_used INTEGER, started_at TEXT, ended_at TEXT);",
        )
        .unwrap();
        c.execute_batch(include_str!("../../migrations/V260__agent_hops_trace.sql")).unwrap();
        c
    }

    fn hop(from: &str, to: &str, text: &str, key: Option<&str>, tp: Option<String>) -> HopIn {
        HopIn { owner: "u".into(), via: "message_agent".into(), from_kind: "bot".into(), from_id: from.into(), to_bot_id: to.into(), thread_id: Some(format!("th-{to}")), idempotency_key: key.map(str::to_string), traceparent: tp, text: text.into() }
    }

    #[test]
    fn traceparent_parses_and_children_share_the_trace() {
        let t = new_trace();
        let p = parse(&t.header()).unwrap();
        assert_eq!(p, t);
        let c = child(&p);
        assert_eq!(c.trace_id, p.trace_id);
        assert_ne!(c.span_id, p.span_id);
        for bad in ["", "00-abc-def-01", "01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01", "00-00000000000000000000000000000000-b7ad6b7169203331-01", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331"] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn receipts_move_forward_and_failure_is_terminal() {
        let c = conn();
        let Admit::Go { hop_id, .. } = admit(&c, &hop("a", "b", "do it", None, None)) else { panic!() };
        assert_eq!(receipt(&c, &hop_id)["state"], "stored");
        advance(&c, &hop_id, "read", None, None, None);
        advance(&c, &hop_id, "delivered", None, Some("th-b"), None); // cannot go back
        assert_eq!(receipt(&c, &hop_id)["state"], "read");
        advance(&c, &hop_id, "answered", None, None, Some(&json!({"status":"done"})));
        assert_eq!(receipt(&c, &hop_id)["state"], "answered");
        let Admit::Go { hop_id: h2, .. } = admit(&c, &hop("a", "b", "another", None, None)) else { panic!() };
        advance(&c, &h2, "failed", Some("model unavailable"), None, None);
        let r = receipt(&c, &h2);
        assert_eq!((r["state"].as_str(), r["reason"].as_str()), (Some("failed"), Some("model unavailable")));
        advance(&c, &h2, "answered", None, None, None);
        assert_eq!(receipt(&c, &h2)["state"], "failed");
    }

    #[test]
    fn a_replayed_key_returns_the_earlier_hop_and_result() {
        let c = conn();
        let Admit::Go { hop_id, .. } = admit(&c, &hop("a", "b", "x", Some("k1"), None)) else { panic!() };
        advance(&c, &hop_id, "answered", None, Some("th-b"), Some(&json!({"status":"done","reply":"ok"})));
        match admit(&c, &hop("a", "b", "x", Some("k1"), None)) {
            Admit::Replay { hop_id: h, state, result, thread_id } => {
                assert_eq!((h, state.as_str(), thread_id.as_deref()), (hop_id, "answered", Some("th-b")));
                assert_eq!(result.unwrap()["reply"], "ok");
            }
            other => panic!("{other:?}"),
        }
        let n: i64 = c.query_row("SELECT COUNT(*) FROM agent_hops", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn the_13th_message_between_a_pair_is_held_and_raises_an_alert() {
        let c = conn();
        for i in 0..12 {
            let (f, t) = if i % 2 == 0 { ("a", "b") } else { ("b", "a") };
            assert!(matches!(admit(&c, &hop(f, t, &format!("m{i}"), None, None)), Admit::Go { .. }), "hop {i}");
        }
        let Admit::Held { reason, sentence, hop_id } = admit(&c, &hop("a", "b", "m13", None, None)) else { panic!("not held") };
        assert_eq!(reason, "pair_limit");
        assert!(sentence.starts_with("Held:"));
        assert_eq!(receipt(&c, &hop_id)["state"], "held");
        let open: i64 = c.query_row("SELECT COUNT(*) FROM attention_items WHERE kind = 'loop_guard' AND resolved_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(open, 1);
        // Another pair is unaffected.
        assert!(matches!(admit(&c, &hop("a", "c", "hi", None, None)), Admit::Go { .. }));
    }

    #[test]
    fn depth_is_capped_and_a_cycle_back_to_an_ancestor_is_refused() {
        let c = conn();
        let mut tp: Option<String> = None;
        for i in 0..MAX_DEPTH {
            match admit(&c, &hop(&format!("b{i}"), &format!("b{}", i + 1), &format!("step {i}"), None, tp.clone())) {
                Admit::Go { trace, depth, .. } => {
                    assert_eq!(depth, i + 1);
                    tp = Some(trace.header());
                }
                other => panic!("{other:?}"),
            }
        }
        let Admit::Held { reason, .. } = admit(&c, &hop("b8", "b9", "one more", None, tp.clone())) else { panic!("depth not enforced") };
        assert_eq!(reason, "depth");

        // Cycle: b1 gets "step 0" at depth 1; handing the same text to b1 again down the chain is refused.
        let Admit::Go { trace, .. } = admit(&c, &hop("x", "y", "same ask", None, None)) else { panic!() };
        let Admit::Go { trace: t2, .. } = admit(&c, &hop("y", "z", "different", None, Some(trace.header()))) else { panic!() };
        let Admit::Held { reason, .. } = admit(&c, &hop("z", "y", "Same  ask", None, Some(t2.header()))) else { panic!("cycle not caught") };
        assert_eq!(reason, "cycle");
    }

    #[test]
    fn timeline_is_one_tree_across_hops_with_events_and_model_windows() {
        let c = conn();
        let Admit::Go { hop_id: h1, trace: t1, .. } = admit(&c, &hop("grok", "a", "plan", None, None)) else { panic!() };
        advance(&c, &h1, "answered", None, Some("th-a"), None);
        let Admit::Go { hop_id: h2, trace: t2, .. } = admit(&c, &HopIn { thread_id: Some("th-g".into()), ..hop("a", "gizzi", "build", None, Some(t1.header())) }) else { panic!() };
        advance(&c, &h2, "answered", None, None, None);
        c.execute("UPDATE agent_hops SET model = 'm1', tokens_in = 10, tokens_out = 20 WHERE id = ?1", params![h2]).unwrap();
        c.execute("INSERT INTO bot_events (id, bot_id, seq, event_type, actor_type, actor_id, occurred_at, thread_id, trace_id, span_id) VALUES ('e1','a',1,'message.sent','bot','a','2026-01-01T00:00:00Z','th-a',?1,?2)", params![t1.trace_id, t1.span_id]).unwrap();
        c.execute("INSERT INTO bot_thread_sessions VALUES ('th-a',1,'s','m1',500,'2026-01-01T00:00:00Z','2026-01-01T00:00:02Z')", []).unwrap();

        let tl = timeline(&c, "th-a");
        assert_eq!(tl["traceIds"][0], t1.trace_id);
        let roots = tl["spans"].as_array().unwrap();
        assert_eq!(roots.len(), 1, "{tl}");
        let root = &roots[0];
        assert_eq!((root["kind"].as_str(), root["state"].as_str()), (Some("hop"), Some("answered")));
        let kinds: Vec<_> = root["children"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"hop") && kinds.contains(&"event") && kinds.contains(&"generation"), "{kinds:?}");
        let inner = root["children"].as_array().unwrap().iter().find(|k| k["kind"] == "hop").unwrap();
        assert_eq!((inner["spanId"].as_str(), inner["model"].as_str(), inner["tokensOut"].as_i64()), (Some(t2.span_id.as_str()), Some("m1"), Some(20)));
        assert!(root["latencyMs"].is_number());
        assert_eq!(traceparent_for_thread(&c, "th-a").unwrap(), TraceParent { trace_id: t1.trace_id, span_id: t1.span_id, sampled: true }.header());
        assert!(traceparent_for_thread(&c, "nope").is_none());
    }
}
