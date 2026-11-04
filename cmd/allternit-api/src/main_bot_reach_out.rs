//! The main bot reaches out first.
//!
//! When a bot becomes the main bot (first-run setup, or "Make main bot" on the
//! Bots rail), it posts a real message into its main chat without being asked:
//! a short hello and two or three pieces of work it can take on, picked from
//! what is set up (mail and calendar connectors, the bot's email address, a
//! computer). The text is built here with no model call, so it arrives even
//! before the bot's brain answers.
//!
//! Each offer is a normal bot suggestion (`bot_suggestions::suggest`: a task
//! thread in status `proposed`), so Go ahead / Not now work on every surface
//! that lists suggestions (the chat's suggested-task chips, the stage card,
//! Inbox "Needs you").
//!
//! The message is written to the bot ledger as `bot.message.posted`, which the
//! Desktop feed and the cloud push sink read as a new message
//! (`message.received`: "<Bot> sent you a message", quiet hours apply).
//!
//! Once per bot per account: the ledger row's idempotency key
//! `reach-out:<user>:<bot>` is the marker. The chat message is also deduped by
//! its `remote_event_id`, and a suggestion with the same open title is reused,
//! so a retry after a partial failure adds nothing. The first-day introduction
//! (`morning_brief`) is marked done for this bot, since this message replaces it.
//!
//! Route (authenticated, under `/api/v1`):
//! `POST /bots/:id/reach-out` `{sessionId?}` →
//! `{status: "posted" | "already", sessionId?, suggestions: [threadId]}`.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::bot_routing::{caller_for_session, Caller, GizziRouter};
use crate::db::DbHandle;
use crate::AppState;

/// The ledger type for a message a bot posted on its own (pushed like a received message).
pub const POSTED: &str = "bot.message.posted";
pub const SOURCE: &str = "bot-reach-out";
const MAX_OFFERS: usize = 3;

pub fn reach_out_router() -> Router<Arc<AppState>> {
    Router::new().route("/bots/:id/reach-out", post(reach_out_route))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReachOutBody {
    #[serde(default)]
    pub session_id: Option<String>,
}

/// What is set up, as far as the offers care.
#[derive(Debug, Default, Clone)]
pub struct Setup {
    pub mail: bool,
    pub calendar: bool,
    pub bot_email: Option<String>,
    pub computer: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub title: String,
    pub objective: String,
    pub why: String,
}

fn offer(title: &str, objective: String, why: &str) -> Offer {
    Offer { title: title.into(), objective, why: why.into() }
}

/// Two or three concrete offers from what is set up; general ones fill the rest.
pub fn offers_for(s: &Setup) -> Vec<Offer> {
    let mut out = Vec::new();
    if s.mail {
        out.push(offer(
            "Sort my inbox",
            "Go through my unread email from the last week, tell me which ones need a reply, and draft those replies for me to check. Don't send anything.".into(),
            "Your mail is connected",
        ));
    }
    if s.calendar {
        out.push(offer(
            "Prep my week",
            "Look at my calendar for the next 7 days and list what I need to prepare for each meeting, with anything that clashes.".into(),
            "Your calendar is connected",
        ));
    }
    if let Some(addr) = s.bot_email.as_deref() {
        out.push(offer(
            "Answer my bot's mail",
            format!("Check the mail that has come in to {addr} and draft a reply to each one for me to approve before it goes out."),
            "I have my own email address",
        ));
    }
    if let Some(name) = s.computer.as_deref() {
        out.push(offer(
            "Tidy my computer",
            format!("On {name}, find old files in Downloads and large files I may not need, and list them. Ask me before you move or delete anything."),
            "A computer is set up for me",
        ));
    }
    if !(s.mail && s.calendar) {
        out.push(offer(
            "Connect my mail and calendar",
            "Walk me through connecting my email and calendar so you can sort my inbox and prep my meetings.".into(),
            "Most of my help starts with your mail and calendar",
        ));
    }
    out.push(offer(
        "Plan my week",
        "Ask me what's on this week, then turn it into a short task list with what you can take on yourself.".into(),
        "A quick way to hand me work",
    ));
    out.truncate(MAX_OFFERS);
    out
}

/// The message itself: hello, the offers, how to answer.
pub fn message_text(bot_name: &str, offers: &[Offer]) -> String {
    let mut out = format!(
        "I'm {bot_name}, your main bot now. I'll check in with you each morning at 9 with work I can take off your plate.\n\nHere's what I can start on right now:\n"
    );
    for o in offers {
        out.push_str(&format!("\n- **{}**: {}", o.title, o.objective));
    }
    out.push_str("\n\nTap **Go ahead** on any of these and I'll get started, or **Not now** to skip it. Nothing runs until you say so.");
    out
}

fn idem_key(user_id: &str, bot_id: &str) -> String {
    format!("reach-out:{user_id}:{bot_id}")
}

fn bot_name(db: &DbHandle, user_id: &str, bot_id: &str) -> Option<(String, Option<String>, Option<String>)> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT COALESCE(json_extract(config, '$.botProfile.displayName'), name), email_address, json_extract(config, '$.canonicalThreadId') FROM agents
         WHERE id = ?1 AND user_id = ?2 AND (is_bot = 1 OR json_extract(config, '$.isBot') = 1)",
        params![bot_id, user_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
    )
    .optional()
    .ok()
    .flatten()
}

fn already(db: &DbHandle, user_id: &str, bot_id: &str) -> bool {
    db.connect()
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT 1 FROM bot_events WHERE bot_id = ?1 AND idempotency_key = ?2",
                params![bot_id, idem_key(user_id, bot_id)],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
        })
        .is_some()
}

fn is_main_chat_of(db: &DbHandle, bot_id: &str, session_id: &str) -> bool {
    db.get_session_metadata(session_id).ok().flatten().is_some_and(|bag| bag["botCanonicalFor"].as_str() == Some(bot_id))
}

/// The bot's main chat: the one the caller named, its pin, the newest tagged one, else a new one (pinned).
async fn main_chat(db: &DbHandle, bot_id: &str, name: &str, asked: Option<&str>, pin: Option<&str>) -> Result<String, String> {
    for s in [asked, pin].into_iter().flatten() {
        if is_main_chat_of(db, bot_id, s) {
            return Ok(s.to_string());
        }
    }
    let tagged: Option<String> = db.connect().ok().and_then(|conn| {
        conn.query_row(
            "SELECT session_id FROM session_metadata WHERE json_extract(metadata, '$.botCanonicalFor') = ?1 ORDER BY rowid DESC LIMIT 1",
            params![bot_id],
            |r| r.get(0),
        )
        .ok()
    });
    let session = match tagged {
        Some(s) => s,
        None => crate::agent_session_routes::create_bot_thread_session(db, bot_id, name, name, true, None).await?,
    };
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "UPDATE agents SET config = json_set(COALESCE(config, '{}'), '$.canonicalThreadId', ?2) WHERE id = ?1",
            params![bot_id, session],
        );
    }
    Ok(session)
}

async fn setup_for(db: &DbHandle, user_id: &str, bot_id: &str, bot_email: Option<String>) -> Setup {
    let rows = crate::open_connector_proxy::list_user_connections_all(user_id).await.unwrap_or_default();
    let has = |svc: &str| rows.iter().any(|(s, _, _)| s == svc);
    let computer: Option<String> = db.connect().ok().and_then(|c| {
        c.query_row(
            "SELECT name FROM computers WHERE status <> 'deleted' AND (bot_id = ?2 OR (owner_type = 'user' AND owner_id = ?1))
             ORDER BY CASE WHEN bot_id = ?2 THEN 0 ELSE 1 END LIMIT 1",
            params![user_id, bot_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    });
    Setup {
        mail: has("gmail") || has("outlook"),
        calendar: has("googlecalendar") || has("outlook_calendar"),
        bot_email: bot_email.filter(|e| e.contains('@')),
        computer,
    }
}

/// Post the reach-out once. `Ok(None)` when this bot already reached out.
pub async fn reach_out(db: &DbHandle, user_id: &str, bot_id: &str, asked_session: Option<&str>) -> Result<Option<(String, Vec<String>)>, (StatusCode, String)> {
    let (name, email, pin) = bot_name(db, user_id, bot_id).ok_or((StatusCode::NOT_FOUND, "bot not found".to_string()))?;
    if already(db, user_id, bot_id) {
        return Ok(None);
    }
    let session = main_chat(db, bot_id, &name, asked_session, pin.as_deref()).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let setup = setup_for(db, user_id, bot_id, email).await;
    let offers = offers_for(&setup);

    let caller = caller_for_session(db, &session).filter(|c| c.bot_id == bot_id && c.user_id == user_id).unwrap_or(Caller {
        bot_id: bot_id.to_string(),
        user_id: user_id.to_string(),
        name: name.clone(),
        session_id: session.clone(),
        thread_id: None,
        project_id: None,
    });
    let rt = GizziRouter { db: db.clone() };
    let mut ids = Vec::new();
    for o in &offers {
        match crate::bot_suggestions::suggest(db, &rt, &caller, &o.title, &o.objective, Some(&o.why)).await {
            Ok(v) => ids.extend(v["threadId"].as_str().map(str::to_string)),
            Err(e) => tracing::warn!(error = %e.message(), bot = %bot_id, "reach-out offer not opened"),
        }
    }

    let text = message_text(&name, &offers);
    crate::agent_session_routes::append_vendor_message(
        db,
        &session,
        &text,
        json!({ "source": SOURCE, "remote_event_id": format!("reach-out:{bot_id}"), "vendor": "allternit", "adapter": SOURCE, "suggestions": ids }),
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;

    // This message is the bot's first word; the first-day intro would repeat it.
    if let Ok(c) = db.connect() {
        let now = chrono::Utc::now();
        let _ = c.execute(
            "INSERT OR IGNORE INTO bot_intro (user_id, bot_id, local_date, status, claimed_at, created_at) VALUES (?1,?2,?3,'answered',?4,?4)",
            params![user_id, bot_id, now.format("%Y-%m-%d").to_string(), now.to_rfc3339()],
        );
    }

    // The marker, and the push ("<Bot> sent you a message", quiet hours apply in the sink).
    let payload: Value = json!({ "from": name, "text": format!("{name} sent you a message"), "source": SOURCE, "sessionId": session, "suggestions": ids });
    crate::gateway_runner::led(db, bot_id, caller.thread_id.as_deref().unwrap_or(""), Some(&session), POSTED, ("bot", bot_id), payload, Some(idem_key(user_id, bot_id)));
    Ok(Some((session, ids)))
}

async fn reach_out_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>, body: Option<Json<ReachOutBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    match reach_out(&state.db, &user.user_id, &bot_id, body.session_id.as_deref()).await {
        Ok(Some((session, ids))) => Json(json!({ "status": "posted", "sessionId": session, "suggestions": ids })).into_response(),
        Ok(None) => Json(json!({ "status": "already", "suggestions": [] })).into_response(),
        Err((status, e)) => (status, Json(json!({ "error": e }))).into_response(),
    }
}

/// After a check-in routine run that worked: tell the person the bot posted (same push as above).
pub fn notify_check_in(db: &DbHandle, bot_id: &str, bot_name: &str, session: Option<&str>, run_key: &str) {
    let payload = json!({ "from": bot_name, "text": format!("{bot_name} sent you a message"), "source": "check-in", "sessionId": session });
    crate::gateway_runner::led(db, bot_id, "", session, POSTED, ("bot", bot_id), payload, Some(format!("check-in:{run_key}")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_follow_what_is_set_up() {
        let none = offers_for(&Setup::default());
        assert_eq!(none.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), vec!["Connect my mail and calendar", "Plan my week"]);
        let all = offers_for(&Setup { mail: true, calendar: true, bot_email: Some("a@b.co".into()), computer: Some("Mac".into()) });
        assert_eq!(all.len(), MAX_OFFERS);
        assert_eq!(all[0].title, "Sort my inbox");
        assert_eq!(all[2].title, "Answer my bot's mail");
        assert!(all[2].objective.contains("a@b.co"));
    }

    #[test]
    fn message_lists_every_offer() {
        let o = offers_for(&Setup { mail: true, ..Setup::default() });
        let t = message_text("Goldie", &o);
        assert!(t.starts_with("I'm Goldie, your main bot now."));
        for x in &o {
            assert!(t.contains(&x.title));
        }
        assert!(t.contains("Nothing runs until you say so."));
    }

    #[test]
    fn route_has_no_overlap() {
        let _ = crate::morning_brief::brief_router().merge(reach_out_router()).merge(crate::bot_suggestions::bot_suggestions_router());
    }
}
