//! Allternit Meetings user routes (docs/meetings/CONTRACT.md section 4).
//!
//! Gizzi joins Meet, Zoom and Teams calls as a bot participant. This module
//! serves the signed-in user's side: meetings (pasted links and calendar
//! rows), bot sessions (one per `url_key` at a time; later requesters
//! attach), the transcript and artifacts, the SSE event stream, live
//! controls and tool approvals. Settings and usage live in
//! `org_meeting_settings`, the pod-token routes in `meeting_pod`.
//!
//! Every handler authenticates itself (Clerk session, a `compute`-scoped
//! API token, or a paired device's runtime token acting as its owner, like
//! artifacts v2), so the module is merged into the public router. Errors are
//! `{ "error": "<code>", "message": "<text>" }` (types.ts `MeetingApiError`).
//! Free plans get `402 meetings_requires_paid_plan` on every route here.
//!
//! All paths sit under `/api/v1/meetings` and `/api/v1/meeting-sessions`,
//! prefixes no other module serves (an overlapping route once crash-looped
//! prod: axum panics at router build).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{json, Map, Value};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::meeting_events as events;
use super::meeting_links::{self, AutoJoinRule, Platform};
use super::meeting_state::{self as machine, EndReason, State as SessionState};
use super::org_meeting_settings::{self as settings, OrgSettings};
use crate::{auth, ApiError, ApiState};

pub(crate) const PAGE_BASE: &str = "https://ai.allternit.com/meetings";
pub(crate) const BOT_IDENTITY: &str = "Gizzi (Allternit)";
const MAX_TITLE_CHARS: usize = 300;
const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 200;
const DEFAULT_TRANSCRIPT_LIMIT: i64 = 500;
const MAX_TRANSCRIPT_LIMIT: i64 = 2000;
const MAX_SPEAK_CHARS: usize = 2000;
/// Bot joins two minutes before the start (`when = start`).
pub(crate) const JOIN_LEAD: chrono::Duration = chrono::Duration::minutes(2);
/// Sessions holding a pod across the whole service before `429 no_capacity`
/// (placeholder until Eoj gate 5; `MEETINGS_GLOBAL_CONCURRENT_CAP`).
const DEFAULT_GLOBAL_CAP: i64 = 20;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/meetings", get(list_meetings).post(create_meeting))
        .route(
            "/api/v1/meetings/:id",
            get(get_meeting).patch(patch_meeting).delete(delete_meeting),
        )
        .route("/api/v1/meetings/:id/sessions", post(create_session))
        .route("/api/v1/meeting-sessions/:sid", get(get_session).patch(patch_session))
        .route("/api/v1/meeting-sessions/:sid/transcript", get(get_transcript))
        .route("/api/v1/meeting-sessions/:sid/artifacts", get(list_artifacts))
        .route("/api/v1/meeting-sessions/:sid/events", get(session_events))
        .route("/api/v1/meeting-sessions/:sid/control", post(control_session))
        .route("/api/v1/meeting-sessions/:sid/approvals/:aid", post(decide_approval))
        .merge(settings::routes())
        .merge(super::meeting_pod::routes())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct MeetingError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub extra: Map<String, Value>,
}

pub(crate) type Result<T> = std::result::Result<T, MeetingError>;

impl MeetingError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        MeetingError { status, code, message: message.into(), extra: Map::new() }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn unsupported_link() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported_link",
            "That isn't a Google Meet, Zoom or Microsoft Teams meeting link.",
        )
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in to Allternit.")
    }
    pub fn paid_plan() -> Self {
        Self::new(
            StatusCode::PAYMENT_REQUIRED,
            "meetings_requires_paid_plan",
            "Meetings is part of the Plus, Super and Ultra plans.",
        )
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn meeting_not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "meeting_not_found", "Meeting not found.")
    }
    pub fn session_not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "session_not_found", "Meeting session not found.")
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
    pub fn not_active() -> Self {
        Self::new(StatusCode::CONFLICT, "session_not_active", "Gizzi is no longer in this meeting.")
    }
    pub fn no_capacity(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "no_capacity", message)
    }
    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong; try again.")
    }
}

impl From<ApiError> for MeetingError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::Unauthorized(_)
            | ApiError::InvalidToken(_)
            | ApiError::TokenExpired(_)
            | ApiError::InvalidCredentials(_) => MeetingError::unauthorized(),
            ApiError::Forbidden(message) => MeetingError::forbidden(message),
            ApiError::BadRequest(message) | ApiError::ValidationError(message) => MeetingError::invalid(message),
            ApiError::ServiceUnavailable(message) => {
                MeetingError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
            }
            other => {
                tracing::error!("meetings: {other}");
                MeetingError::internal()
            }
        }
    }
}

impl From<sqlx::Error> for MeetingError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!("meetings: database error: {error}");
        MeetingError::internal()
    }
}

impl IntoResponse for MeetingError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.code, "message": self.message });
        if let Value::Object(map) = &mut body {
            map.extend(self.extra);
        }
        (self.status, Json(body)).into_response()
    }
}

pub(crate) fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

// ---------------------------------------------------------------------------
// Caller, plan gate, org gate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct Caller {
    pub id: String,
    pub org_id: Option<String>,
    pub org_role: Option<String>,
}

impl Caller {
    /// Clerk org admin: `admin` (session token v2) or `org:admin` (v1).
    pub fn is_org_admin(&self) -> bool {
        matches!(self.org_role.as_deref(), Some("admin") | Some("org:admin"))
    }

    /// The caller's role in `org`, when that is the caller's active org.
    pub fn role_in(&self, org: Option<&str>) -> Option<&str> {
        match (org, self.org_id.as_deref()) {
            (Some(o), Some(mine)) if o == mine => self.org_role.as_deref(),
            _ => None,
        }
    }
}

pub(crate) async fn caller(state: &ApiState, headers: &HeaderMap) -> Result<Caller> {
    // Tests stand in for Clerk sessions with this header; compiled out of
    // every non-test build (same seam as artifacts v2).
    #[cfg(test)]
    if let Some(raw) = headers.get("x-test-caller").and_then(|v| v.to_str().ok()) {
        let v: Value = serde_json::from_str(raw).expect("x-test-caller is JSON");
        let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        return Ok(Caller {
            id: field("id").expect("x-test-caller.id"),
            org_id: field("org_id"),
            org_role: field("org_role"),
        });
    }
    match auth::resolve_user_scoped(&state.db, headers, "compute").await {
        Ok(user) => Ok(Caller { id: user.id, org_id: user.organization_id, org_role: user.org_role }),
        // A paired device's `gizzi login` token acts as its owner, with no
        // org (the `gizzi meetings` command uses it).
        Err(primary) => {
            let Some(token) = super::runtime_pairing::device_token_from_headers(headers) else {
                return Err(primary.into());
            };
            let device = super::runtime_pairing::runtime_device_for_token(&state.db, token, None)
                .await
                .map_err(|_| MeetingError::unauthorized())?;
            Ok(Caller { id: device.user_id, ..Caller::default() })
        }
    }
}

/// `402 meetings_requires_paid_plan` unless the caller is on Plus, Super or
/// Ultra. Returns the plan id.
pub(crate) async fn require_paid(db: &PgPool, caller: &Caller) -> Result<String> {
    let plan = crate::services::voice_usage::plan_for_user(db, &caller.id).await?;
    if plan == "free" {
        return Err(MeetingError::paid_plan());
    }
    Ok(plan)
}

/// Org settings for `org` (None for individual accounts): `403
/// meetings_disabled_by_org` when the org turned Meetings off, `403
/// forbidden` when the caller's role is not in the allowed roles. A caller
/// whose role is unknown (device token) is held to `enabled` only.
pub(crate) async fn org_gate(db: &PgPool, org: Option<&str>, role: Option<&str>) -> Result<Option<OrgSettings>> {
    let Some(org) = org else { return Ok(None) };
    let org_settings = settings::load_org_settings(db, org).await?;
    if !org_settings.enabled {
        return Err(MeetingError::new(
            StatusCode::FORBIDDEN,
            "meetings_disabled_by_org",
            "Your organization has turned off Meetings.",
        ));
    }
    if let Some(role) = role {
        let role = settings::normalise_role(role);
        if !org_settings.roles.contains(&role) {
            return Err(MeetingError::forbidden(format!(
                "Your organization allows Meetings only for these roles: {}.",
                org_settings.roles.join(", ")
            )));
        }
    }
    Ok(Some(org_settings))
}

fn parse_body(body: &Bytes) -> Result<Map<String, Value>> {
    if body.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(MeetingError::invalid("The body must be a JSON object.")),
        Err(_) => Err(MeetingError::invalid("The body is not valid JSON.")),
    }
}

fn opt_str<'a>(body: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(MeetingError::invalid(format!("{key} must be a string."))),
    }
}

fn opt_bool(body: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(MeetingError::invalid(format!("{key} must be true or false."))),
    }
}

fn check_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(MeetingError::invalid(format!("title is at most {MAX_TITLE_CHARS} characters.")));
    }
    Ok(title.to_string())
}

fn check_reply_mode(mode: &str) -> Result<&'static str> {
    match mode {
        "chat" => Ok("chat"),
        "voice" => Ok("voice"),
        _ => Err(MeetingError::invalid("reply_mode must be chat or voice.")),
    }
}

fn parse_uuid(raw: &str, not_found: fn() -> MeetingError) -> Result<Uuid> {
    Uuid::parse_str(raw).map_err(|_| not_found())
}

pub(crate) fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn ts_opt(at: Option<DateTime<Utc>>) -> Value {
    at.map(|t| Value::String(ts(t))).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Rows and JSON
// ---------------------------------------------------------------------------

const MEETING_SELECT: &str = "SELECT m.id, m.user_id, m.org_id, m.platform, m.title, m.join_url, \
    m.url_key, m.starts_at, m.ends_at, m.source, m.calendar_connection_id, m.external_event_id, \
    m.attendee_count, m.external_attendee_count, m.is_one_on_one, m.auto_join, m.reply_mode, \
    m.latest_session_id, m.created_at, m.updated_at, s.state AS s_state, \
    s.end_reason AS s_end_reason, s.live_at AS s_live_at, \
    (s.summary IS NOT NULL) AS s_summary_ready, \
    (CASE WHEN jsonb_typeof(s.decisions) = 'array' THEN jsonb_array_length(s.decisions) ELSE 0 END) AS s_decisions, \
    (CASE WHEN jsonb_typeof(s.action_items) = 'array' THEN jsonb_array_length(s.action_items) ELSE 0 END) AS s_action_items \
    FROM meetings m LEFT JOIN meeting_sessions s ON s.id = m.latest_session_id";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct MeetingRow {
    pub id: Uuid,
    pub user_id: String,
    pub org_id: Option<String>,
    pub platform: String,
    pub title: String,
    pub join_url: String,
    pub url_key: String,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub source: String,
    pub calendar_connection_id: Option<Uuid>,
    pub external_event_id: Option<String>,
    pub attendee_count: Option<i32>,
    pub external_attendee_count: Option<i32>,
    pub is_one_on_one: bool,
    pub auto_join: Option<bool>,
    pub reply_mode: String,
    pub latest_session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub s_state: Option<String>,
    pub s_end_reason: Option<String>,
    pub s_live_at: Option<DateTime<Utc>>,
    pub s_summary_ready: Option<bool>,
    pub s_decisions: Option<i32>,
    pub s_action_items: Option<i32>,
}

impl MeetingRow {
    fn latest(&self) -> Option<(SessionState, Option<EndReason>)> {
        let state = SessionState::parse(self.s_state.as_deref()?)?;
        Some((state, self.s_end_reason.as_deref().and_then(EndReason::parse)))
    }

    pub fn gizzi(&self, rule: AutoJoinRule, now: DateTime<Utc>) -> &'static str {
        machine::meeting_gizzi(
            self.latest(),
            meeting_links::will_auto_join(self.auto_join, rule, self.is_one_on_one),
            self.starts_at.is_some_and(|s| s > now),
        )
    }

    pub fn platform_label(&self) -> &'static str {
        Platform::parse(&self.platform).map(Platform::label).unwrap_or("Meeting")
    }

    pub fn people_label(&self) -> String {
        people_label(self.attendee_count, self.external_attendee_count)
    }

    pub fn page_url(&self) -> String {
        format!("{PAGE_BASE}/{}", self.id)
    }

    /// `{ready, decisions, action_items}` when the latest session ended
    /// (`ready` = a summary exists), else null. Saves the list a request
    /// per row.
    pub fn recap(&self) -> Value {
        match self.s_state.as_deref() {
            Some("ended") => json!({
                "ready": self.s_summary_ready.unwrap_or(false),
                "decisions": self.s_decisions.unwrap_or(0),
                "action_items": self.s_action_items.unwrap_or(0),
            }),
            _ => Value::Null,
        }
    }

    /// When Gizzi went live in the current call: the active (not ended)
    /// latest session's `live_at`, else null.
    pub fn live_at(&self) -> Option<DateTime<Utc>> {
        match self.s_state.as_deref() {
            Some(state) if state != "ended" => self.s_live_at,
            _ => None,
        }
    }
}

/// "6 people", "8 people, 3 outside"; empty when the attendee count is not
/// known (a pasted link).
pub(crate) fn people_label(attendees: Option<i32>, outside: Option<i32>) -> String {
    let Some(n) = attendees else { return String::new() };
    let people = if n == 1 { "1 person".to_string() } else { format!("{n} people") };
    match outside {
        Some(m) if m > 0 => format!("{people}, {m} outside"),
        _ => people,
    }
}

pub(crate) fn meeting_json(row: &MeetingRow, rule: AutoJoinRule, now: DateTime<Utc>) -> Value {
    json!({
        "id": row.id,
        "user_id": row.user_id,
        "org_id": row.org_id,
        "platform": row.platform,
        "platform_label": row.platform_label(),
        "title": row.title,
        "join_url": row.join_url,
        "url_key": row.url_key,
        "starts_at": ts_opt(row.starts_at),
        "ends_at": ts_opt(row.ends_at),
        "source": row.source,
        "calendar_connection_id": row.calendar_connection_id,
        "external_event_id": row.external_event_id,
        "attendee_count": row.attendee_count,
        "external_attendee_count": row.external_attendee_count,
        "is_one_on_one": row.is_one_on_one,
        "auto_join": row.auto_join,
        "reply_mode": row.reply_mode,
        "gizzi": row.gizzi(rule, now),
        "people_label": row.people_label(),
        "latest_session_id": row.latest_session_id,
        "live_at": ts_opt(row.live_at()),
        "recap": row.recap(),
        "page_url": row.page_url(),
        "created_at": ts(row.created_at),
        "updated_at": ts(row.updated_at),
    })
}

pub(crate) const SESSION_COLUMNS: &str = "id, meeting_id, user_id, org_id, url_key, state, end_reason, \
    platform, speak_replies, listening, bot_identity, pod_ref, scheduled_join_at, joined_at, live_at, \
    ended_at, summary, decisions, action_items, created_at, updated_at";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct SessionRow {
    pub id: Uuid,
    pub meeting_id: Uuid,
    pub user_id: String,
    pub org_id: Option<String>,
    pub url_key: String,
    pub state: String,
    pub end_reason: Option<String>,
    pub platform: String,
    pub speak_replies: bool,
    pub listening: bool,
    pub bot_identity: String,
    #[allow(dead_code)]
    pub pod_ref: Option<String>,
    pub scheduled_join_at: Option<DateTime<Utc>>,
    pub joined_at: Option<DateTime<Utc>>,
    pub live_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
    pub decisions: Value,
    pub action_items: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SessionRow {
    pub fn state(&self) -> SessionState {
        SessionState::parse(&self.state).unwrap_or(SessionState::Ended)
    }
    pub fn end_reason(&self) -> Option<EndReason> {
        self.end_reason.as_deref().and_then(EndReason::parse)
    }
    pub fn gizzi(&self) -> &'static str {
        machine::session_gizzi(self.state(), self.end_reason())
    }
}

pub(crate) fn session_json(row: &SessionRow) -> Value {
    json!({
        "id": row.id,
        "meeting_id": row.meeting_id,
        "state": row.state,
        "gizzi": row.gizzi(),
        "end_reason": row.end_reason,
        "platform": row.platform,
        "speak_replies": row.speak_replies,
        "listening": row.listening,
        "bot_identity": row.bot_identity,
        "requested_by": row.user_id,
        "scheduled_join_at": ts_opt(row.scheduled_join_at),
        "joined_at": ts_opt(row.joined_at),
        "live_at": ts_opt(row.live_at),
        "ended_at": ts_opt(row.ended_at),
        "summary": row.summary,
        "decisions": row.decisions,
        "action_items": row.action_items,
        "created_at": ts(row.created_at),
        "updated_at": ts(row.updated_at),
    })
}

pub(crate) const PARTICIPANT_COLUMNS: &str =
    "id, session_id, platform_participant_id, display_name, email, user_id, class, joined_at, left_at, notice_at";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ParticipantRow {
    pub id: Uuid,
    pub session_id: Uuid,
    #[allow(dead_code)]
    pub platform_participant_id: Option<String>,
    pub display_name: String,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub class: String,
    pub joined_at: DateTime<Utc>,
    pub left_at: Option<DateTime<Utc>>,
    pub notice_at: Option<DateTime<Utc>>,
}

pub(crate) fn participant_json(row: &ParticipantRow) -> Value {
    json!({
        "id": row.id,
        "session_id": row.session_id,
        "display_name": row.display_name,
        "email": row.email,
        "user_id": row.user_id,
        "class": row.class,
        "joined_at": ts(row.joined_at),
        "left_at": ts_opt(row.left_at),
        "notice_at": ts_opt(row.notice_at),
    })
}

pub(crate) const SEGMENT_COLUMNS: &str =
    "id, session_id, seq, speaker, participant_id, confidence, source, start_ms, end_ms, text";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct SegmentRow {
    pub id: Uuid,
    pub session_id: Uuid,
    pub seq: i32,
    pub speaker: Option<String>,
    pub participant_id: Option<Uuid>,
    pub confidence: f32,
    pub source: String,
    pub start_ms: i32,
    pub end_ms: i32,
    pub text: String,
}

pub(crate) fn segment_json(row: &SegmentRow) -> Value {
    json!({
        "id": row.id,
        "session_id": row.session_id,
        "seq": row.seq,
        "speaker": row.speaker,
        "participant_id": row.participant_id,
        "confidence": row.confidence,
        "source": row.source,
        "start_ms": row.start_ms,
        "end_ms": row.end_ms,
        "text": row.text,
    })
}

pub(crate) const ARTIFACT_COLUMNS: &str =
    "id, session_id, artifact_id, kind, title, url, version, created_at, updated_at";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct MeetingArtifactRow {
    pub id: Uuid,
    pub session_id: Uuid,
    pub artifact_id: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) fn artifact_json(row: &MeetingArtifactRow) -> Value {
    json!({
        "id": row.id,
        "session_id": row.session_id,
        "artifact_id": row.artifact_id,
        "kind": row.kind,
        "title": row.title,
        "url": row.url,
        "version": row.version,
        "created_at": ts(row.created_at),
        "updated_at": ts(row.updated_at),
    })
}

pub(crate) const APPROVAL_COLUMNS: &str =
    "id, session_id, user_id, tool, args, args_summary, asked_by, status, requested_at, resolved_at, expires_at";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ApprovalRow {
    pub id: Uuid,
    pub session_id: Uuid,
    #[allow(dead_code)]
    pub user_id: String,
    pub tool: String,
    #[allow(dead_code)]
    pub args: Value,
    pub args_summary: String,
    pub asked_by: Option<String>,
    pub status: String,
    pub requested_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

pub(crate) fn approval_json(row: &ApprovalRow) -> Value {
    json!({
        "id": row.id,
        "session_id": row.session_id,
        "tool": row.tool,
        "args_summary": row.args_summary,
        "asked_by": row.asked_by,
        "status": row.status,
        "requested_at": ts(row.requested_at),
        "resolved_at": ts_opt(row.resolved_at),
        "expires_at": ts(row.expires_at),
    })
}

// ---------------------------------------------------------------------------
// Shared session helpers (also used by meeting_pod)
// ---------------------------------------------------------------------------

pub(crate) async fn fetch_session(db: &PgPool, sid: Uuid) -> Result<Option<SessionRow>> {
    Ok(sqlx::query_as::<_, SessionRow>(&format!("SELECT {SESSION_COLUMNS} FROM meeting_sessions WHERE id = $1"))
        .bind(sid)
        .fetch_optional(db)
        .await?)
}

async fn active_session_for(db: &PgPool, url_key: &str) -> Result<Option<SessionRow>> {
    Ok(sqlx::query_as::<_, SessionRow>(&format!(
        "SELECT {SESSION_COLUMNS} FROM meeting_sessions WHERE url_key = $1 AND state <> 'ended'"
    ))
    .bind(url_key)
    .fetch_optional(db)
    .await?)
}

pub(crate) fn state_event(row: &SessionRow) -> Value {
    json!({
        "type": "meeting.session.state",
        "session_id": row.id,
        "state": row.state,
        "gizzi": row.gizzi(),
        "end_reason": row.end_reason,
        "at": ts(row.updated_at),
    })
}

/// Move a session `from -> to` (already validated by the state machine).
/// Conditional on the stored state still being `from`, so two writers can't
/// both apply; a lost race is `409 conflict`. Sets the timestamps, the
/// `listening` flag, emits `meeting.session.state`, and on `ended` expires
/// pending approvals, writes the bot-minute meter row and emits
/// `meeting.ended`.
pub(crate) async fn set_state(
    db: &PgPool,
    sid: Uuid,
    from: SessionState,
    to: SessionState,
    reason: Option<EndReason>,
) -> Result<SessionRow> {
    let row = sqlx::query_as::<_, SessionRow>(&format!(
        "UPDATE meeting_sessions SET \
             state = $3, \
             end_reason = $4, \
             listening = CASE $3 WHEN 'paused' THEN false WHEN 'ended' THEN false WHEN 'live' THEN true ELSE listening END, \
             joined_at = CASE WHEN $3 IN ('waiting_admit', 'live') THEN COALESCE(joined_at, now()) ELSE joined_at END, \
             live_at = CASE WHEN $3 = 'live' THEN COALESCE(live_at, now()) ELSE live_at END, \
             ended_at = CASE WHEN $3 = 'ended' THEN now() ELSE ended_at END, \
             updated_at = now() \
         WHERE id = $1 AND state = $2 \
         RETURNING {SESSION_COLUMNS}"
    ))
    .bind(sid)
    .bind(from.as_str())
    .bind(to.as_str())
    .bind(reason.map(EndReason::as_str))
    .fetch_optional(db)
    .await?
    .ok_or_else(|| MeetingError::conflict("The session changed state; reload and try again."))?;

    events::emit(sid, "meeting.session.state", state_event(&row));
    super::meeting_notify::on_state(db.clone(), sid);
    if to == SessionState::Ended {
        on_ended(db, &row).await?;
    }
    Ok(row)
}

async fn on_ended(db: &PgPool, row: &SessionRow) -> Result<()> {
    sqlx::query(
        "UPDATE meeting_tool_approvals SET status = 'expired', resolved_at = now() \
         WHERE session_id = $1 AND status = 'pending'",
    )
    .bind(row.id)
    .execute(db)
    .await?;
    meter_session(db, row.id).await?;
    let ended_at = row.ended_at.unwrap_or(row.updated_at);
    events::emit(
        row.id,
        "meeting.ended",
        json!({
            "type": "meeting.ended",
            "session_id": row.id,
            "end_reason": row.end_reason,
            "ended_at": ts(ended_at),
        }),
    );
    events::retire(row.id);
    Ok(())
}

/// Write the bot-minute meter row for an ended session: `live_at` to
/// `ended_at`, rounded up to whole seconds, 0 when the bot never went live.
/// `UNIQUE (session_id)` makes it exactly once however often it runs.
pub(crate) async fn meter_session(db: &PgPool, sid: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_bot_minutes (user_id, org_id, session_id, seconds, platform, occurred_at) \
         SELECT user_id, org_id, id, \
                CASE WHEN live_at IS NULL OR ended_at IS NULL THEN 0 \
                     ELSE GREATEST(0, CEIL(EXTRACT(EPOCH FROM (ended_at - live_at))))::int END, \
                platform, COALESCE(ended_at, now()) \
         FROM meeting_sessions WHERE id = $1 AND state = 'ended' \
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(sid)
    .execute(db)
    .await?;
    Ok(())
}

/// The session as the caller may see it: the owner, or a user who has a
/// (not deleted) meeting with the same `url_key` (an attached requester).
/// Returns the session, whether the caller owns it, and the caller's own
/// meeting row for it.
async fn session_for(db: &PgPool, caller: &Caller, raw_sid: &str) -> Result<(SessionRow, bool, MeetingRow)> {
    let sid = parse_uuid(raw_sid, MeetingError::session_not_found)?;
    let session = fetch_session(db, sid).await?.ok_or_else(MeetingError::session_not_found)?;
    let owner = session.user_id == caller.id;
    let meeting = if owner {
        sqlx::query_as::<_, MeetingRow>(&format!("{MEETING_SELECT} WHERE m.id = $1"))
            .bind(session.meeting_id)
            .fetch_optional(db)
            .await?
    } else {
        sqlx::query_as::<_, MeetingRow>(&format!(
            "{MEETING_SELECT} WHERE m.user_id = $1 AND m.url_key = $2 AND m.deleted_at IS NULL \
             ORDER BY (m.latest_session_id = $3) DESC NULLS LAST, m.created_at DESC LIMIT 1"
        ))
        .bind(&caller.id)
        .bind(&session.url_key)
        .bind(session.id)
        .fetch_optional(db)
        .await?
    };
    let meeting = meeting.ok_or_else(MeetingError::session_not_found)?;
    Ok((session, owner, meeting))
}

async fn owned_meeting(db: &PgPool, caller: &Caller, raw_id: &str) -> Result<MeetingRow> {
    let id = parse_uuid(raw_id, MeetingError::meeting_not_found)?;
    sqlx::query_as::<_, MeetingRow>(&format!(
        "{MEETING_SELECT} WHERE m.id = $1 AND m.user_id = $2 AND m.deleted_at IS NULL"
    ))
    .bind(id)
    .bind(&caller.id)
    .fetch_optional(db)
    .await?
    .ok_or_else(MeetingError::meeting_not_found)
}

pub(crate) async fn fetch_meeting(db: &PgPool, id: Uuid) -> Result<MeetingRow> {
    sqlx::query_as::<_, MeetingRow>(&format!("{MEETING_SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(MeetingError::meeting_not_found)
}

async fn user_rule(db: &PgPool, user_id: &str) -> Result<AutoJoinRule> {
    let user_settings = settings::load_user_settings(db, user_id).await?;
    Ok(AutoJoinRule::parse(&user_settings.auto_join).unwrap_or(AutoJoinRule::Off))
}

fn global_cap() -> i64 {
    std::env::var("MEETINGS_GLOBAL_CONCURRENT_CAP")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n: &i64| *n >= 0)
        .unwrap_or(DEFAULT_GLOBAL_CAP)
}

/// `429 no_capacity` when the service or the user is at its concurrent bot
/// cap (sessions that hold a pod: launching through paused).
pub(crate) async fn check_capacity(db: &PgPool, user_id: &str, plan: &str) -> Result<()> {
    let (global, mine): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE user_id = $1) FROM meeting_sessions \
         WHERE state IN ('launching', 'joining', 'waiting_admit', 'live', 'paused')",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    if global >= global_cap() {
        return Err(MeetingError::no_capacity("Gizzi is in too many meetings right now. Try again in a few minutes."));
    }
    let cap = settings::plan_limits(plan).concurrent_cap;
    if mine >= cap {
        return Err(MeetingError::no_capacity(format!(
            "Your plan allows Gizzi in {cap} meeting{} at a time.",
            if cap == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GET /api/v1/meetings
// ---------------------------------------------------------------------------

const ACTIVE_SQL: &str = "(s.state IS NOT NULL AND s.state NOT IN ('ended', 'scheduled'))";
/// A meeting still running (or about to): its end, or an hour after start.
const NOT_OVER_SQL: &str = "COALESCE(m.ends_at, m.starts_at + interval '1 hour') >= now()";

/// Start of the caller's day, in UTC, for `tz` (IANA name; default UTC).
fn day_start(tz: Option<&str>, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let tz: chrono_tz::Tz = match tz.map(str::trim).filter(|t| !t.is_empty()) {
        None => chrono_tz::UTC,
        Some(name) => name.parse().map_err(|_| MeetingError::invalid("tz must be an IANA time zone name."))?,
    };
    let local = now.with_timezone(&tz).date_naive();
    let midnight = local.and_hms_opt(0, 0, 0).expect("midnight exists");
    Ok(tz
        .from_local_datetime(&midnight)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(now))
}

async fn list_meetings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let now = Utc::now();
    let range = query.get("range").map(String::as_str).unwrap_or("upcoming");
    let limit = match query.get("limit") {
        None => DEFAULT_LIST_LIMIT,
        Some(raw) => raw
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIST_LIMIT).contains(n))
            .ok_or_else(|| MeetingError::invalid(format!("limit must be 1 to {MAX_LIST_LIMIT}.")))?,
    };
    let offset = match query.get("cursor").map(String::as_str).filter(|c| !c.is_empty()) {
        None => 0,
        Some(raw) => raw
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .ok_or_else(|| MeetingError::invalid("cursor is not valid."))?,
    };
    let start = day_start(query.get("tz").map(String::as_str), now)?;
    let (filter, order) = match range {
        "now" => (
            format!("({ACTIVE_SQL} OR (m.starts_at <= now() + interval '15 minutes' AND {NOT_OVER_SQL}))"),
            "asc",
        ),
        "today" => (format!("({ACTIVE_SQL} OR (m.starts_at >= $2 AND m.starts_at < $2 + interval '1 day'))"), "asc"),
        "week" => (format!("({ACTIVE_SQL} OR (m.starts_at >= $2 AND m.starts_at < $2 + interval '7 days'))"), "asc"),
        "upcoming" => (
            format!(
                "({ACTIVE_SQL} OR (m.starts_at IS NOT NULL AND {NOT_OVER_SQL}) \
                 OR (m.starts_at IS NULL AND (s.state IS NULL OR s.state <> 'ended')))"
            ),
            "asc",
        ),
        "past" => (
            format!(
                "(NOT {ACTIVE_SQL} AND ((m.starts_at IS NOT NULL AND NOT {NOT_OVER_SQL}) OR s.state = 'ended'))"
            ),
            "desc",
        ),
        _ => return Err(MeetingError::invalid("range must be now, today, week, upcoming or past.")),
    };
    let order_sql = if order == "desc" {
        "m.starts_at DESC NULLS LAST, m.created_at DESC".to_string()
    } else {
        format!("(CASE WHEN {ACTIVE_SQL} THEN 0 ELSE 1 END), m.starts_at ASC NULLS LAST, m.created_at DESC")
    };
    let sql = format!(
        "{MEETING_SELECT} WHERE m.user_id = $1 AND m.deleted_at IS NULL AND {filter} \
         ORDER BY {order_sql} LIMIT $3 OFFSET $4"
    );
    let rows = sqlx::query_as::<_, MeetingRow>(&sql)
        .bind(&caller.id)
        .bind(start)
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
    let rule = user_rule(&state.db, &caller.id).await?;
    let more = rows.len() as i64 > limit;
    let meetings: Vec<Value> = rows.iter().take(limit as usize).map(|r| meeting_json(r, rule, now)).collect();
    Ok(Json(json!({
        "meetings": meetings,
        "next_cursor": if more { Value::String((offset + limit).to_string()) } else { Value::Null },
    })))
}

// ---------------------------------------------------------------------------
// POST /api/v1/meetings
// ---------------------------------------------------------------------------

pub(crate) struct NewMeeting<'a> {
    pub url: &'a str,
    pub title: Option<&'a str>,
    pub starts_at: Option<DateTime<Utc>>,
    pub reply_mode: Option<&'a str>,
    pub source: &'a str,
}

/// Add a meeting from a link, deduped per user by `(url_key, starts_at)`.
/// Returns the row and whether it was newly created (or restored from a
/// soft delete).
pub(crate) async fn add_meeting(db: &PgPool, caller: &Caller, new: NewMeeting<'_>) -> Result<(MeetingRow, bool)> {
    let link = meeting_links::parse_link(new.url).ok_or_else(MeetingError::unsupported_link)?;
    let title = new.title.map(check_title).transpose()?;
    let reply_mode = match new.reply_mode {
        Some(mode) => check_reply_mode(mode)?.to_string(),
        None => settings::load_user_settings(db, &caller.id).await?.default_reply_mode,
    };

    for attempt in 0..2 {
        let existing: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT id, deleted_at FROM meetings \
             WHERE user_id = $1 AND url_key = $2 AND starts_at IS NOT DISTINCT FROM $3 \
             ORDER BY deleted_at DESC NULLS FIRST LIMIT 1",
        )
        .bind(&caller.id)
        .bind(&link.url_key)
        .bind(new.starts_at)
        .fetch_optional(db)
        .await?;
        match existing {
            Some((id, None)) => return Ok((fetch_meeting(db, id).await?, false)),
            Some((id, Some(_))) => {
                sqlx::query(
                    "UPDATE meetings SET deleted_at = NULL, join_url = $2, title = COALESCE($3, title), \
                     reply_mode = $4, auto_join = NULL, updated_at = now() WHERE id = $1",
                )
                .bind(id)
                .bind(&link.join_url)
                .bind(title.as_deref())
                .bind(&reply_mode)
                .execute(db)
                .await?;
                return Ok((fetch_meeting(db, id).await?, true));
            }
            None => {}
        }
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO meetings (user_id, org_id, platform, title, join_url, url_key, starts_at, source, reply_mode) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(&caller.id)
        .bind(caller.org_id.as_deref())
        .bind(link.platform.as_str())
        .bind(title.as_deref().unwrap_or(""))
        .bind(&link.join_url)
        .bind(&link.url_key)
        .bind(new.starts_at)
        .bind(new.source)
        .bind(&reply_mode)
        .fetch_one(db)
        .await;
        match inserted {
            Ok(id) => return Ok((fetch_meeting(db, id).await?, true)),
            // A concurrent add of the same meeting won: read it back.
            Err(error) if is_unique_violation(&error) && attempt == 0 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(MeetingError::internal())
}

fn parse_time(raw: Option<&str>, key: &str) -> Result<Option<DateTime<Utc>>> {
    raw.map(|s| {
        DateTime::parse_from_rfc3339(s.trim())
            .map(|t| t.with_timezone(&Utc))
            .map_err(|_| MeetingError::invalid(format!("{key} must be an ISO-8601 time.")))
    })
    .transpose()
}

async fn create_meeting(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    org_gate(&state.db, caller.org_id.as_deref(), caller.org_role.as_deref()).await?;
    let body = parse_body(&body)?;
    let url = opt_str(&body, "url")?.ok_or_else(|| MeetingError::invalid("url is required."))?;
    // `source` is optional: the chat link card sends "chat".
    let source = match opt_str(&body, "source")? {
        None | Some("link") => "link",
        Some("chat") => "chat",
        Some(_) => return Err(MeetingError::invalid("source must be link or chat.")),
    };
    let new = NewMeeting {
        url,
        title: opt_str(&body, "title")?,
        starts_at: parse_time(opt_str(&body, "starts_at")?, "starts_at")?,
        reply_mode: opt_str(&body, "reply_mode")?,
        source,
    };
    let (row, created) = add_meeting(&state.db, &caller, new).await?;
    let rule = user_rule(&state.db, &caller.id).await?;
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(meeting_json(&row, rule, Utc::now()))).into_response())
}

// ---------------------------------------------------------------------------
// GET / PATCH / DELETE /api/v1/meetings/:id
// ---------------------------------------------------------------------------

/// One meeting with its `gizzi` status plus `latest_session`
/// `{id, state, end_reason, live_at, ended_at, summary}` (null before Gizzi
/// was sent). 404 unless the caller owns it.
async fn get_meeting(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let meeting = owned_meeting(&state.db, &caller, &id).await?;
    let rule = user_rule(&state.db, &caller.id).await?;
    let latest = match meeting.latest_session_id {
        Some(sid) => fetch_session(&state.db, sid).await?,
        None => None,
    };
    let mut body = meeting_json(&meeting, rule, Utc::now());
    body["latest_session"] = latest
        .as_ref()
        .map(|s| {
            json!({
                "id": s.id,
                "state": s.state,
                "end_reason": s.end_reason,
                "live_at": ts_opt(s.live_at),
                "ended_at": ts_opt(s.ended_at),
                "summary": s.summary,
            })
        })
        .unwrap_or(Value::Null);
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// PATCH / DELETE /api/v1/meetings/:id
// ---------------------------------------------------------------------------

async fn patch_meeting(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let meeting = owned_meeting(&state.db, &caller, &id).await?;
    let body = parse_body(&body)?;
    let row = update_meeting(&state.db, &meeting, &body).await?;
    let rule = user_rule(&state.db, &caller.id).await?;
    Ok(Json(meeting_json(&row, rule, Utc::now())))
}

/// Apply `{ title?, auto_join?, reply_mode? }`; `auto_join: null` clears the
/// override (follow the user's rule).
pub(crate) async fn update_meeting(db: &PgPool, meeting: &MeetingRow, body: &Map<String, Value>) -> Result<MeetingRow> {
    for key in body.keys() {
        if !matches!(key.as_str(), "title" | "auto_join" | "reply_mode") {
            return Err(MeetingError::invalid(format!("Unknown field '{key}'.")));
        }
    }
    let title = match body.get("title") {
        None => meeting.title.clone(),
        Some(Value::String(t)) => check_title(t)?,
        Some(_) => return Err(MeetingError::invalid("title must be a string.")),
    };
    let auto_join = match body.get("auto_join") {
        None => meeting.auto_join,
        Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(MeetingError::invalid("auto_join must be true, false or null.")),
    };
    let reply_mode = match opt_str(body, "reply_mode")? {
        None => meeting.reply_mode.clone(),
        Some(mode) => check_reply_mode(mode)?.to_string(),
    };
    sqlx::query("UPDATE meetings SET title = $2, auto_join = $3, reply_mode = $4, updated_at = now() WHERE id = $1")
        .bind(meeting.id)
        .bind(&title)
        .bind(auto_join)
        .bind(&reply_mode)
        .execute(db)
        .await?;
    fetch_meeting(db, meeting.id).await
}

async fn delete_meeting(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let meeting = owned_meeting(&state.db, &caller, &id).await?;
    if let Some(sid) = meeting.latest_session_id {
        if let Some(session) = fetch_session(&state.db, sid).await? {
            let current = session.state();
            if current.holds_pod() {
                return Err(MeetingError::conflict("Gizzi is in this meeting. Make it leave first."));
            }
            // A not-yet-launched bot this user scheduled is cancelled.
            if current == SessionState::Scheduled && session.user_id == caller.id {
                set_state(&state.db, sid, current, SessionState::Ended, Some(EndReason::LeftByUser)).await?;
            }
        }
    }
    sqlx::query("UPDATE meetings SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(meeting.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /api/v1/meetings/:id/sessions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum When {
    Now,
    Start,
}

impl When {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "now" => Some(When::Now),
            "start" => Some(When::Start),
            _ => None,
        }
    }
}

/// Send Gizzi to a meeting. Checks, in order: plan (402), org enabled and
/// role (403), outside attendees vs the org's guest setting (403),
/// Zoom connected (409 `zoom_not_connected`, new Zoom bots only), capacity
/// (429, only when a new pod is needed), then dedupe by `url_key`:
/// an active session for the same URL is attached to (200) instead of a
/// second bot. Returns the session and whether it was created.
pub(crate) async fn start_session(
    db: &PgPool,
    caller: &Caller,
    meeting: &MeetingRow,
    when: When,
    speak_replies: Option<bool>,
) -> Result<(SessionRow, bool)> {
    let plan = require_paid(db, caller).await?;
    let org = org_gate(db, meeting.org_id.as_deref(), caller.role_in(meeting.org_id.as_deref())).await?;
    if meeting.external_attendee_count.unwrap_or(0) > 0 && org.as_ref().is_some_and(|o| !o.allow_external_guests) {
        return Err(MeetingError::forbidden(
            "This meeting has people from outside your organization, and your organization's \
             allow_external_guests setting is off.",
        ));
    }
    if when == When::Start && meeting.starts_at.is_none() {
        return Err(MeetingError::invalid("This meeting has no start time. Send Gizzi now instead."));
    }

    for attempt in 0..2 {
        if let Some(existing) = active_session_for(db, &meeting.url_key).await? {
            let mut session = existing;
            if when == When::Now && session.state() == SessionState::Scheduled {
                check_capacity(db, &caller.id, &plan).await?;
                session = set_state(db, session.id, SessionState::Scheduled, SessionState::Launching, None).await?;
            }
            attach(db, meeting.id, session.id).await?;
            return Ok((session, false));
        }
        // A new Zoom bot joins on the caller's behalf (OBF token from their
        // connected Zoom account); attaching to a running bot needs none.
        if meeting.platform == "zoom" && !zoom_connected(db, &caller.id).await? {
            return Err(MeetingError::new(
                StatusCode::CONFLICT,
                "zoom_not_connected",
                "Connect your Zoom account first. Gizzi joins Zoom calls on your behalf.",
            ));
        }
        if when == When::Now {
            check_capacity(db, &caller.id, &plan).await?;
        }
        let (initial, scheduled_join_at) = match when {
            When::Now => (SessionState::Launching, None),
            When::Start => (SessionState::Scheduled, meeting.starts_at.map(|s| s - JOIN_LEAD)),
        };
        let speak = speak_replies.unwrap_or(meeting.reply_mode == "voice");
        let inserted = sqlx::query_as::<_, SessionRow>(&format!(
            "INSERT INTO meeting_sessions (meeting_id, user_id, org_id, url_key, state, platform, \
                 speak_replies, bot_identity, scheduled_join_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {SESSION_COLUMNS}"
        ))
        .bind(meeting.id)
        .bind(&caller.id)
        .bind(meeting.org_id.as_deref())
        .bind(&meeting.url_key)
        .bind(initial.as_str())
        .bind(&meeting.platform)
        .bind(speak)
        .bind(BOT_IDENTITY)
        .bind(scheduled_join_at)
        .fetch_one(db)
        .await;
        match inserted {
            Ok(session) => {
                attach(db, meeting.id, session.id).await?;
                events::emit(session.id, "meeting.session.state", state_event(&session));
                return Ok((session, true));
            }
            // Someone else's request for the same URL got there first.
            Err(error) if is_unique_violation(&error) && attempt == 0 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(MeetingError::conflict("A session for this meeting is already starting."))
}

async fn zoom_connected(db: &PgPool, user_id: &str) -> Result<bool> {
    let row: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM zoom_connections WHERE user_id = $1 AND status = 'active'")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

async fn attach(db: &PgPool, meeting_id: Uuid, sid: Uuid) -> Result<()> {
    sqlx::query("UPDATE meetings SET latest_session_id = $2, updated_at = now() WHERE id = $1")
        .bind(meeting_id)
        .bind(sid)
        .execute(db)
        .await?;
    Ok(())
}

async fn create_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    // Plan first, before revealing whether the meeting exists.
    require_paid(&state.db, &caller).await?;
    let body = parse_body(&body)?;
    let when = opt_str(&body, "when")?
        .and_then(When::parse)
        .ok_or_else(|| MeetingError::invalid("when must be now or start."))?;
    let speak_replies = opt_bool(&body, "speak_replies")?;
    let meeting = owned_meeting(&state.db, &caller, &id).await?;
    let (session, created) = start_session(&state.db, &caller, &meeting, when, speak_replies).await?;
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(session_json(&session))).into_response())
}

// ---------------------------------------------------------------------------
// GET /api/v1/meeting-sessions/:sid (+ transcript, artifacts)
// ---------------------------------------------------------------------------

async fn get_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let (session, owner, meeting) = session_for(&state.db, &caller, &sid).await?;
    let participants = sqlx::query_as::<_, ParticipantRow>(&format!(
        "SELECT {PARTICIPANT_COLUMNS} FROM meeting_participants WHERE session_id = $1 ORDER BY joined_at, display_name"
    ))
    .bind(session.id)
    .fetch_all(&state.db)
    .await?;
    // Approvals act on the owner's tools and accounts: only the owner sees them.
    let approvals = if owner {
        sqlx::query_as::<_, ApprovalRow>(&format!(
            "SELECT {APPROVAL_COLUMNS} FROM meeting_tool_approvals \
             WHERE session_id = $1 AND status = 'pending' AND expires_at > now() ORDER BY requested_at"
        ))
        .bind(session.id)
        .fetch_all(&state.db)
        .await?
    } else {
        Vec::new()
    };
    let rule = user_rule(&state.db, &caller.id).await?;
    Ok(Json(json!({
        "session": session_json(&session),
        "meeting": meeting_json(&meeting, rule, Utc::now()),
        "participants": participants.iter().map(participant_json).collect::<Vec<_>>(),
        "pending_approvals": approvals.iter().map(approval_json).collect::<Vec<_>>(),
    })))
}

/// Transcript page after `after_seq` (exclusive). `next_after_seq` is the
/// last seq returned (poll with it), or null when the page is empty.
pub(crate) async fn transcript_page(
    db: &PgPool,
    sid: Uuid,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<(Vec<SegmentRow>, Option<i32>)> {
    let limit = limit.unwrap_or(DEFAULT_TRANSCRIPT_LIMIT);
    if !(1..=MAX_TRANSCRIPT_LIMIT).contains(&limit) {
        return Err(MeetingError::invalid(format!("limit must be 1 to {MAX_TRANSCRIPT_LIMIT}.")));
    }
    let after = after_seq.unwrap_or(-1);
    if after < -1 {
        return Err(MeetingError::invalid("after_seq must be 0 or more."));
    }
    let rows = sqlx::query_as::<_, SegmentRow>(&format!(
        "SELECT {SEGMENT_COLUMNS} FROM meeting_transcript_segments WHERE session_id = $1 AND seq > $2 \
         ORDER BY seq LIMIT $3"
    ))
    .bind(sid)
    .bind(after)
    .bind(limit)
    .fetch_all(db)
    .await?;
    let next = rows.last().map(|r| r.seq);
    Ok((rows, next))
}

fn int_param(query: &HashMap<String, String>, key: &str) -> Result<Option<i64>> {
    query
        .get(key)
        .filter(|v| !v.is_empty())
        .map(|v| v.parse::<i64>().map_err(|_| MeetingError::invalid(format!("{key} must be a whole number."))))
        .transpose()
}

async fn get_transcript(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let (session, _, _) = session_for(&state.db, &caller, &sid).await?;
    let (rows, next) =
        transcript_page(&state.db, session.id, int_param(&query, "after_seq")?, int_param(&query, "limit")?).await?;
    Ok(Json(json!({
        "segments": rows.iter().map(segment_json).collect::<Vec<_>>(),
        "next_after_seq": next,
    })))
}

async fn list_artifacts(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let (session, _, _) = session_for(&state.db, &caller, &sid).await?;
    let rows = sqlx::query_as::<_, MeetingArtifactRow>(&format!(
        "SELECT {ARTIFACT_COLUMNS} FROM meeting_artifacts WHERE session_id = $1 ORDER BY created_at"
    ))
    .bind(session.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "artifacts": rows.iter().map(artifact_json).collect::<Vec<_>>() })))
}

// ---------------------------------------------------------------------------
// GET /api/v1/meeting-sessions/:sid/events (SSE)
// ---------------------------------------------------------------------------

async fn session_events(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Result<Response> {
    use futures::stream::{self, StreamExt};
    use tokio_stream::wrappers::BroadcastStream;

    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let (session, _, _) = session_for(&state.db, &caller, &sid).await?;
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());

    let (replay, rx) = events::subscribe(session.id, last_event_id);
    let floor = replay.last().map(|f| f.id).unwrap_or(0);
    // The current state first, without an id so a reconnect keeps resuming
    // from the last numbered frame.
    let first = SseEvent::default()
        .event("meeting.session.state")
        .data(state_event(&session).to_string());
    let to_event = |frame: &events::Frame| {
        SseEvent::default()
            .id(frame.id.to_string())
            .event(frame.event)
            .data(frame.data.to_string())
    };
    let replayed: Vec<SseEvent> = replay.iter().map(|f| to_event(f)).collect();
    let live = BroadcastStream::new(rx).filter_map(move |item| async move {
        match item {
            Ok(frame) if frame.id > floor => Some(to_event(&frame)),
            // Lagged past the channel: the client can reload on the next frame.
            _ => None,
        }
    });
    let stream = stream::iter(std::iter::once(first).chain(replayed))
        .chain(live)
        .map(Ok::<_, std::convert::Infallible>);
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20)).text("ping"))
        .into_response())
}

// ---------------------------------------------------------------------------
// POST control, PATCH session, approvals
// ---------------------------------------------------------------------------

fn require_owner(owner: bool) -> Result<()> {
    if owner {
        Ok(())
    } else {
        Err(MeetingError::forbidden("Only the person who sent Gizzi can control it."))
    }
}

/// Apply a control action to a session (owner already checked).
pub(crate) async fn control(db: &PgPool, session: SessionRow, action: &str, text: Option<&str>) -> Result<SessionRow> {
    let current = session.state();
    if current == SessionState::Ended {
        return Err(MeetingError::not_active());
    }
    let sid = session.id;
    match action {
        "stop_listening" => {
            if current != SessionState::Live {
                return Err(MeetingError::conflict("Gizzi isn't listening in a live call right now."));
            }
            let row = set_state(db, sid, current, SessionState::Paused, None).await?;
            events::push_control(sid, json!({ "type": "stop_listening" }));
            Ok(row)
        }
        "resume" => {
            if current != SessionState::Paused {
                return Err(MeetingError::conflict("Gizzi isn't paused."));
            }
            let row = set_state(db, sid, current, SessionState::Live, None).await?;
            events::push_control(sid, json!({ "type": "resume" }));
            Ok(row)
        }
        "leave" => {
            events::push_control(sid, json!({ "type": "leave" }));
            set_state(db, sid, current, SessionState::Ended, Some(EndReason::LeftByUser)).await
        }
        "speak" => {
            let text = text.map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| {
                MeetingError::invalid("speak needs text.")
            })?;
            if text.chars().count() > MAX_SPEAK_CHARS {
                return Err(MeetingError::invalid(format!("text is at most {MAX_SPEAK_CHARS} characters.")));
            }
            if !session.speak_replies {
                return Err(MeetingError::conflict("Turn on Speak replies first."));
            }
            if !matches!(current, SessionState::Live | SessionState::Paused) {
                return Err(MeetingError::conflict("Gizzi isn't in the call yet."));
            }
            events::push_control(sid, json!({ "type": "speak", "text": text }));
            Ok(session)
        }
        _ => Err(MeetingError::invalid("action must be stop_listening, resume, leave or speak.")),
    }
}

async fn control_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let body = parse_body(&body)?;
    let (session, owner, _) = session_for(&state.db, &caller, &sid).await?;
    require_owner(owner)?;
    let action = opt_str(&body, "action")?.ok_or_else(|| MeetingError::invalid("action is required."))?;
    let row = control(&state.db, session, action, opt_str(&body, "text")?).await?;
    Ok(Json(session_json(&row)))
}

async fn patch_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let body = parse_body(&body)?;
    for key in body.keys() {
        if key != "speak_replies" {
            return Err(MeetingError::invalid(format!("Unknown field '{key}'.")));
        }
    }
    let (session, owner, _) = session_for(&state.db, &caller, &sid).await?;
    require_owner(owner)?;
    let Some(on) = opt_bool(&body, "speak_replies")? else {
        return Ok(Json(session_json(&session)));
    };
    let row = sqlx::query_as::<_, SessionRow>(&format!(
        "UPDATE meeting_sessions SET speak_replies = $2, updated_at = now() \
         WHERE id = $1 AND state <> 'ended' RETURNING {SESSION_COLUMNS}"
    ))
    .bind(session.id)
    .bind(on)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(MeetingError::not_active)?;
    events::push_control(row.id, json!({ "type": "set_speak_replies", "on": on }));
    Ok(Json(session_json(&row)))
}

async fn decide_approval(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((sid, aid)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let body = parse_body(&body)?;
    let status = match opt_str(&body, "decision")? {
        Some("approve") => "approved",
        Some("deny") => "denied",
        _ => return Err(MeetingError::invalid("decision must be approve or deny.")),
    };
    let (session, owner, _) = session_for(&state.db, &caller, &sid).await?;
    require_owner(owner)?;
    let aid = Uuid::parse_str(&aid)
        .map_err(|_| MeetingError::new(StatusCode::NOT_FOUND, "not_found", "Approval not found."))?;
    let approval = sqlx::query_as::<_, ApprovalRow>(&format!(
        "SELECT {APPROVAL_COLUMNS} FROM meeting_tool_approvals WHERE id = $1 AND session_id = $2"
    ))
    .bind(aid)
    .bind(session.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MeetingError::new(StatusCode::NOT_FOUND, "not_found", "Approval not found."))?;
    if approval.status == "pending" && approval.expires_at <= Utc::now() {
        sqlx::query("UPDATE meeting_tool_approvals SET status = 'expired', resolved_at = now() WHERE id = $1 AND status = 'pending'")
            .bind(aid)
            .execute(&state.db)
            .await?;
        return Err(MeetingError::conflict("This request expired. Ask Gizzi again."));
    }
    let row = sqlx::query_as::<_, ApprovalRow>(&format!(
        "UPDATE meeting_tool_approvals SET status = $2, resolved_at = now() \
         WHERE id = $1 AND status = 'pending' RETURNING {APPROVAL_COLUMNS}"
    ))
    .bind(aid)
    .bind(status)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MeetingError::conflict("This request was already answered."))?;
    let payload = approval_json(&row);
    events::emit(
        session.id,
        "meeting.approval.resolved",
        json!({ "type": "meeting.approval.resolved", "session_id": session.id, "approval": payload }),
    );
    events::push_control(session.id, json!({ "type": "approval", "approval_id": row.id, "status": row.status }));
    Ok(Json(payload))
}

#[cfg(test)]
#[path = "meetings_tests.rs"]
mod tests;
