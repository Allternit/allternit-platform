//! Allternit Meetings steps 2.9 (recap email) and 2.14 (pushes).
//!
//! Two hooks, both fire-and-forget (they spawn and never fail the caller):
//! - [`on_state`]: a session changed state (`meetings::set_state`).
//! - [`on_summary`]: the pod's end summary was stored (`meeting_pod::ingest`).
//!
//! ## Pushes (owner only)
//! `joining`, `waiting_admit`, `live`, `not_admitted` and `ended` with a recap
//! ready send one alert with `url: /meetings/<meeting id>`, through the
//! person's browsers and iPhones ([`web_push::send_to_user`] also reaches
//! APNs). Alerts for one session share the tag `meeting-<session id>`, so a
//! repeat replaces itself. They honour a `meeting.update` switch in
//! `notification_preferences` (default on) and quiet hours. Live Activity
//! pushes go to `MeetingActivityAttributes` tokens: an update to the token
//! registered for the session, or a push-to-start to the device's start token
//! when no activity runs yet. The content state is exactly
//! `{gizzi, liveSince, listening}` (MeetingActivityAttributes.swift).
//!
//! ## Recap (contract section 2, `user_meeting_settings.recap_email`)
//! When a session has ended and has a summary, the recap goes to the meeting
//! owner and to participants classed `org_member` whose email is on the
//! invite. Never to guests. There is no org-members list in cloud-api (only
//! the JWT's active org), so "in the org" is the participant class. Sending
//! needs `ALLTERNIT_CUSTOMER_EMAILS=1` and goes through
//! `customer_emails::deliver`, which claims (user, `meeting_recap`, session id)
//! first, so a recap is sent once per person per session.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::apns::{self, Apns, PushTarget, RawOptions};
use super::live_activity::{la_payload, LaExtras, LA_TOPIC_SUFFIX};
use super::meetings::{self, MeetingRow, SessionRow, PAGE_BASE};
use super::notifications::{self, Level};
use super::web_push::{self, HttpTransport, PushMessage, PushTransport, Vapid};
use crate::services::customer_emails::{self, MeetingRecap, Recipient};

pub const ATTRIBUTES: &str = "MeetingActivityAttributes";
/// The registry-style switch name in `notification_preferences`.
pub const EVENT: &str = "meeting.update";
const DISMISS_SECS: i64 = 5 * 60;

pub fn on_state(db: PgPool, sid: Uuid) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        if let Err(error) = handle(&db, sid, false).await {
            tracing::warn!(%sid, "meeting notify (state): {error}");
        }
    });
}

pub fn on_summary(db: PgPool, sid: Uuid) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        if let Err(error) = handle(&db, sid, true).await {
            tracing::warn!(%sid, "meeting notify (summary): {error}");
        }
    });
}

async fn handle(db: &PgPool, sid: Uuid, summary_arrived: bool) -> Result<(), String> {
    let Some(session) = meetings::fetch_session(db, sid).await.map_err(|e| format!("{e:?}"))? else { return Ok(()) };
    let meeting = meetings::fetch_meeting(db, session.meeting_id).await.map_err(|e| format!("{e:?}"))?;
    let ended = session.state == "ended";
    if summary_arrived && !ended {
        // The summary is stored; the ended transition (same batch) handles the rest.
        return Ok(());
    }
    push_state(db, &session, &meeting, &*transport(), Vapid::from_env().as_ref(), Apns::from_env().is_some()).await;
    if ended && session.summary.is_some() {
        let owner = if customer_emails::enabled() { customer_emails::fetch_recipient(&session.user_id).await } else { None };
        send_recap(db, &session, &meeting, owner).await;
    }
    Ok(())
}

fn transport() -> Arc<dyn PushTransport> {
    #[cfg(test)]
    if let Some(t) = web_push::TEST_TRANSPORT.lock().unwrap().clone() {
        return t;
    }
    Arc::new(HttpTransport::new())
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

fn name_of(meeting: &MeetingRow) -> String {
    let t = meeting.title.trim();
    if t.is_empty() { "your meeting".to_string() } else { t.to_string() }
}

/// The alert for a session's current state, or `None` when that state sends none.
pub fn alert_copy(session: &SessionRow, meeting: &MeetingRow) -> Option<(String, String, Level)> {
    let name = name_of(meeting);
    let platform = meeting.platform_label();
    match session.gizzi() {
        "joining" => Some(("Gizzi is joining".into(), format!("{name} · {platform}"), Level::Normal)),
        "waiting_admit" => Some(("Let Gizzi in".into(), format!("Gizzi is waiting to be let into {name}."), Level::NeedsYou)),
        "live" => Some(("Gizzi is in the call".into(), format!("{name} · Gizzi is listening."), Level::Normal)),
        "not_admitted" => Some(("Gizzi wasn't let in".into(), format!("Nobody let Gizzi into {name}."), Level::Normal)),
        "ended" if session.summary.is_some() => {
            let count = |v: &Value| v.as_array().map_or(0, Vec::len);
            let (d, a) = (count(&session.decisions), count(&session.action_items));
            let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
            Some(("Recap ready".into(), format!("{name}: {}, {}.", plural(d, "decision"), plural(a, "action item")), Level::Normal))
        }
        _ => None,
    }
}

/// The push for one state change: opens the meeting page.
pub fn push_message(session: &SessionRow, meeting: &MeetingRow) -> Option<(PushMessage, Level)> {
    let (title, body, level) = alert_copy(session, meeting)?;
    let gizzi = session.gizzi();
    let msg = PushMessage {
        kind: "event",
        title,
        body,
        url: format!("/meetings/{}", meeting.id),
        tag: format!("meeting-{}", session.id),
        data: json!({
            "event": EVENT,
            "meetingId": meeting.id,
            "sessionId": session.id,
            "gizzi": gizzi,
            "level": if level == Level::NeedsYou { "needs_you" } else { "normal" },
        }),
        ttl_secs: 3600,
        high_urgency: level == Level::NeedsYou,
    };
    Some((msg, level))
}

/// `MeetingActivityAttributes.ContentState` as JSON: `gizzi`, `liveSince` (epoch seconds), `listening`.
pub fn content_state(session: &SessionRow) -> Value {
    let gizzi = session.gizzi();
    let over = matches!(gizzi, "ended" | "not_admitted");
    json!({
        "gizzi": gizzi,
        "liveSince": if over { None } else { session.live_at.map(|t| t.timestamp() as f64) },
        "listening": !over && session.listening,
    })
}

/// `MeetingActivityAttributes` as JSON (the push-to-start attributes).
pub fn activity_attributes(session: &SessionRow, meeting: &MeetingRow) -> Value {
    json!({
        "meetingId": meeting.id.to_string(),
        "sessionId": session.id.to_string(),
        "title": name_of(meeting),
        "platformLabel": meeting.platform_label(),
    })
}

/// Live Activity payloads for one session: `(token, priority, payload)`.
pub fn activity_pushes(
    session: &SessionRow,
    meeting: &MeetingRow,
    updates: &[PushTarget],
    starts: &[PushTarget],
    now: i64,
) -> Vec<(PushTarget, u8, Value)> {
    let sid = session.id.to_string();
    let gizzi = session.gizzi();
    let over = matches!(gizzi, "ended" | "not_admitted");
    let mut out = Vec::new();
    let mine: Vec<&PushTarget> = updates.iter().filter(|t| t.attributes_type.as_deref() == Some(ATTRIBUTES) && t.activity_key.as_deref() == Some(sid.as_str())).collect();
    for t in &mine {
        let (event, extras, priority) = if over {
            ("end", LaExtras { dismissal_date: Some(now + DISMISS_SECS), ..LaExtras::default() }, 10)
        } else {
            ("update", LaExtras::default(), if gizzi == "waiting_admit" { 10 } else { 5 })
        };
        out.push(((*t).clone(), priority, la_payload(event, content_state(session), now, &extras)));
    }
    if !over && matches!(gizzi, "joining" | "waiting_admit" | "live") {
        for t in starts.iter().filter(|t| t.attributes_type.as_deref() == Some(ATTRIBUTES)) {
            if mine.iter().any(|m| m.device_id == t.device_id) {
                continue;
            }
            let alert = alert_copy(session, meeting).map(|(title, body, _)| json!({ "title": title, "body": body }));
            let extras = LaExtras { alert, start: Some((ATTRIBUTES, activity_attributes(session, meeting))), ..LaExtras::default() };
            out.push((t.clone(), 10, la_payload("start", content_state(session), now, &extras)));
        }
    }
    out
}

async fn push_state(db: &PgPool, session: &SessionRow, meeting: &MeetingRow, transport: &dyn PushTransport, vapid: Option<&Vapid>, apns_on: bool) {
    let user = &session.user_id;
    if apns_on {
        let updates = apns::activity_targets(db, user, "la_update").await.unwrap_or_default();
        let starts = apns::activity_targets(db, user, "la_start").await.unwrap_or_default();
        let devices = apns::device_targets(db, user).await.unwrap_or_default();
        let starts: Vec<PushTarget> = starts.into_iter().filter(|t| devices.iter().any(|d| d.device_id == t.device_id && d.has("la_push"))).collect();
        for (target, priority, payload) in activity_pushes(session, meeting, &updates, &starts, Utc::now().timestamp()) {
            apns::send_raw(db, "liveactivity", LA_TOPIC_SUFFIX, priority, &payload, &target, &RawOptions::default()).await;
        }
    }
    let Some((msg, level)) = push_message(session, meeting) else { return };
    if notifications::preference(db, user, EVENT).await == Some(false) {
        return;
    }
    if notifications::hold_if_quiet(db, user, EVENT, &msg, level, None, None).await {
        return;
    }
    match vapid {
        Some(v) => {
            web_push::send_to_user(db, transport, v, user, &msg).await;
        }
        None if apns_on => {
            apns::send_to_user(db, user, &msg).await;
        }
        None => {}
    }
}

// ---------------------------------------------------------------------------
// Recap email
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Clerk id when known, else `email:<address>` (the claim key).
    pub claim_id: String,
    pub user_id: Option<String>,
    pub email: Option<String>,
}

/// Who gets the recap: the owner, plus `org_member` participants whose email is on the invite.
/// Guests (and anyone not on the invite) never do.
pub fn recap_candidates(owner: &str, invited: &[String], participants: &[(String, Option<String>, Option<String>)]) -> Vec<Candidate> {
    let invited: Vec<String> = invited.iter().map(|e| e.trim().to_lowercase()).collect();
    let mut out = vec![Candidate { claim_id: owner.to_string(), user_id: Some(owner.to_string()), email: None }];
    for (class, email, user_id) in participants {
        if class != "org_member" {
            continue;
        }
        let Some(email) = email.as_deref().map(|e| e.trim().to_lowercase()).filter(|e| e.contains('@')) else { continue };
        if !invited.contains(&email) || user_id.as_deref() == Some(owner) {
            continue;
        }
        if out.iter().any(|c| c.email.as_deref() == Some(email.as_str())) {
            continue;
        }
        out.push(Candidate { claim_id: user_id.clone().unwrap_or_else(|| format!("email:{email}")), user_id: user_id.clone(), email: Some(email) });
    }
    out
}

async fn recap_wanted(db: &PgPool, user_id: Option<&str>) -> bool {
    let Some(user) = user_id else { return true };
    sqlx::query_scalar::<_, bool>("SELECT recap_email FROM user_meeting_settings WHERE user_id = $1")
        .bind(user)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
}

fn recap_date(meeting: &MeetingRow, session: &SessionRow) -> String {
    let at: DateTime<Utc> = meeting.starts_at.or(session.live_at).or(session.ended_at).unwrap_or(session.created_at);
    at.format("%A, %B %-d, %Y").to_string()
}

/// Send the recap for an ended session with a summary. `owner` is the owner's address (Clerk); `None` skips
/// the owner unless a participant row carries it. Returns the addresses a send was attempted for.
pub async fn send_recap(db: &PgPool, session: &SessionRow, meeting: &MeetingRow, owner: Option<Recipient>) -> Vec<String> {
    let mut sent = Vec::new();
    if !customer_emails::enabled() || session.state != "ended" {
        return sent;
    }
    let Some(summary) = session.summary.as_deref() else { return sent };
    let invited: Vec<String> = sqlx::query_scalar::<_, Value>("SELECT attendees FROM meetings WHERE id = $1")
        .bind(meeting.id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x["email"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default();
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as("SELECT class, email, user_id FROM meeting_participants WHERE session_id = $1")
        .bind(session.id)
        .fetch_all(db)
        .await
        .unwrap_or_default();
    let decisions: Vec<String> = session.decisions.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let items: Vec<(String, Option<String>)> = session
        .action_items
        .as_array()
        .map(|a| a.iter().filter_map(|v| Some((v["text"].as_str()?.to_string(), v["owner"].as_str().map(str::to_string)))).collect())
        .unwrap_or_default();
    let page_url = format!("{PAGE_BASE}/{}", meeting.id);
    let date = recap_date(meeting, session);
    let recap = MeetingRecap { title: &meeting.title, date: &date, summary, decisions: &decisions, action_items: &items, page_url: &page_url };
    let reference = session.id.to_string();
    for c in recap_candidates(&session.user_id, &invited, &rows) {
        let recipient = match (&c.email, c.claim_id == session.user_id) {
            (_, true) => match owner.clone() {
                Some(o) => o,
                None => continue,
            },
            (Some(email), _) => Recipient { email: email.clone(), first_name: None },
            _ => continue,
        };
        if !recap_wanted(db, c.user_id.as_deref()).await {
            continue;
        }
        let email = customer_emails::meeting_recap_email(&recipient, &recap);
        if customer_emails::deliver(db, &c.claim_id, "meeting_recap", &reference, &recipient, &email).await {
            sent.push(recipient.email);
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::notifications::tests::schema;
    use crate::routes::test_support::test_pool;
    use crate::routes::web_push::tests::{browser_sub, decrypt, fixture_keys, Recorder};

    const SWIFT: &str = include_str!("../../../../surfaces/allternit-mobile/ios/Core/LiveActivity/MeetingActivityAttributes.swift");

    async fn meeting_schema(db: &PgPool) {
        schema(db).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/130_meetings.sql")).execute(db).await.expect("130 applies");
        sqlx::raw_sql(
            "CREATE TABLE customer_emails (id bigserial PRIMARY KEY, user_id text NOT NULL, kind text NOT NULL, ref text NOT NULL DEFAULT '', email text, status text NOT NULL, error text, created_at timestamptz NOT NULL DEFAULT now(), UNIQUE (user_id, kind, ref))",
        )
        .execute(db)
        .await
        .unwrap();
    }

    async fn seed(db: &PgPool, owner: &str, state: &str, end_reason: Option<&str>, summary: Option<&str>) -> (SessionRow, MeetingRow) {
        let attendees = json!([{ "email": "ann@corp.com" }, { "email": "bob@corp.com" }, { "email": "guest@out.com" }]);
        let mid: Uuid = sqlx::query_scalar(
            "INSERT INTO meetings (user_id, platform, title, join_url, url_key, source, attendees) VALUES ($1, 'meet', 'Roadmap sync', 'https://meet.google.com/abc-defg-hij', $2, 'link', $3) RETURNING id",
        )
        .bind(owner)
        .bind(format!("meet:{}", Uuid::new_v4()))
        .bind(&attendees)
        .fetch_one(db)
        .await
        .unwrap();
        let sid: Uuid = sqlx::query_scalar(
            "INSERT INTO meeting_sessions (meeting_id, user_id, url_key, state, end_reason, platform, summary, decisions, action_items, live_at) \
             VALUES ($1, $2, $3, $4, $5, 'meet', $6, '[\"Ship Friday\"]', '[{\"text\":\"Send notes\",\"owner\":\"Ann\",\"due\":null}]', now()) RETURNING id",
        )
        .bind(mid)
        .bind(owner)
        .bind(format!("meet:{}", Uuid::new_v4()))
        .bind(state)
        .bind(end_reason)
        .bind(summary)
        .fetch_one(db)
        .await
        .unwrap();
        for (name, email, class, user) in [
            ("Ann", Some("ann@corp.com"), "org_member", Some("u_ann")),
            ("Bob", Some("bob@corp.com"), "org_member", None),
            ("Carl", Some("carl@corp.com"), "org_member", None),
            ("Guest", Some("guest@out.com"), "guest", None),
        ] {
            sqlx::query("INSERT INTO meeting_participants (session_id, display_name, email, user_id, class, joined_at) VALUES ($1, $2, $3, $4, $5, now())")
                .bind(sid)
                .bind(name)
                .bind(email)
                .bind(user)
                .bind(class)
                .execute(db)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE meetings SET latest_session_id = $2 WHERE id = $1").bind(mid).bind(sid).execute(db).await.unwrap();
        (meetings::fetch_session(db, sid).await.unwrap().unwrap(), meetings::fetch_meeting(db, mid).await.unwrap())
    }

    #[test]
    fn recap_candidates_skip_guests_and_people_not_on_the_invite() {
        let invited = vec!["Ann@Corp.com".to_string(), "guest@out.com".to_string()];
        let people = vec![
            ("org_member".to_string(), Some("ann@corp.com".to_string()), None),
            ("guest".to_string(), Some("guest@out.com".to_string()), None),
            ("org_member".to_string(), Some("uninvited@corp.com".to_string()), None),
            ("org_member".to_string(), Some("ann@corp.com".to_string()), None),
        ];
        let got = recap_candidates("u_owner", &invited, &people);
        let emails: Vec<_> = got.iter().map(|c| c.email.clone()).collect();
        assert_eq!(emails, vec![None, Some("ann@corp.com".to_string())], "owner, then the invited org member once");
    }

    /// Needs a sendmail stand-in: a script that appends the envelope recipient to a file.
    #[tokio::test]
    async fn recap_goes_to_owner_and_invited_org_members_never_guests_and_only_once() {
        let dir = std::env::temp_dir().join(format!("recap-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("sent.txt");
        let script = dir.join("sendmail.sh");
        std::fs::write(&script, format!("#!/bin/sh\nfor a; do last=$a; done\ncat > /dev/null\necho \"$last\" >> {}\n", out.display())).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("ALLTERNIT_SENDMAIL", &script);
        std::env::set_var("ALLTERNIT_CUSTOMER_EMAILS", "1");

        let db = test_pool().await;
        meeting_schema(&db).await;
        let owner = Recipient { email: "owner@corp.com".into(), first_name: Some("Olive".into()) };
        let (session, meeting) = seed(&db, "u_owner", "ended", Some("left_by_user"), Some("We agreed to ship.")).await;

        let mut first = send_recap(&db, &session, &meeting, Some(owner.clone())).await;
        first.sort();
        assert_eq!(first, vec!["ann@corp.com", "bob@corp.com", "owner@corp.com"], "carl is not on the invite, the guest is a guest");
        let again = send_recap(&db, &session, &meeting, Some(owner.clone())).await;
        assert!(again.is_empty(), "second run sends nothing");
        let mail = std::fs::read_to_string(&out).unwrap();
        assert_eq!(mail.lines().count(), 3);
        assert!(!mail.contains("guest@out.com") && !mail.contains("carl@corp.com"));

        // The owner's switch off: no recap to the owner for a new session.
        sqlx::query("INSERT INTO user_meeting_settings (user_id, recap_email) VALUES ('u_off', false)").execute(&db).await.unwrap();
        let (s2, m2) = seed(&db, "u_off", "ended", Some("left_by_user"), Some("Notes.")).await;
        let sent = send_recap(&db, &s2, &m2, Some(owner.clone())).await;
        assert!(!sent.contains(&"owner@corp.com".to_string()), "owner opted out: {sent:?}");
        // Not ended, or no summary: nothing.
        let (s3, m3) = seed(&db, "u_x", "live", None, None).await;
        assert!(send_recap(&db, &s3, &m3, Some(owner)).await.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn meeting_push_opens_the_meeting_page_and_reaches_the_owners_browser() {
        let db = test_pool().await;
        meeting_schema(&db).await;
        let (session, meeting) = seed(&db, "u_push", "waiting_admit", None, None).await;
        let (p256dh, auth_b64, key, auth) = browser_sub();
        web_push::subscribe(&db, "u_push", "phone", "https://push.example.com/u_push", &p256dh, &auth_b64, "test").await.unwrap();
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let public = URL_SAFE_NO_PAD.decode(&p256dh).unwrap();
        let (vpub, vpriv) = fixture_keys();
        let vapid = Vapid::new(&vpub, &vpriv, "mailto:test@allternit.com").unwrap();
        let rec = Recorder::default();
        push_state(&db, &session, &meeting, &rec, Some(&vapid), false).await;
        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let payload: Value = serde_json::from_slice(&decrypt(&sent[0].2, &key, &public, &auth)).unwrap();
        assert_eq!(payload["url"], format!("/meetings/{}", meeting.id));
        assert_eq!(payload["title"], "Let Gizzi in");
        drop(sent);

        // The switch off: nothing more goes out.
        sqlx::query("INSERT INTO notification_preferences (user_id, event_type, enabled) VALUES ('u_push', $1, false)").bind(EVENT).execute(&db).await.unwrap();
        push_state(&db, &session, &meeting, &rec, Some(&vapid), false).await;
        assert_eq!(rec.sent.lock().unwrap().len(), 1);
    }

    #[test]
    fn meeting_push_states_and_live_activity_state_match_the_swift_struct() {
        let now = Utc::now();
        let mk = |state: &str, reason: Option<&str>, summary: Option<&str>| SessionRow {
            id: Uuid::new_v4(),
            meeting_id: Uuid::new_v4(),
            user_id: "u".into(),
            org_id: None,
            url_key: "k".into(),
            state: state.into(),
            end_reason: reason.map(str::to_string),
            platform: "meet".into(),
            speak_replies: false,
            listening: true,
            bot_identity: "Gizzi (Allternit)".into(),
            pod_ref: None,
            scheduled_join_at: None,
            joined_at: None,
            live_at: Some(now),
            ended_at: None,
            summary: summary.map(str::to_string),
            decisions: json!(["a"]),
            action_items: json!([]),
            created_at: now,
            updated_at: now,
        };
        // ContentState keys are exactly the Swift struct's stored properties.
        let body = SWIFT.split("struct ContentState").nth(1).unwrap().split("var stateLabel").next().unwrap();
        let mut swift_keys: Vec<String> = body.lines().filter_map(|l| l.trim().strip_prefix("var ")).map(|l| l.split(':').next().unwrap().trim().to_string()).collect();
        swift_keys.sort();
        let live = mk("live", None, None);
        let state = content_state(&live);
        let mut keys: Vec<String> = state.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, swift_keys);
        assert_eq!(state["gizzi"], "live");
        assert!(state["liveSince"].is_f64() && state["listening"] == true);
        let attr_body = SWIFT.split("var meetingId").nth(1).unwrap();
        for k in ["meetingId", "sessionId", "title", "platformLabel"] {
            assert!(k == "meetingId" || attr_body.contains(&format!("var {k}")), "{k}");
        }
        // Not let in is its own state, and ends the activity.
        let nope = mk("ended", Some("not_admitted"), None);
        assert_eq!(content_state(&nope), json!({ "gizzi": "not_admitted", "liveSince": null, "listening": false }));
        let _ = (live, nope);
    }
}
