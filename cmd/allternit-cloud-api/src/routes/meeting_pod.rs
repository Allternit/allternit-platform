//! Meeting pod routes (docs/meetings/CONTRACT.md section 6), authenticated
//! with the pod token the launcher issues for one session
//! ([`issue_pod_token`]; only its sha256 is stored). A token is valid while
//! the session is open and for 5 minutes after it ends, so the pod can post
//! its last batch.
//!
//! - `POST /api/v1/meeting-sessions/:sid/ingest`: participants, transcript,
//!   chat, screen notes and the bot's state. Rows are idempotent by
//!   `idempotency_key`; screen notes are capped at 1 per 30 s per session.
//!   A `state` runs through the state machine (an illegal move is a 409 and
//!   is ignored; the rows in the batch are still kept).
//! - `POST /api/v1/meeting-sessions/:sid/tool-calls`: the in-pod agent asks
//!   to use a tool. Artifact tools (`create_doc`, `create_slides`,
//!   `create_canvas`, `update_artifact`) run here against artifacts v2.
//!   Any other tool is personal (the owner's connectors): it needs the
//!   owner's approval on the signed-in meeting page, per tool per session,
//!   re-checked on every send of the same `call_id`. Once approved it runs
//!   here, through the owner's connectors on their runtime (the signed relay
//!   to `/webhooks/mcp-edge/connectors`, the same tool union
//!   `mcp.allternit.com/mcp/connectors` serves). The result is stored per
//!   `call_id` (`meeting_tool_calls`), so a re-send returns it instead of
//!   running the tool again. No reachable runtime answers
//!   `{status:"error", error:"connectors_unavailable"}`. A denial holds for
//!   the rest of the session.
//! - Ingest also carries the end-of-meeting `summary` (written to the
//!   session, `meeting.summary` emitted) and chat `visibility`
//!   (`chat|page_only`).
//! - `GET /api/v1/meeting-sessions/:sid/control-stream`: SSE of control
//!   messages for the pod.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::meeting_events as events;
use super::meeting_state::{self as machine, EndReason, State as SessionState, Transition};
use super::meetings::{
    artifact_json, fetch_session, participant_json, segment_json, set_state, ApprovalRow, MeetingArtifactRow,
    MeetingError, ParticipantRow, Result, SegmentRow, SessionRow, APPROVAL_COLUMNS, ARTIFACT_COLUMNS,
    PARTICIPANT_COLUMNS, SEGMENT_COLUMNS,
};
use crate::ApiState;

pub(crate) const POD_TOKEN_PREFIX: &str = "mpt_";
const TOKEN_GRACE: chrono::Duration = chrono::Duration::minutes(5);
const MAX_BATCH_ROWS: usize = 500;
const MAX_TEXT_CHARS: usize = 20_000;
const SCREEN_NOTE_GAP: chrono::Duration = chrono::Duration::seconds(30);
const APPROVAL_TTL: chrono::Duration = chrono::Duration::minutes(10);
/// Below this speaker-label confidence a personal tool asks who spoke.
const CLARIFY_BELOW: f64 = 0.5;
const DEFAULT_ARTIFACT_BASE: &str = "https://ai.allternit.com/a";
/// The owner runtime's connectors MCP server, reached over the signed relay.
const CONNECTORS_RUNTIME_PATH: &str = "/webhooks/mcp-edge/connectors";
const MCP_CLIENT_HEADER: &str = "x-allternit-mcp-client";
const MCP_CLIENT: &str = "allternit-meetings";
const MAX_TOOL_REPLY_BYTES: usize = 4 * 1024 * 1024;
const MAX_DECISIONS: usize = 100;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/meeting-sessions/:sid/ingest", post(ingest))
        .route("/api/v1/meeting-sessions/:sid/tool-calls", post(tool_call))
        .route("/api/v1/meeting-sessions/:sid/control-stream", get(control_stream))
}

// ---------------------------------------------------------------------------
// Pod token
// ---------------------------------------------------------------------------

pub(crate) fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Mint a fresh pod token for a session (random 256 bits) and store its
/// hash, replacing any earlier one. The launcher passes the token to the pod.
// Called by the pod launcher (services::meeting_launcher).
pub(crate) async fn issue_pod_token(db: &PgPool, sid: Uuid) -> Result<String> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = format!("{POD_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let updated = sqlx::query("UPDATE meeting_sessions SET pod_token_hash = $2, updated_at = now() WHERE id = $1")
        .bind(sid)
        .bind(hash_token(&token))
        .execute(db)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(MeetingError::session_not_found());
    }
    Ok(token)
}

pub(crate) async fn pod_session(db: &PgPool, headers: &HeaderMap, raw_sid: &str) -> Result<SessionRow> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| t.starts_with(POD_TOKEN_PREFIX))
        .ok_or_else(MeetingError::unauthorized)?;
    let sid = Uuid::parse_str(raw_sid).map_err(|_| MeetingError::unauthorized())?;
    let matches: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM meeting_sessions WHERE id = $1 AND pod_token_hash = $2")
            .bind(sid)
            .bind(hash_token(token))
            .fetch_optional(db)
            .await?;
    if matches.is_none() {
        return Err(MeetingError::unauthorized());
    }
    let session = fetch_session(db, sid).await?.ok_or_else(MeetingError::unauthorized)?;
    if session.ended_at.is_some_and(|ended| ended + TOKEN_GRACE < Utc::now()) {
        return Err(MeetingError::unauthorized());
    }
    Ok(session)
}

fn body_object(body: &Bytes) -> Result<Map<String, Value>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(MeetingError::invalid("The body must be a JSON object.")),
    }
}

fn array<'a>(body: &'a Map<String, Value>, key: &str) -> Result<&'a [Value]> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(list)) => Ok(list.as_slice()),
        Some(_) => Err(MeetingError::invalid(format!("{key} must be an array."))),
    }
}

fn req_str<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| MeetingError::invalid(format!("{what}.{key} is required.")))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn text_field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a str> {
    let text = v
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| MeetingError::invalid(format!("{what}.{key} is required.")))?;
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(MeetingError::invalid(format!("{what}.{key} is too long.")));
    }
    Ok(text)
}

fn time_field(v: &Value, key: &str, what: &str) -> Result<Option<DateTime<Utc>>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => DateTime::parse_from_rfc3339(s)
            .map(|t| Some(t.with_timezone(&Utc)))
            .map_err(|_| MeetingError::invalid(format!("{what}.{key} must be an ISO-8601 time."))),
        Some(_) => Err(MeetingError::invalid(format!("{what}.{key} must be an ISO-8601 time."))),
    }
}

fn int_field(v: &Value, key: &str, what: &str) -> Result<i32> {
    v.get(key)
        .and_then(Value::as_i64)
        .filter(|n| *n >= 0 && *n <= i32::MAX as i64)
        .map(|n| n as i32)
        .ok_or_else(|| MeetingError::invalid(format!("{what}.{key} must be a whole number of 0 or more.")))
}

// ---------------------------------------------------------------------------
// POST ingest
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Counts {
    accepted: u32,
    duplicates: u32,
    dropped: u32,
}

impl Counts {
    fn json(&self) -> Value {
        json!({ "accepted": self.accepted, "duplicates": self.duplicates, "dropped": self.dropped })
    }
}

async fn ingest(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let session = pod_session(&state.db, &headers, &sid).await?;
    let body = body_object(&body)?;
    let participants = array(&body, "participants")?;
    let transcript = array(&body, "transcript")?;
    let chat = array(&body, "chat")?;
    let notes = array(&body, "screen_notes")?;
    if participants.len() + transcript.len() + chat.len() + notes.len() > MAX_BATCH_ROWS {
        return Err(MeetingError::invalid(format!("A batch holds at most {MAX_BATCH_ROWS} rows.")));
    }
    // Validate the requested state up front so a malformed one fails the
    // batch before anything is written.
    let requested = match body.get("state") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let to = opt_str(v, "state")
                .and_then(SessionState::parse)
                .ok_or_else(|| MeetingError::invalid("state.state is not a session state."))?;
            let reason = match v.get("end_reason") {
                None | Some(Value::Null) => None,
                Some(r) => Some(
                    r.as_str()
                        .and_then(EndReason::parse)
                        .ok_or_else(|| MeetingError::invalid("state.end_reason is not an end reason."))?,
                ),
            };
            Some((to, reason))
        }
    };
    let summary = match body.get("summary") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_summary(v)?),
    };
    let db = &state.db;
    let sid = session.id;
    let mut counts = Counts::default();

    for p in participants {
        upsert_participant(db, sid, p).await?;
    }
    if let Some(notice_at) = time_field(&Value::Object(body.clone()), "notice_at", "batch")? {
        sqlx::query(
            "UPDATE meeting_participants SET notice_at = $2 \
             WHERE session_id = $1 AND left_at IS NULL AND notice_at IS NULL",
        )
        .bind(sid)
        .bind(notice_at)
        .execute(db)
        .await?;
    }

    let mut segments = Vec::new();
    for t in transcript {
        let confidence = t.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
        if !(0.0..=1.0).contains(&confidence) {
            return Err(MeetingError::invalid("transcript.confidence must be 0 to 1."));
        }
        let source = opt_str(t, "source").unwrap_or("audio");
        if !matches!(source, "audio" | "captions") {
            return Err(MeetingError::invalid("transcript.source must be audio or captions."));
        }
        let speaker = opt_str(t, "speaker");
        let row = sqlx::query_as::<_, SegmentRow>(&format!(
            "INSERT INTO meeting_transcript_segments \
                 (session_id, seq, speaker, participant_id, confidence, source, start_ms, end_ms, text, idempotency_key) \
             VALUES ($1, $2, $3, \
                 (SELECT id FROM meeting_participants WHERE session_id = $1 AND display_name = $3 \
                  ORDER BY left_at IS NULL DESC, joined_at DESC LIMIT 1), \
                 $4, $5, $6, $7, $8, $9) \
             ON CONFLICT DO NOTHING RETURNING {SEGMENT_COLUMNS}"
        ))
        .bind(sid)
        .bind(int_field(t, "seq", "transcript")?)
        .bind(speaker)
        .bind(confidence as f32)
        .bind(source)
        .bind(int_field(t, "start_ms", "transcript")?)
        .bind(int_field(t, "end_ms", "transcript")?)
        .bind(text_field(t, "text", "transcript")?)
        .bind(req_str(t, "idempotency_key", "transcript")?)
        .fetch_optional(db)
        .await?;
        match row {
            Some(row) => {
                counts.accepted += 1;
                segments.push(row);
            }
            None => counts.duplicates += 1,
        }
    }
    if !segments.is_empty() {
        events::emit(
            sid,
            "meeting.transcript.delta",
            json!({
                "type": "meeting.transcript.delta",
                "session_id": sid,
                "segments": segments.iter().map(segment_json).collect::<Vec<_>>(),
            }),
        );
    }

    for c in chat {
        let direction = opt_str(c, "direction").unwrap_or("in");
        if !matches!(direction, "in" | "out") {
            return Err(MeetingError::invalid("chat.direction must be in or out."));
        }
        let visibility = opt_str(c, "visibility").unwrap_or("chat");
        if !matches!(visibility, "chat" | "page_only") {
            return Err(MeetingError::invalid("chat.visibility must be chat or page_only."));
        }
        let at = time_field(c, "at", "chat")?.unwrap_or_else(Utc::now);
        let row: Option<(Uuid, String, String, String, DateTime<Utc>)> = sqlx::query_as(
            "INSERT INTO meeting_chat_messages (session_id, direction, sender, text, platform_message_id, idempotency_key, at, visibility) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING \
             RETURNING id, direction, sender, text, at",
        )
        .bind(sid)
        .bind(direction)
        .bind(req_str(c, "sender", "chat")?)
        .bind(text_field(c, "text", "chat")?)
        .bind(opt_str(c, "platform_message_id"))
        .bind(req_str(c, "idempotency_key", "chat")?)
        .bind(at)
        .bind(visibility)
        .fetch_optional(db)
        .await?;
        let Some((id, direction, sender, text, at)) = row else {
            counts.duplicates += 1;
            continue;
        };
        counts.accepted += 1;
        let at = super::meetings::ts(at);
        events::emit(
            sid,
            "meeting.chat.message",
            json!({
                "type": "meeting.chat.message",
                "session_id": sid,
                "message": {
                    "id": id, "session_id": sid, "direction": direction, "sender": sender,
                    "text": text, "at": at, "visibility": visibility,
                },
            }),
        );
        if direction == "out" {
            events::emit(
                sid,
                "meeting.reply",
                json!({
                    "type": "meeting.reply", "session_id": sid, "channel": "chat",
                    "text": text, "at": at, "visibility": visibility,
                }),
            );
        }
    }

    // Screen notes: duplicates first, then at most one note per 30 s.
    let mut last_note: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MAX(created_at) FROM meeting_screen_notes WHERE session_id = $1")
            .bind(sid)
            .fetch_one(db)
            .await?;
    for n in notes {
        let key = req_str(n, "idempotency_key", "screen_notes")?;
        let at_ms = int_field(n, "at_ms", "screen_notes")?;
        let description = text_field(n, "description", "screen_notes")?;
        let seen: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM meeting_screen_notes WHERE session_id = $1 AND idempotency_key = $2")
                .bind(sid)
                .bind(key)
                .fetch_optional(db)
                .await?;
        if seen.is_some() {
            counts.duplicates += 1;
            continue;
        }
        let now = Utc::now();
        if last_note.is_some_and(|last| now - last < SCREEN_NOTE_GAP) {
            counts.dropped += 1;
            continue;
        }
        let inserted = sqlx::query(
            "INSERT INTO meeting_screen_notes (session_id, at_ms, description, idempotency_key, created_at) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(sid)
        .bind(at_ms)
        .bind(description)
        .bind(key)
        .bind(now)
        .execute(db)
        .await?
        .rows_affected();
        if inserted == 1 {
            counts.accepted += 1;
            last_note = Some(now);
        } else {
            counts.duplicates += 1;
        }
    }

    if let Some((text, decisions, action_items)) = summary {
        sqlx::query(
            "UPDATE meeting_sessions SET summary = $2, decisions = $3, action_items = $4, updated_at = now() WHERE id = $1",
        )
        .bind(sid)
        .bind(&text)
        .bind(&decisions)
        .bind(&action_items)
        .execute(db)
        .await?;
        events::emit(
            sid,
            "meeting.summary",
            json!({
                "type": "meeting.summary",
                "session_id": sid,
                "summary": text,
                "decisions": decisions,
                "action_items": action_items,
            }),
        );
    }

    if let Some((to, reason)) = requested {
        // Re-read: a user control may have moved the session since auth.
        let current = fetch_session(db, sid).await?.ok_or_else(MeetingError::session_not_found)?.state();
        match machine::transition(current, to, reason) {
            Ok(Transition::Unchanged) => {}
            Ok(Transition::To(next)) => {
                set_state(db, sid, current, next, reason).await?;
            }
            Err(error) => {
                let mut err = MeetingError::conflict(error.message());
                if let Value::Object(map) = counts.json() {
                    err.extra = map;
                }
                return Err(err);
            }
        }
    }
    Ok(Json(counts.json()))
}

/// `{summary, decisions: string[], action_items: [{text, owner, due}]}`,
/// normalised to the stored shape.
fn parse_summary(v: &Value) -> Result<(String, Value, Value)> {
    let text = v
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| t.chars().count() <= MAX_TEXT_CHARS)
        .ok_or_else(|| MeetingError::invalid("summary.summary must be text."))?
        .to_string();
    let decisions: Vec<Value> = match v.get("decisions") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) if list.len() <= MAX_DECISIONS => list
            .iter()
            .map(|d| {
                d.as_str()
                    .map(|s| Value::String(s.trim().to_string()))
                    .ok_or_else(|| MeetingError::invalid("summary.decisions must be strings."))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(MeetingError::invalid("summary.decisions must be a list of at most 100 strings.")),
    };
    let items: Vec<Value> = match v.get("action_items") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) if list.len() <= MAX_DECISIONS => list
            .iter()
            .map(|item| {
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| MeetingError::invalid("summary.action_items[].text is required."))?;
                let opt = |k: &str| item.get(k).and_then(Value::as_str).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null);
                Ok(json!({ "text": text.trim(), "owner": opt("owner"), "due": opt("due") }))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(MeetingError::invalid("summary.action_items must be a list of at most 100 items.")),
    };
    Ok((text, Value::Array(decisions), Value::Array(items)))
}

async fn upsert_participant(db: &PgPool, sid: Uuid, p: &Value) -> Result<()> {
    let pid = req_str(p, "platform_participant_id", "participants")?;
    let name = req_str(p, "display_name", "participants")?;
    let class = opt_str(p, "class").unwrap_or("guest");
    if !matches!(class, "org_member" | "guest") {
        return Err(MeetingError::invalid("participants.class must be org_member or guest."));
    }
    let joined_at = time_field(p, "joined_at", "participants")?.unwrap_or_else(Utc::now);
    let left_at = time_field(p, "left_at", "participants")?;
    let before = sqlx::query_as::<_, ParticipantRow>(&format!(
        "SELECT {PARTICIPANT_COLUMNS} FROM meeting_participants WHERE session_id = $1 AND platform_participant_id = $2"
    ))
    .bind(sid)
    .bind(pid)
    .fetch_optional(db)
    .await?;
    let after = sqlx::query_as::<_, ParticipantRow>(&format!(
        "INSERT INTO meeting_participants (session_id, platform_participant_id, display_name, email, class, joined_at, left_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (session_id, platform_participant_id) DO UPDATE SET \
             display_name = EXCLUDED.display_name, \
             email = COALESCE(EXCLUDED.email, meeting_participants.email), \
             class = EXCLUDED.class, \
             left_at = EXCLUDED.left_at \
         RETURNING {PARTICIPANT_COLUMNS}"
    ))
    .bind(sid)
    .bind(pid)
    .bind(name)
    .bind(opt_str(p, "email"))
    .bind(class)
    .bind(joined_at)
    .bind(left_at)
    .fetch_one(db)
    .await?;
    let change = match &before {
        None => Some("joined"),
        Some(b) if b.left_at.is_none() && after.left_at.is_some() => Some("left"),
        Some(b) if b.display_name != after.display_name || b.class != after.class || b.email != after.email
            || b.left_at != after.left_at =>
        {
            Some("updated")
        }
        _ => None,
    };
    if let Some(change) = change {
        events::emit(
            sid,
            "meeting.participant.changed",
            json!({
                "type": "meeting.participant.changed",
                "session_id": sid,
                "participant": participant_json(&after),
                "change": change,
            }),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// POST tool-calls
// ---------------------------------------------------------------------------

fn tool_kind(tool: &str) -> Option<&'static str> {
    match tool {
        "create_doc" => Some("doc"),
        "create_slides" => Some("slides"),
        "create_canvas" => Some("canvas"),
        _ => None,
    }
}

fn valid_tool_name(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 128
        && tool.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn args_summary(tool: &str, args: &Value) -> String {
    if let Some(summary) = args.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        return summary.trim().chars().take(300).collect();
    }
    let compact = args.to_string();
    let compact: String = compact.chars().take(200).collect();
    format!("{tool} {compact}")
}

async fn tool_call(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let session = pod_session(&state.db, &headers, &sid).await?;
    if session.state() == SessionState::Ended {
        return Err(MeetingError::not_active());
    }
    let body = Value::Object(body_object(&body)?);
    let call_id = req_str(&body, "call_id", "tool_call")?.trim().to_string();
    if call_id.len() > 128 {
        return Err(MeetingError::invalid("call_id is at most 128 characters."));
    }
    let tool = req_str(&body, "tool", "tool_call")?.trim().to_string();
    if !valid_tool_name(&tool) {
        return Err(MeetingError::invalid("tool is not a valid tool name."));
    }
    let args = match body.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(_) => return Err(MeetingError::invalid("args must be an object.")),
    };
    let asked_by = opt_str(&body, "asked_by").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let confidence = body.get("speaker_confidence").and_then(Value::as_f64);

    let db = &state.db;
    // A call that already ran returns its stored result (idempotent per call_id).
    let stored: Option<(String, String, Option<Value>)> =
        sqlx::query_as("SELECT tool, status, result FROM meeting_tool_calls WHERE session_id = $1 AND call_id = $2")
            .bind(session.id)
            .bind(&call_id)
            .fetch_optional(db)
            .await?;
    if let Some((stored_tool, status, result)) = stored {
        if stored_tool != tool {
            return Err(MeetingError::invalid("call_id was already used for another tool."));
        }
        return Ok(Json(match (status.as_str(), result) {
            ("running", _) | (_, None) => json!({ "status": "error", "error": "in_progress" }),
            (_, Some(result)) => result,
        }));
    }
    if let Some(kind) = tool_kind(&tool) {
        return create_artifact(db, &session, &call_id, kind, &args).await.map(Json);
    }
    if tool == "update_artifact" {
        return update_artifact(db, &session, &call_id, &args).await.map(Json);
    }

    // A personal tool. An unclear speaker is asked first.
    if confidence.is_some_and(|c| c < CLARIFY_BELOW) {
        return Ok(Json(json!({
            "status": "clarify",
            "question": "I'm not sure who asked. Say your name and ask again, and I'll check with the meeting owner.",
        })));
    }
    let latest = sqlx::query_as::<_, ApprovalRow>(&format!(
        "SELECT {APPROVAL_COLUMNS} FROM meeting_tool_approvals WHERE session_id = $1 AND tool = $2 \
         ORDER BY (status = 'approved') DESC, requested_at DESC LIMIT 1"
    ))
    .bind(session.id)
    .bind(&tool)
    .fetch_optional(db)
    .await?;
    if let Some(row) = &latest {
        match row.status.as_str() {
            "approved" => return run_connector_tool(&state, &session, &call_id, &tool, &args).await.map(Json),
            "denied" => return Ok(Json(json!({ "status": "denied" }))),
            "pending" if row.expires_at > Utc::now() => {
                return Ok(Json(json!({ "status": "approval_required", "approval_id": row.id })))
            }
            "pending" => {
                // Expired unanswered: the waiting call ends as denied; the next
                // call asks again.
                sqlx::query(
                    "UPDATE meeting_tool_approvals SET status = 'expired', resolved_at = now() \
                     WHERE id = $1 AND status = 'pending'",
                )
                .bind(row.id)
                .execute(db)
                .await?;
                return Ok(Json(json!({ "status": "denied" })));
            }
            _ => {}
        }
    }
    let now = Utc::now();
    let row = sqlx::query_as::<_, ApprovalRow>(&format!(
        "INSERT INTO meeting_tool_approvals (session_id, user_id, tool, args, args_summary, asked_by, requested_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {APPROVAL_COLUMNS}"
    ))
    .bind(session.id)
    .bind(&session.user_id)
    .bind(&tool)
    .bind(&args)
    .bind(args_summary(&tool, &args))
    .bind(asked_by.as_deref())
    .bind(now)
    .bind(now + APPROVAL_TTL)
    .fetch_one(db)
    .await?;
    events::emit(
        session.id,
        "meeting.approval.requested",
        json!({
            "type": "meeting.approval.requested",
            "session_id": session.id,
            "approval": super::meetings::approval_json(&row),
        }),
    );
    Ok(Json(json!({ "status": "approval_required", "approval_id": row.id })))
}

/// Run an approved personal tool through the owner's connectors: claim the
/// call, relay one MCP `tools/call` to the owner's runtimes (connected ones
/// first), store the result. With no runtime reachable the claim is dropped
/// and the pod gets `connectors_unavailable` (it may retry later).
async fn run_connector_tool(state: &ApiState, session: &SessionRow, call_id: &str, tool: &str, args: &Value) -> Result<Value> {
    let db = &state.db;
    let claimed = sqlx::query(
        "INSERT INTO meeting_tool_calls (session_id, call_id, tool, status) VALUES ($1, $2, $3, 'running') \
         ON CONFLICT (session_id, call_id) DO NOTHING",
    )
    .bind(session.id)
    .bind(call_id)
    .bind(tool)
    .execute(db)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(json!({ "status": "error", "error": "in_progress" }));
    }
    let outcome = call_owner_connectors(state, &session.user_id, call_id, tool, args).await;
    let Some(result) = outcome else {
        sqlx::query("DELETE FROM meeting_tool_calls WHERE session_id = $1 AND call_id = $2 AND status = 'running'")
            .bind(session.id)
            .bind(call_id)
            .execute(db)
            .await?;
        return Ok(json!({ "status": "error", "error": "connectors_unavailable" }));
    };
    let status = if result["status"] == "ok" { "ok" } else { "error" };
    sqlx::query(
        "UPDATE meeting_tool_calls SET status = $3, result = $4, updated_at = now() WHERE session_id = $1 AND call_id = $2",
    )
    .bind(session.id)
    .bind(call_id)
    .bind(status)
    .bind(&result)
    .execute(db)
    .await?;
    Ok(result)
}

/// One JSON-RPC `tools/call` to the owner's connectors MCP server on each of
/// their runtimes in turn. `None` when no runtime answered.
async fn call_owner_connectors(state: &ApiState, owner: &str, call_id: &str, tool: &str, args: &Value) -> Option<Value> {
    use base64::engine::general_purpose::STANDARD;
    use std::collections::HashMap;

    let runtimes: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL \
         AND credential_expires_at > CURRENT_TIMESTAMP ORDER BY last_seen_at DESC NULLS LAST, created_at DESC",
    )
    .bind(owner)
    .fetch_all(&state.db)
    .await
    .ok()?;
    if runtimes.is_empty() {
        return None;
    }
    let connected = super::runtime_relay::connected_runtime_ids().await;
    let (mut ordered, asleep): (Vec<String>, Vec<String>) = runtimes.into_iter().partition(|id| connected.contains(id));
    ordered.extend(asleep);
    let body = json!({
        "jsonrpc": "2.0",
        "id": call_id,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    for runtime_id in ordered {
        let response = super::runtime_relay::relay_signed_request_to_runtime_with(
            &state.db,
            &state.contabo_runtime_service,
            &state.quota_service,
            &state.provisioning_service,
            owner,
            &runtime_id,
            super::runtime_relay::RelayRequest {
                method: "POST".to_string(),
                path: CONNECTORS_RUNTIME_PATH.to_string(),
                headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
                body: STANDARD.encode(&body),
                body_encoding: "base64".to_string(),
            },
            &["content-type"],
            HashMap::from([(MCP_CLIENT_HEADER.to_string(), MCP_CLIENT.to_string())]),
        )
        .await;
        let Ok(response) = response else { continue };
        let status = response.status();
        let Ok(bytes) = axum::body::to_bytes(response.into_body(), MAX_TOOL_REPLY_BYTES).await else { continue };
        if !status.is_success() {
            // Offline / warming / not this runtime: try the next one.
            tracing::info!(%runtime_id, %status, "meetings: connectors relay did not answer");
            continue;
        }
        let Ok(reply) = serde_json::from_slice::<Value>(&bytes) else { continue };
        return Some(tool_result_from_rpc(&reply));
    }
    None
}

/// Map a JSON-RPC `tools/call` reply to a ToolCallResult.
fn tool_result_from_rpc(reply: &Value) -> Value {
    if let Some(error) = reply.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("The tool failed.");
        return json!({ "status": "error", "error": "tool_failed", "message": message });
    }
    let result = reply.get("result").cloned().unwrap_or(Value::Null);
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return json!({ "status": "error", "error": "tool_failed", "result": result });
    }
    json!({ "status": "ok", "result": result })
}

fn artifact_url(id: &str) -> String {
    let base = std::env::var("ARTIFACTS_PUBLIC_BASE_URL")
        .ok()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_ARTIFACT_BASE.to_string());
    format!("{base}/{id}")
}

/// The artifacts v2 kind and body format for a meeting artifact kind.
fn store_kind(kind: &str) -> &'static str {
    match kind {
        "doc" => "doc",
        "slides" => "slides",
        _ => "design",
    }
}

fn body_and_format<'a>(args: &'a Value, store: &str) -> Result<(&'a str, String)> {
    let body = args.get("body").and_then(Value::as_str).unwrap_or("");
    if body.len() > crate::artifacts::MAX_BODY_BYTES {
        return Err(MeetingError::invalid("body is too large."));
    }
    let spec = crate::artifacts::kinds::spec(store).expect("meeting artifact kinds exist");
    let format = match args.get("body_format").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) if spec.body_formats.contains(&f) => f.to_string(),
        Some(_) => return Err(MeetingError::invalid(format!("body_format must be one of {}.", spec.body_formats.join(", ")))),
        // A doc the agent writes in a call is Markdown unless it says otherwise.
        None if store == "doc" => "text/markdown".to_string(),
        None => spec.default_body_format.to_string(),
    };
    Ok((body, format))
}

async fn artifacts_allowed(db: &PgPool, org: Option<&str>) -> Result<bool> {
    let Some(org) = org else { return Ok(true) };
    match super::artifacts_v2::org_settings(db, org).await {
        Ok(settings) => Ok(settings.enabled),
        Err(_) => Err(MeetingError::internal()),
    }
}

async fn create_artifact(db: &PgPool, session: &SessionRow, call_id: &str, kind: &str, args: &Value) -> Result<Value> {
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.chars().count() <= 300)
        .ok_or_else(|| MeetingError::invalid("args.title is required (at most 300 characters)."))?;
    if !artifacts_allowed(db, session.org_id.as_deref()).await? {
        return Ok(json!({ "status": "denied" }));
    }
    let store = store_kind(kind);
    let (body, format) = body_and_format(args, store)?;
    // Same call, same artifact: the id is derived from (session, call_id).
    let digest = hex::encode(Sha256::digest(format!("{}:{call_id}", session.id).as_bytes()));
    let artifact_id = format!("art_mtg_{}", &digest[..24]);
    let url = artifact_url(&artifact_id);

    let mut tx = db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO artifacts (id, owner_id, org_id, kind, runtime_version, title, origin, current_version) \
         VALUES ($1, $2, $3, $4, 2, $5, $6, 1) ON CONFLICT (id) DO NOTHING",
    )
    .bind(&artifact_id)
    .bind(&session.user_id)
    .bind(session.org_id.as_deref())
    .bind(store)
    .bind(title)
    .bind(json!({ "surface": "meeting", "session_id": session.id, "meeting_id": session.meeting_id }))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 1 {
        sqlx::query(
            "INSERT INTO artifact_versions (artifact_id, version, body, body_format, meta, size_bytes, sha256, author_id) \
             VALUES ($1, 1, $2, $3, $4, $5, $6, 'assistant')",
        )
        .bind(&artifact_id)
        .bind(body)
        .bind(&format)
        .bind(json!({ "meeting_call_id": call_id }))
        .bind(body.len() as i32)
        .bind(hex::encode(Sha256::digest(body.as_bytes())))
        .execute(&mut *tx)
        .await?;
    }
    let row = sqlx::query_as::<_, MeetingArtifactRow>(&format!(
        "INSERT INTO meeting_artifacts (session_id, artifact_id, kind, title, url, version) \
         VALUES ($1, $2, $3, $4, $5, 1) \
         ON CONFLICT (session_id, artifact_id) DO UPDATE SET session_id = EXCLUDED.session_id \
         RETURNING {ARTIFACT_COLUMNS}"
    ))
    .bind(session.id)
    .bind(&artifact_id)
    .bind(kind)
    .bind(title)
    .bind(&url)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let payload = artifact_json(&row);
    if inserted == 1 {
        events::emit(
            session.id,
            "meeting.artifact.created",
            json!({ "type": "meeting.artifact.created", "session_id": session.id, "artifact": payload }),
        );
    }
    Ok(json!({ "status": "ok", "result": { "artifact": payload } }))
}

async fn update_artifact(db: &PgPool, session: &SessionRow, call_id: &str, args: &Value) -> Result<Value> {
    let artifact_id = args
        .get("artifact_id")
        .and_then(Value::as_str)
        .ok_or_else(|| MeetingError::invalid("args.artifact_id is required."))?;
    let existing = sqlx::query_as::<_, MeetingArtifactRow>(&format!(
        "SELECT {ARTIFACT_COLUMNS} FROM meeting_artifacts WHERE session_id = $1 AND artifact_id = $2"
    ))
    .bind(session.id)
    .bind(artifact_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| MeetingError::invalid("args.artifact_id is not an artifact from this meeting."))?;
    let store = store_kind(&existing.kind);
    let (body, format) = body_and_format(args, store)?;
    let title = match args.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) if t.chars().count() <= 300 => Some(t.to_string()),
        Some(_) => return Err(MeetingError::invalid("args.title is at most 300 characters.")),
        None => None,
    };

    let mut tx = db.begin().await?;
    let current: Option<i32> = sqlx::query_scalar("SELECT current_version FROM artifacts WHERE id = $1 FOR UPDATE")
        .bind(artifact_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(current) = current else {
        return Err(MeetingError::invalid("That artifact was deleted."));
    };
    let done: Option<i32> = sqlx::query_scalar(
        "SELECT version FROM artifact_versions WHERE artifact_id = $1 AND meta->>'meeting_call_id' = $2",
    )
    .bind(artifact_id)
    .bind(call_id)
    .fetch_optional(&mut *tx)
    .await?;
    let changed = done.is_none();
    if changed {
        let next = current + 1;
        sqlx::query(
            "INSERT INTO artifact_versions (artifact_id, version, body, body_format, meta, size_bytes, sha256, author_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'assistant')",
        )
        .bind(artifact_id)
        .bind(next)
        .bind(body)
        .bind(&format)
        .bind(json!({ "meeting_call_id": call_id }))
        .bind(body.len() as i32)
        .bind(hex::encode(Sha256::digest(body.as_bytes())))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE artifacts SET current_version = $2, title = COALESCE($3, title), updated_at = now() WHERE id = $1",
        )
        .bind(artifact_id)
        .bind(next)
        .bind(title.as_deref())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE meeting_artifacts SET version = $3, title = COALESCE($4, title), updated_at = now() \
             WHERE session_id = $1 AND artifact_id = $2",
        )
        .bind(session.id)
        .bind(artifact_id)
        .bind(next)
        .bind(title.as_deref())
        .execute(&mut *tx)
        .await?;
    }
    let row = sqlx::query_as::<_, MeetingArtifactRow>(&format!(
        "SELECT {ARTIFACT_COLUMNS} FROM meeting_artifacts WHERE session_id = $1 AND artifact_id = $2"
    ))
    .bind(session.id)
    .bind(artifact_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let payload = artifact_json(&row);
    if changed {
        events::emit(
            session.id,
            "meeting.artifact.updated",
            json!({ "type": "meeting.artifact.updated", "session_id": session.id, "artifact": payload }),
        );
    }
    Ok(json!({ "status": "ok", "result": { "artifact": payload } }))
}

// ---------------------------------------------------------------------------
// GET control-stream (SSE)
// ---------------------------------------------------------------------------

async fn control_stream(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Result<Response> {
    use futures::stream::{self, StreamExt};
    use tokio_stream::wrappers::BroadcastStream;

    let session = pod_session(&state.db, &headers, &sid).await?;
    let rx = events::subscribe_control(session.id);
    let guest_quiet = match session.org_id.as_deref() {
        Some(org) => super::org_meeting_settings::load_org_settings(&state.db, org).await?.guest_quiet,
        None => false,
    };
    // Where things stand, so a pod that (re)connects late starts in sync.
    let mut initial = vec![
        json!({ "type": "set_speak_replies", "on": session.speak_replies }),
        json!({ "type": "set_guest_quiet", "on": guest_quiet }),
    ];
    match session.state() {
        SessionState::Paused => initial.push(json!({ "type": "stop_listening" })),
        SessionState::Ended => initial.push(json!({ "type": "leave" })),
        _ => {}
    }
    let to_event = |message: Value| {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("control").to_string();
        SseEvent::default().event(kind).data(message.to_string())
    };
    let live = BroadcastStream::new(rx).filter_map(move |item| async move { item.ok().map(to_event) });
    let stream = stream::iter(initial.into_iter().map(to_event))
        .chain(live)
        .map(Ok::<_, std::convert::Infallible>);
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20)).text("ping"))
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_replies_map_to_tool_call_results() {
        let ok = tool_result_from_rpc(&json!({ "jsonrpc": "2.0", "id": "c1", "result": { "content": [], "isError": false } }));
        assert_eq!(ok["status"], "ok");
        assert_eq!(ok["result"]["isError"], false);
        let tool_error = tool_result_from_rpc(&json!({ "result": { "content": [{ "type": "text", "text": "no" }], "isError": true } }));
        assert_eq!((tool_error["status"].as_str(), tool_error["error"].as_str()), (Some("error"), Some("tool_failed")));
        let rpc_error = tool_result_from_rpc(&json!({ "error": { "code": -32602, "message": "Unknown tool" } }));
        assert_eq!(rpc_error, json!({ "status": "error", "error": "tool_failed", "message": "Unknown tool" }));
    }

    #[test]
    fn summaries_normalise_and_reject_bad_shapes() {
        let (text, decisions, items) = parse_summary(&json!({
            "summary": " Done. ", "decisions": [" a "], "action_items": [{ "text": "x", "due": "Fri" }]
        }))
        .unwrap();
        assert_eq!((text.as_str(), decisions), ("Done.", json!(["a"])));
        assert_eq!(items, json!([{ "text": "x", "owner": null, "due": "Fri" }]));
        for bad in [json!({}), json!({ "summary": "s", "decisions": [1] }), json!({ "summary": "s", "action_items": [{}] })] {
            assert!(parse_summary(&bad).is_err(), "{bad}");
        }
    }
}
