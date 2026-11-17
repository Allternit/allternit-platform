//! An agent reaching its own user (Primitives Wave A, `send_user_message`):
//! gizzi's tool sends files and asks for a push through these two routes.
//!
//! Unlike the bot tools (`thread_bot_tools`), these work in every session the
//! caller owns: a bot's chat, a plain chat, a Gizzi Code session. Whose
//! session it is comes from [`resolve_owner`], never from the model. Gizzi
//! sends the user's token or the internal service token.
//!
//! * `POST /bot-routing/user/files` `{sessionId, path, caption?}` → `{id, name, size}`.
//!   Copies a local file (the runtime and this API share the machine) into the
//!   Files drive's Agent Output folder with a provenance sidecar
//!   (`files_v2::routing::import_agent_output`). Refused: a relative path (400
//!   `not_absolute`), nothing there (404 `missing`), a folder (400
//!   `not_a_file`), more than [`MAX_FILE_BYTES`] (413 `too_large`), the drive
//!   off (409 `drive_off`).
//! * `POST /bot-routing/user/notify` `{sessionId, kind: message|question, level: normal|needs_you, preview, ref?}`
//!   → `{pushed, reason?: rate_limited|unpaired|incognito, event}`. Level
//!   `normal` writes `agent.message.sent` (quiet hours apply); `needs_you`
//!   writes `thread.needs_user` at urgency `normal`, never `urgent`. A bot's
//!   chat writes to `bot_events`, any other session to `runtime_user_events`;
//!   the forwarder (`runtime_events`) takes it to the cloud, which pushes it.
//!   `pushed` means the push was requested, not delivered. `unpaired`: this
//!   runtime forwards nothing (no cloud link, or linked to another account).
//!   `incognito`: an incognito chat never leaves this computer, so nothing is
//!   written. The limiter ([`MAX_PER_THREAD_PER_HOUR`],
//!   [`MAX_PER_USER_PER_DAY`]) counts this route's own ledger rows; the cloud
//!   applies its own message limiter too (`web_push::allow_message_push`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{AuthUser, INTERNAL_SERVICE_USER_ID};
use crate::bot_event_routes::{append_event, ActorBody, AppendEventBody};
use crate::bot_routing::{caller_for_session, Caller};
use crate::db::DbHandle;
use crate::files_v2::routing::{import_agent_output, ImportError, SentBy};
use crate::relay_auth::RelaySecret;
use crate::AppState;

// ---- limits (one place; Eoj can change them) ----

/// Largest file an agent may send.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// Pushes one thread (or session) may ask for in an hour ...
pub const MAX_PER_THREAD_PER_HOUR: i64 = 4;
/// ... and one person in a day.
pub const MAX_PER_USER_PER_DAY: i64 = 20;
/// The preview shown in the notification.
pub const MAX_PREVIEW_CHARS: usize = 200;
const MAX_REF_CHARS: usize = 200;
const MAX_CAPTION_CHARS: usize = 200;
/// Marks this route's ledger rows (the limiter counts only these).
const ORIGIN: &str = "user_notify";
pub const EVENT_MESSAGE: &str = "agent.message.sent";
pub const EVENT_NEEDS_USER: &str = "thread.needs_user";

pub fn agent_user_tools_router() -> Router<Arc<AppState>> {
    Router::new().route("/bot-routing/user/files", post(files_route)).route("/bot-routing/user/notify", post(notify_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesBody {
    pub session_id: String,
    pub path: String,
    pub caption: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyBody {
    pub session_id: String,
    pub kind: String,
    pub level: String,
    pub preview: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into(), "code": code }))).into_response()
}

fn clip(s: &str, cap: usize) -> String {
    s.trim().chars().take(cap).collect::<String>().trim_end().to_string()
}

// ---- whose session ----

/// The session's owner, and its bot when it is a bot's chat.
#[derive(Debug, Clone)]
pub struct Owner {
    pub user_id: String,
    pub bot: Option<Caller>,
}

/// A refusal: status, code, sentence.
pub type Refusal = (StatusCode, &'static str, String);

fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
}

/// Who owns `session_id`, checked against the caller.
///
/// * A bot's chat: the bot's owner, the same rule as `bot_routing::authorize`.
/// * Any other session (a chat, Gizzi Code): this runtime has one owner, the
///   account it is paired as. A signed-in caller must be that account (any
///   account when the runtime is not paired); the internal service acts for it.
pub fn resolve_owner(db: &DbHandle, user_id: &str, session_id: &str, paired_owner: Option<&str>) -> Result<Owner, Refusal> {
    if !valid_session_id(session_id) {
        return Err((StatusCode::BAD_REQUEST, "bad_session", "sessionId is missing or malformed".into()));
    }
    let internal = user_id == INTERNAL_SERVICE_USER_ID;
    if let Some(c) = caller_for_session(db, session_id) {
        if !internal && c.user_id != user_id {
            return Err((StatusCode::NOT_FOUND, "not_your_session", "This session isn't yours.".into()));
        }
        return Ok(Owner { user_id: c.user_id.clone(), bot: Some(c) });
    }
    let paired = paired_owner.map(str::trim).filter(|o| !o.is_empty());
    let owner = match (internal, paired) {
        (true, Some(p)) => p.to_string(),
        (true, None) => "local".to_string(),
        (false, Some(p)) if p != user_id => return Err((StatusCode::NOT_FOUND, "not_your_session", "This session isn't yours.".into())),
        (false, _) => user_id.to_string(),
    };
    Ok(Owner { user_id: owner, bot: None })
}

async fn owner_for(state: &AppState, user: &AuthUser, session_id: &str) -> Result<Owner, Response> {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), session_id.to_string());
    let paired = crate::relay_auth::process_secret().paired_owner();
    match tokio::task::spawn_blocking(move || resolve_owner(&db, &uid, &sid, paired.as_deref())).await {
        Ok(Ok(o)) => Ok(o),
        Ok(Err((status, code, msg))) => Err(err(status, code, msg)),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, "failed", e.to_string())),
    }
}

// ---- files ----

/// The HTTP answer for a refused import.
pub fn import_refusal(e: &ImportError) -> Refusal {
    match e {
        ImportError::NotAbsolute => (StatusCode::BAD_REQUEST, "not_absolute", "path must be absolute".into()),
        ImportError::Missing => (StatusCode::NOT_FOUND, "missing", "There's no file at that path.".into()),
        ImportError::NotAFile => (StatusCode::BAD_REQUEST, "not_a_file", "That's a folder, not a file. Send files one at a time.".into()),
        ImportError::TooLarge { limit } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large", format!("Files can be at most {} MB.", limit / (1024 * 1024))),
        ImportError::DriveOff => (StatusCode::CONFLICT, "drive_off", "The Files drive is off on this computer, so the file wasn't saved to Agent Output.".into()),
        ImportError::Failed(m) => (StatusCode::INTERNAL_SERVER_ERROR, "failed", m.clone()),
    }
}

async fn files_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<FilesBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let by = SentBy {
        session_id: body.session_id.clone(),
        tool: "send_user_message".into(),
        agent_id: owner.bot.as_ref().map(|c| c.bot_id.clone()),
        caption: body.caption.as_deref().map(|c| clip(c, MAX_CAPTION_CHARS)).filter(|c| !c.is_empty()),
    };
    match import_agent_output(PathBuf::from(body.path.trim()), MAX_FILE_BYTES, by).await {
        Ok(f) => Json(json!({ "id": f.id, "name": f.name, "size": f.size })).into_response(),
        Err(e) => {
            let (status, code, msg) = import_refusal(&e);
            err(status, code, msg)
        }
    }
}

// ---- notify ----

/// What the limiter allows.
#[derive(Debug, PartialEq)]
pub enum Allowance {
    Ok,
    RateLimited,
}

const COUNTED: &str = "event_type IN ('agent.message.sent', 'thread.needs_user') AND json_extract(payload, '$.origin') = 'user_notify'";

/// [`MAX_PER_THREAD_PER_HOUR`] per thread key, [`MAX_PER_USER_PER_DAY`] per
/// person, counted from this route's rows in both ledgers.
pub fn allowance(conn: &rusqlite::Connection, user_id: &str, thread_key: &str, now: chrono::DateTime<chrono::Utc>) -> rusqlite::Result<Allowance> {
    let hour = (now - chrono::Duration::hours(1)).to_rfc3339();
    let day = (now - chrono::Duration::days(1)).to_rfc3339();
    let per_thread: i64 = conn.query_row(
        &format!(
            "SELECT (SELECT count(*) FROM bot_events WHERE {COUNTED} AND json_extract(payload, '$.threadKey') = ?1 AND julianday(occurred_at) > julianday(?2))
                  + (SELECT count(*) FROM runtime_user_events WHERE {COUNTED} AND json_extract(payload, '$.threadKey') = ?1 AND julianday(occurred_at) > julianday(?2))"
        ),
        params![thread_key, hour],
        |r| r.get(0),
    )?;
    if per_thread >= MAX_PER_THREAD_PER_HOUR {
        return Ok(Allowance::RateLimited);
    }
    let per_user: i64 = conn.query_row(
        &format!(
            "SELECT (SELECT count(*) FROM bot_events e JOIN agents a ON a.id = e.bot_id WHERE a.user_id = ?1 AND {COUNTED} AND julianday(e.occurred_at) > julianday(?2))
                  + (SELECT count(*) FROM runtime_user_events WHERE user_id = ?1 AND {COUNTED} AND julianday(occurred_at) > julianday(?2))"
        ),
        params![user_id, day],
        |r| r.get(0),
    )?;
    Ok(if per_user >= MAX_PER_USER_PER_DAY { Allowance::RateLimited } else { Allowance::Ok })
}

/// A name for a session that has no bot: its title when the runtime knows one.
fn session_title(db: &DbHandle, session_id: &str) -> Option<String> {
    let from_meta = db.get_session_metadata(session_id).ok().flatten().and_then(|bag| {
        ["title", "sessionTitle", "agentName"].iter().find_map(|k| bag[*k].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string))
    });
    from_meta
        .or_else(|| {
            let conn = db.connect().ok()?;
            conn.query_row("SELECT name FROM agent_sessions WHERE id = ?1", params![session_id], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten().flatten()
        })
        .map(|t| clip(&t, 80))
        .filter(|t| !t.is_empty())
}

fn incognito(db: &DbHandle, owner: &Owner, session_id: &str) -> bool {
    match owner.bot.as_ref().and_then(|c| c.thread_id.as_deref()) {
        Some(tid) => db
            .connect()
            .ok()
            .and_then(|c| c.query_row("SELECT incognito FROM bot_threads WHERE id = ?1", params![tid], |r| r.get::<_, i64>(0)).optional().ok().flatten())
            .is_some_and(|v| v != 0),
        None => db.is_session_ephemeral(session_id).unwrap_or(false),
    }
}

/// Write the notify event and say whether a push was requested.
pub fn notify(db: &DbHandle, owner: &Owner, body: &NotifyBody, secret: &dyn RelaySecret) -> Result<Value, Refusal> {
    let kind = match body.kind.trim() {
        k @ ("message" | "question") => k,
        _ => return Err((StatusCode::BAD_REQUEST, "bad_kind", "kind must be message or question".into())),
    };
    let event = match body.level.trim() {
        "normal" => EVENT_MESSAGE,
        "needs_you" => EVENT_NEEDS_USER,
        _ => return Err((StatusCode::BAD_REQUEST, "bad_level", "level must be normal or needs_you".into())),
    };
    let preview = clip(&body.preview, MAX_PREVIEW_CHARS);
    if preview.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "bad_preview", "preview is required".into()));
    }
    let sid = body.session_id.as_str();
    if incognito(db, owner, sid) {
        return Ok(json!({ "pushed": false, "reason": "incognito", "event": event }));
    }
    let thread_id = owner.bot.as_ref().and_then(|c| c.thread_id.clone());
    let thread_key = thread_id.clone().unwrap_or_else(|| sid.to_string());
    let now = chrono::Utc::now();
    let db_err = |e: rusqlite::Error| (StatusCode::INTERNAL_SERVER_ERROR, "failed", e.to_string());
    let conn = db.connect().map_err(db_err)?;
    if allowance(&conn, &owner.user_id, &thread_key, now).map_err(db_err)? == Allowance::RateLimited {
        return Ok(json!({ "pushed": false, "reason": "rate_limited", "event": event }));
    }
    let mut payload = json!({ "origin": ORIGIN, "threadKey": thread_key, "sessionId": sid, "kind": kind, "text": preview });
    if event == EVENT_NEEDS_USER {
        // Normal urgency always: an agent's own ask never pages through quiet hours.
        payload["reason"] = json!(preview);
        payload["urgency"] = json!("normal");
    }
    if let Some(r) = body.reference.as_deref().map(|r| clip(r, MAX_REF_CHARS)).filter(|r| !r.is_empty()) {
        payload["ref"] = json!(r);
    }
    match &owner.bot {
        Some(c) => match &thread_id {
            Some(tid) => crate::thread_routes::ledger(db, &c.bot_id, tid, event, ("bot", c.bot_id.as_str()), payload, Some(sid)),
            None => {
                let append = AppendEventBody {
                    event_type: event.to_string(),
                    actor: ActorBody { r#type: "bot".into(), id: c.bot_id.clone() },
                    payload,
                    occurred_at: None,
                    session_id: Some(sid.to_string()),
                    goal_id: None,
                    wih_id: None,
                    task_id: None,
                    run_id: None,
                    idempotency_key: None,
                };
                append_event(db, &c.bot_id, &append, &now.to_rfc3339()).map_err(db_err)?;
            }
        },
        None => {
            if let Some(t) = session_title(db, sid) {
                payload["sessionTitle"] = json!(t);
            }
            crate::runtime_events::record_user_event(&conn, &owner.user_id, event, Some(sid), &payload, None).map_err(db_err)?;
        }
    }
    // The forwarder sends only rows of the account this runtime is paired as.
    let paired = secret.device_token().is_some() && secret.runtime_id().is_some() && secret.paired_owner().as_deref() == Some(owner.user_id.as_str());
    Ok(if paired { json!({ "pushed": true, "event": event }) } else { json!({ "pushed": false, "reason": "unpaired", "event": event }) })
}

async fn notify_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<NotifyBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || notify(&db, &owner, &body, crate::relay_auth::process_secret().as_ref())).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err((status, code, msg))) => err(status, code, msg),
        Err(e) => err(StatusCode::BAD_GATEWAY, "failed", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_auth::UnconfiguredRelaySecret;
    use crate::runtime_events::testing::Paired;
    use crate::runtime_events::{envelope, read_rows, SOURCE_BOT, SOURCE_USER};

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-user-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    fn me(user: &str) -> Owner {
        Owner { user_id: user.into(), bot: None }
    }

    fn body(session: &str, level: &str) -> NotifyBody {
        NotifyBody { session_id: session.into(), kind: "message".into(), level: level.into(), preview: "Your report is ready".into(), reference: None }
    }

    #[tokio::test]
    async fn the_fifth_push_in_an_hour_is_refused_per_thread_and_twenty_a_day_per_person() {
        let st = setup().await;
        for _ in 0..MAX_PER_THREAD_PER_HOUR {
            let v = notify(&st.db, &me("user-a"), &body("s-1", "normal"), &Paired).unwrap();
            assert_eq!(v["pushed"], true, "{v}");
        }
        let v = notify(&st.db, &me("user-a"), &body("s-1", "needs_you"), &Paired).unwrap();
        assert_eq!((v["pushed"].as_bool(), v["reason"].as_str()), (Some(false), Some("rate_limited")));
        // Another thread still gets through, until the person's daily cap.
        for s in 2..=5 {
            for _ in 0..MAX_PER_THREAD_PER_HOUR {
                assert_eq!(notify(&st.db, &me("user-a"), &body(&format!("s-{s}"), "normal"), &Paired).unwrap()["pushed"], true);
            }
        }
        let v = notify(&st.db, &me("user-a"), &body("s-6", "normal"), &Paired).unwrap();
        assert_eq!(v["reason"], "rate_limited");
        // A refused push writes nothing.
        let n: i64 = st.db.connect().unwrap().query_row("SELECT count(*) FROM runtime_user_events WHERE user_id = 'user-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, MAX_PER_USER_PER_DAY);
    }

    #[tokio::test]
    async fn an_unlinked_runtime_or_another_account_answers_unpaired() {
        let st = setup().await;
        let v = notify(&st.db, &me("user-a"), &body("s-1", "normal"), &UnconfiguredRelaySecret).unwrap();
        assert_eq!((v["pushed"].as_bool(), v["reason"].as_str()), (Some(false), Some("unpaired")));
        // Paired as user-a: user-b's rows are never forwarded.
        let v = notify(&st.db, &me("user-b"), &body("s-2", "normal"), &Paired).unwrap();
        assert_eq!(v["reason"], "unpaired");
    }

    #[tokio::test]
    async fn needs_you_is_thread_needs_user_at_normal_urgency_in_both_ledgers() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        // No bot: the owner ledger, forwarded as thread.needs_user with urgency normal.
        notify(&st.db, &me("user-a"), &body("s-chat", "needs_you"), &Paired).unwrap();
        let (rows, _, _) = read_rows(&c, "user-a", SOURCE_USER, 0).unwrap();
        let ev = envelope(SOURCE_USER, &rows[0]).unwrap();
        assert_eq!((ev["type"].as_str(), ev["data"]["urgency"].as_str()), (Some("thread.needs_user"), Some("normal")));
        assert_eq!((ev["data"]["reason"].as_str(), ev["thread_id"].as_str()), (Some("Your report is ready"), Some("s-chat")));
        // Level normal is the new event.
        notify(&st.db, &me("user-a"), &body("s-chat", "normal"), &Paired).unwrap();
        let (rows, _, _) = read_rows(&c, "user-a", SOURCE_USER, 0).unwrap();
        assert_eq!(envelope(SOURCE_USER, &rows[1]).unwrap()["type"], "agent.message.sent");

        // A bot's chat: the bot ledger.
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1', 'user-a', 'Olive', 'm', 'p', 1, '{}')", []).unwrap();
        st.db.set_session_metadata("s-bot", &json!({ "isBot": true, "agentId": "bot-1" })).unwrap();
        let owner = resolve_owner(&st.db, "user-a", "s-bot", Some("user-a")).unwrap();
        assert_eq!(owner.bot.as_ref().map(|b| b.bot_id.as_str()), Some("bot-1"));
        let v = notify(&st.db, &owner, &body("s-bot", "needs_you"), &Paired).unwrap();
        assert_eq!(v["pushed"], true);
        let (rows, _, _) = read_rows(&c, "user-a", SOURCE_BOT, 0).unwrap();
        let ev = rows.iter().filter_map(|r| envelope(SOURCE_BOT, r)).find(|e| e["data"]["sessionId"] == "s-bot").unwrap();
        assert_eq!((ev["type"].as_str(), ev["data"]["urgency"].as_str(), ev["bot_id"].as_str()), (Some("thread.needs_user"), Some("normal"), Some("bot-1")));
    }

    #[tokio::test]
    async fn sessions_belong_to_their_owner() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-b', 'user-b', 'Raven', 'm', 'p', 1, '{}')", []).unwrap();
        st.db.set_session_metadata("s-theirs", &json!({ "isBot": true, "agentId": "bot-b" })).unwrap();
        assert_eq!(resolve_owner(&st.db, "user-a", "s-theirs", None).unwrap_err().1, "not_your_session");
        // A session with no bot belongs to the account this runtime is paired as.
        assert_eq!(resolve_owner(&st.db, "user-a", "s-code", Some("user-a")).unwrap().user_id, "user-a");
        assert_eq!(resolve_owner(&st.db, "user-b", "s-code", Some("user-a")).unwrap_err().1, "not_your_session");
        assert_eq!(resolve_owner(&st.db, INTERNAL_SERVICE_USER_ID, "s-code", Some("user-a")).unwrap().user_id, "user-a");
        assert_eq!(resolve_owner(&st.db, "user-a", "../etc", None).unwrap_err().1, "bad_session");
    }

    #[test]
    fn files_refuse_relative_paths_folders_and_big_files() {
        let tmp = tempfile::tempdir().unwrap();
        let code = |p: &std::path::Path| crate::files_v2::routing::check_source(p, MAX_FILE_BYTES).map(|_| ()).map_err(|e| import_refusal(&e).1);
        assert_eq!(code(std::path::Path::new("report.pdf")), Err("not_absolute"));
        assert_eq!(code(tmp.path()), Err("not_a_file"));
        assert_eq!(code(&tmp.path().join("nope.txt")), Err("missing"));
        let big = tmp.path().join("big.bin");
        std::fs::File::create(&big).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
        assert_eq!(code(&big), Err("too_large"));
        assert_eq!(import_refusal(&ImportError::TooLarge { limit: MAX_FILE_BYTES }).0, StatusCode::PAYLOAD_TOO_LARGE);
        let ok = tmp.path().join("ok.txt");
        std::fs::write(&ok, b"hi").unwrap();
        assert_eq!(code(&ok), Ok(()));
    }

    #[test]
    fn a_sent_file_lands_beside_its_provenance_and_never_replaces_one() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("Agent Output");
        let src = tmp.path().join("chart.png");
        std::fs::write(&src, b"png").unwrap();
        let by = SentBy { session_id: "s-1".into(), tool: "send_user_message".into(), agent_id: None, caption: Some("Q3".into()) };
        let now = chrono::Utc::now();
        let (first, size) = crate::files_v2::routing::copy_with_provenance(&out, &src, MAX_FILE_BYTES, &by, now).unwrap();
        assert_eq!((first.file_name().unwrap().to_str(), size), (Some("chart.png"), 3));
        let side: Value = serde_json::from_slice(&std::fs::read(out.join("chart.png.provenance.json")).unwrap()).unwrap();
        assert_eq!((side["provenance"]["source"].as_str(), side["sent_by"]["session_id"].as_str()), (Some("agent"), Some("s-1")));
        let (second, _) = crate::files_v2::routing::copy_with_provenance(&out, &src, MAX_FILE_BYTES, &by, now).unwrap();
        assert_eq!(second.file_name().unwrap().to_str(), Some("chart 2.png"));
        assert_eq!(std::fs::read(&first).unwrap(), b"png");
    }
}
