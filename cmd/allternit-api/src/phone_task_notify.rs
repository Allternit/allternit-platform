//! Text-line status for phone tasks that started on an SMS thread (`source: "sms"`).
//!
//! A phone task runs for a while; the person who texted it should hear back on
//! the same thread: started, needs approval (with the inbox link), done,
//! denied, timed out. Rules, none of them optional:
//! - Sent only from the per-bot number that owns the thread (the thread's
//!   `channelKey` is `phone:<bot number>:<caller>`; the number must belong to
//!   the thread's bot). No match, no text.
//! - Status only: a short title and the inbox link. Never screen content (no
//!   OCR, app text or screenshots).
//! - Approval happens in the app's inbox. Nothing here reads inbound texts, so
//!   a "yes" or "approve" text approves nothing.
//! - A failed send never fails or slows the task.

use std::sync::Arc;

use rusqlite::params;
use serde_json::Value;

use crate::channel_gateway::{ChannelTransport, Outbound};
use crate::channel_phone::{build_sms, is_e164};
use crate::channel_transports::{pick, HttpSend};
use crate::db::DbHandle;
use crate::phone_guard::TaskCtx;

const TITLE_MAX: usize = 40;
const INBOX_URL: &str = "https://ai.allternit.com/";

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started,
    NeedsApproval { approval_id: String },
    Done,
    Denied,
    TimedOut,
    /// Failed, phone locked or any other end that isn't one of the above.
    Stopped,
}

/// The event for a final run report status.
pub fn event_for_status(status: &str) -> Event {
    match status {
        "done" => Event::Done,
        "denied" => Event::Denied,
        "approval_timeout" => Event::TimedOut,
        _ => Event::Stopped,
    }
}

fn short_title(goal: &str) -> String {
    let one: String = goal.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= TITLE_MAX {
        return one;
    }
    let cut: String = one.chars().take(TITLE_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The inbox link for an approval id.
pub fn inbox_link(approval_id: &str) -> String {
    format!("{INBOX_URL}?allternit_inbox=approval:{approval_id}")
}

/// The text for an event. Short, plain, no screen content.
pub fn status_text(ev: &Event, goal: &str) -> String {
    let t = short_title(goal);
    match ev {
        Event::Started => format!("Started: \"{t}\". I'll text when it needs you or is done."),
        Event::NeedsApproval { approval_id } => format!("Needs your approval: \"{t}\". Open the app inbox to decide: {}", inbox_link(approval_id)),
        Event::Done => format!("Done: \"{t}\"."),
        Event::Denied => format!("Stopped: \"{t}\" was denied. Nothing more was done."),
        Event::TimedOut => format!("Timed out waiting for approval on \"{t}\". Nothing was done. Ask again to retry."),
        Event::Stopped => format!("Couldn't finish \"{t}\". Check the app for details."),
    }
}

/// Where a status text goes: the sealed-secret JSON of the owning number's account and the caller to text.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub secret: String,
    pub to: String,
}

/// The route for a task, from the sealed SMS account `secrets`. `None` for a
/// task that isn't an SMS task, has no thread, or whose thread isn't on a
/// number owned by the thread's own bot.
pub fn route_with(db: &DbHandle, ctx: &TaskCtx, secrets: &[String]) -> Option<Route> {
    if ctx.source != "sms" {
        return None;
    }
    let thread = ctx.thread_id.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
    let conn = db.connect().ok()?;
    let (bot, origin): (String, Option<String>) = conn.query_row("SELECT bot_id, origin FROM bot_threads WHERE id = ?1", params![thread], |r| Ok((r.get(0)?, r.get(1)?))).ok()?;
    if ctx.bot_id.as_deref().is_some_and(|b| !b.is_empty() && b != bot) {
        return None;
    }
    let key = serde_json::from_str::<Value>(&origin?).ok()?.get("channelKey")?.as_str()?.to_string();
    let rest = key.strip_prefix("phone:")?;
    let (bot_number, caller) = rest.split_once(':')?;
    if !is_e164(bot_number) || !is_e164(caller) {
        return None;
    }
    let number_id: String = conn.query_row("SELECT number_id FROM channel_phone_numbers WHERE e164 = ?1 AND bot_id = ?2", params![bot_number, bot], |r| r.get(0)).ok()?;
    let secret = secrets.iter().find(|s| pick(s, "numberId") == number_id)?.clone();
    Some(Route { secret, to: caller.to_string() })
}

/// [`route_with`] over the runtime's real sealed SMS accounts.
pub fn route(db: &DbHandle, ctx: &TaskCtx) -> Option<Route> {
    if ctx.source != "sms" {
        return None;
    }
    let secrets: Vec<String> = crate::channel_transports::accounts(db, "sms", None).into_iter().map(|a| a.secret).collect();
    route_with(db, ctx, &secrets)
}

/// Send one status text. Errors are logged and dropped.
pub async fn send(route: &Route, http: Arc<dyn HttpSend>, text: &str) {
    let tx = build_sms(http, &route.secret).with_runtime_token(crate::phone_sync::runtime_bearer());
    let out = Outbound { workspace: None, channel: String::new(), thread: Some(route.to.clone()), text: text.to_string(), identity: None };
    if let Err(e) = tx.post(&out).await {
        eprintln!("phone_task_notify: status text not sent: {e:?}");
    }
}

/// Text the status for `ev` when the task is an SMS task. No-op otherwise.
pub async fn notify(db: &DbHandle, ctx: &TaskCtx, http: Arc<dyn HttpSend>, ev: &Event, goal: &str) {
    if let Some(r) = route(db, ctx) {
        send(&r, http, &status_text(ev, goal)).await;
    }
}

/// While a run waits on an approval, text the inbox link once per approval.
/// Abort the returned handle when the run ends. `None` for a non-SMS task.
pub fn watch_approvals(db: &DbHandle, ctx: &TaskCtx, http: Arc<dyn HttpSend>, run_id: &str, goal: &str) -> Option<tokio::task::JoinHandle<()>> {
    let r = route(db, ctx)?;
    let (run_id, goal) = (run_id.to_string(), goal.to_string());
    Some(tokio::spawn(async move {
        let mut seen: Option<String> = None;
        loop {
            let live = crate::phone_guard::live_status(&run_id);
            let waiting = live.as_ref().filter(|l| l["status"].as_str() == Some("waiting_approval")).and_then(|l| l["approvalId"].as_str().map(str::to_string));
            if let Some(id) = waiting {
                if seen.as_deref() != Some(id.as_str()) {
                    send(&r, http.clone(), &status_text(&Event::NeedsApproval { approval_id: id.clone() }, &goal)).await;
                    seen = Some(id);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }))
}

/// The HTTP sender production uses.
pub fn http() -> Arc<dyn HttpSend> {
    Arc::new(crate::channel_transports::ReqwestSend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_phone::{sms_normalize, upsert_number};
    use crate::channel_transports::{HttpReq, HttpResp};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            let n = {
                let mut s = self.sent.lock().unwrap();
                s.push(req);
                s.len()
            };
            Ok(HttpResp { status: 200, body: json!({ "messageId": format!("m{n}") }) })
        }
    }

    fn telnyx(from: &str, to: &str, text: &str) -> Value {
        json!({ "data": { "event_type": "message.received", "payload": { "id": "t1", "direction": "inbound", "from": { "phone_number": from }, "to": [{ "phone_number": to }], "text": text } } })
    }

    #[tokio::test]
    async fn an_sms_task_texts_each_status_and_a_text_cannot_approve() {
        let dir = std::env::temp_dir().join(format!("phone-notify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        upsert_number(&st.db, "num-1", "user-a", "bot-1", "+14155550100", None).unwrap();
        // The owner's text arrives and normalizes to the thread's conversation key.
        let inbound = sms_normalize(&telnyx("+14155550123", "+14155550100", "open settings on my phone")).remove(0);
        c.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, title, status, origin, last_activity_at, created_at, updated_at) VALUES ('th-1','user-a','bot-1','Phone','idle',?1,'t','t','t')",
            params![json!({ "channel": "phone", "channelKey": inbound.conversation }).to_string()],
        )
        .unwrap();
        c.execute("INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, authority, action, detail_json, state, created_at) VALUES ('gap_1','user-a','th-1','bot-1','allternit','send','{}','pending','t')", []).unwrap();
        let ctx = TaskCtx { owner: "user-a".into(), bot_id: Some("bot-1".into()), thread_id: Some("th-1".into()), session_id: None, source: "sms".into() };
        let secret = json!({ "numberId": "num-1", "cloudUrl": "https://cloud.test", "token": "tok" }).to_string();
        let r = route_with(&st.db, &ctx, &[secret.clone()]).expect("route");
        assert_eq!(r.to, "+14155550123");

        let goal = inbound.text.clone().unwrap();
        let http = Arc::new(FakeHttp::default());
        for ev in [Event::Started, Event::NeedsApproval { approval_id: "gap_1".into() }, Event::Done, Event::Denied, Event::TimedOut] {
            send(&r, http.clone(), &status_text(&ev, &goal)).await;
        }
        let sent = http.sent.lock().unwrap();
        let texts: Vec<String> = sent.iter().map(|q| q.body["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(texts[0], "Started: \"open settings on my phone\". I'll text when it needs you or is done.");
        assert_eq!(texts[1], "Needs your approval: \"open settings on my phone\". Open the app inbox to decide: https://ai.allternit.com/?allternit_inbox=approval:gap_1");
        assert_eq!(texts[2], "Done: \"open settings on my phone\".");
        assert_eq!(texts[3], "Stopped: \"open settings on my phone\" was denied. Nothing more was done.");
        assert_eq!(texts[4], "Timed out waiting for approval on \"open settings on my phone\". Nothing was done. Ask again to retry.");
        // Every text goes from the owning number to the caller, and fits one SMS.
        for q in sent.iter() {
            assert_eq!((q.body["numberId"].as_str(), q.body["to"].as_str()), (Some("num-1"), Some("+14155550123")));
        }
        assert!(texts.iter().all(|t| t.chars().count() <= 160), "{texts:?}");
        assert_eq!(event_for_status("approval_timeout"), Event::TimedOut);
        drop(sent);

        // An inbound "approve" text is just a message: nothing here reads it, the hold stays pending.
        let approve = sms_normalize(&telnyx("+14155550123", "+14155550100", "approve")).remove(0);
        assert_eq!(approve.text.as_deref(), Some("approve"));
        let state: String = st.db.connect().unwrap().query_row("SELECT state FROM gateway_approvals WHERE id = 'gap_1'", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "pending");

        // No route: not an SMS task, another bot's number, or no sealed account for the number.
        assert!(route_with(&st.db, &TaskCtx { source: "chat".into(), ..ctx.clone() }, &[secret.clone()]).is_none());
        assert!(route_with(&st.db, &TaskCtx { bot_id: Some("bot-2".into()), ..ctx.clone() }, &[secret.clone()]).is_none());
        assert!(route_with(&st.db, &ctx, &[json!({ "numberId": "num-9" }).to_string()]).is_none());
    }

    #[test]
    fn long_titles_are_cut_and_carry_no_newlines() {
        let t = status_text(&Event::Done, "open\nsettings and then do a very long list of things on the phone that goes on");
        assert!(!t.contains('\n') && t.contains('…') && t.chars().count() < 70);
    }
}
