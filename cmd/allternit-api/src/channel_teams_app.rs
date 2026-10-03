//! Microsoft Teams shared app — the runtime side (ao-teams).
//!
//! Allternit's shared Teams app (one single-tenant Azure Bot + a multi-tenant
//! Entra app, one Teams app package per customer tenant) talks to runtimes
//! through cloud-api: the bot's messaging endpoint lives there, and inbound
//! activities are queued and relayed to this runtime over its relay, waking it
//! if it sleeps. This module is the delivery address
//! (`POST /webhooks/teams-app`): the cloud stamps the owning user in
//! the trusted `x-allternit-user-id` header (only the relay may set it), the
//! activity is normalized with the shared [`crate::channel_transports::teams_normalize`],
//! routed through [`crate::channel_transports::route_inbound`] exactly like
//! every other channel, and replies go back out through
//! [`TeamsAppCloudTransport`], which asks the cloud to send
//! (`POST /api/v1/channels/teams/send`) — the app's secrets never reach here.
//!
//! Mention forms (this provider's hook, per CHANNELS_CONTRACTS): messages that
//! address the app itself ("@Allternit", "@AllternitBot", or Teams' rendered
//! `<at>Allternit</at>` tag) have that token stripped; a following "@name"
//! then routes to that member bot through the shared `route_inbound` logic.
//! The configurable tab's channel → bot binding is the same
//! `channel_account_bots` switchboard the Messaging UI already writes.

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::channel_gateway::{Identity, Inbound, Outbound, PostError, Receipt};
use crate::channel_gateway::ChannelTransport;
use crate::channel_transports::{dispatch_events, teams_normalize, Account, HttpReq, HttpSend};
use crate::db::DbHandle;
use crate::AppState;

/// Trusted header the cloud relay stamps on queued deliveries: the Allternit
/// user the Teams tenant belongs to. Only cloud-api may set it (same contract
/// as `x-allternit-user-id` on agency_forward).
const USER_HEADER: &str = "x-allternit-user-id";
/// Bearer the runtime uses to call cloud-api's teams routes: the paired
/// runtime device token, provided by the host app.
const CLOUD_TOKEN_ENV: &str = "ALLTERNIT_CLOUD_TOKEN";

pub fn teams_app_router() -> Router<Arc<AppState>> {
    // Not under /webhooks/channels/:provider — a static segment sibling of
    // that param route panics at router build on older axum/matchit.
    Router::new().route("/webhooks/teams-app", post(teams_app_webhook))
}

async fn teams_app_webhook(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(owner) = headers.get(USER_HEADER).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing_user_header" }))).into_response();
    };
    let Ok(activity) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response();
    };
    // Ack fast: bot turns can outlive any platform timeout. The queue retries.
    let st = state.clone();
    let owner = owner.to_string();
    tokio::spawn(async move {
        if let (Some(acct), Some(tx)) = (ensure_account(&st.db, &owner), cloud_transport(&st, &owner)) {
            let names: Vec<String> = crate::channel_transports::member_bots(&st.db, &acct).iter().map(|b| b.name.clone()).collect();
            let events = normalize_activity(&activity, &names);
            dispatch_events(&st, &acct, tx, events).await;
        }
    });
    Json(json!({ "ok": true })).into_response()
}

/// Normalize one relayed Bot Framework activity into channel inbounds, with
/// this provider's mention hook applied to message text.
fn normalize_activity(activity: &Value, member_names: &[String]) -> Vec<Inbound> {
    let mut events = teams_normalize(activity);
    for e in &mut events {
        if let Some(text) = e.text.take() {
            e.text = Some(rewrite_app_mention(&text, member_names));
        }
    }
    events
}

/// Strip the addressing of the shared app itself so "@Allternit <name> hi"
/// (or Teams' rendered "<at>Allternit</at> <name> hi") reads as "@name hi"
/// and the shared "@name" member-bot routing picks it up.
fn rewrite_app_mention(text: &str, member_names: &[String]) -> String {
    let mut out = text.to_string();
    loop {
        let Some(start) = out.to_ascii_lowercase().find("<at>allternit") else { break };
        let Some(rel_end) = out[start..].find("</at>") else { break };
        let end = start + rel_end + "</at>".len();
        // Swallow one following separator with the tag.
        let rest = &out[end..];
        let skip = rest.chars().next().map(|c| if c == ' ' || c == '\t' { c.len_utf8() } else { 0 }).unwrap_or(0);
        out.replace_range(start..end + skip, "");
    }
    let mut rest_words: Vec<String> = vec![];
    let mut dropped = false;
    // "@Allternit Scout ..." addresses Scout: the word right after a typed app
    // mention becomes "@Scout" for route_inbound when it names a member bot.
    let mut name_next = false;
    for w in out.split_whitespace() {
        if !dropped {
            let handle = w.trim_start_matches('@').to_ascii_lowercase();
            if handle == "allternit" || handle == "allternitbot" {
                dropped = true;
                name_next = true;
                continue;
            }
        }
        let is_member = |word: &str| {
            let handle = |x: &str| x.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
            member_names.iter().any(|n| handle(n) == handle(word))
        };
        if name_next && !w.starts_with('@') && is_member(w) {
            rest_words.push(format!("@{w}"));
        } else {
            rest_words.push(w.to_string());
        }
        name_next = false;
    }
    rest_words.join(" ")
}

/// The connection row the shared app answers on: `teams_app_connections`
/// (V217) remembers it per owner, and the `provider_account_bindings` row it
/// points at is what the Messaging switchboard turns bots on against. No
/// secret is stored — the shared app's credentials live only in cloud-api.
pub(crate) fn ensure_account(db: &DbHandle, owner: &str) -> Option<Account> {
    let account_id = format!("teams-app:{owner}");
    let conn = db.connect().ok()?;
    let now = crate::agent_gateway_routes::now();
    conn.execute(
        "INSERT OR IGNORE INTO teams_app_connections (owner, account_id, state, created_at, updated_at) VALUES (?1, ?2, 'CONNECTED', ?3, ?3)",
        rusqlite::params![owner, account_id, now],
    )
    .ok()?;
    conn.execute(
        "INSERT OR IGNORE INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, scopes_json, state, created_at, updated_at)
         VALUES (?1, ?2, 'teams', 'channel_oauth', 'teams-app-shared', 'Microsoft Teams (Allternit app)', '[\"messages\"]', 'CONNECTED', ?3, ?3)",
        rusqlite::params![account_id, owner, now],
    )
    .ok()?;
    Some(Account { id: account_id, owner: owner.to_string(), restricted_bot: None, secret: "{}".to_string() })
}

fn cloud_base(state: &AppState) -> Option<String> {
    state.config.cloud_api_url().map(|u| u.trim_end_matches('/').to_string())
}

fn cloud_transport(state: &Arc<AppState>, owner: &str) -> Option<Arc<dyn ChannelTransport>> {
    let base = cloud_base(state)?;
    let token = std::env::var(CLOUD_TOKEN_ENV).ok().filter(|s| !s.is_empty());
    Some(Arc::new(TeamsAppCloudTransport {
        http: Arc::new(crate::channel_transports::ReqwestSend),
        base,
        owner: owner.to_string(),
        token,
    }))
}

/// Outbound for shared-app Teams threads: instead of holding the app's
/// secrets, the runtime asks cloud-api to send
/// (`POST /api/v1/channels/teams/send`, authenticated with the runtime's
/// device token). `out.workspace` carries the Bot Framework `serviceUrl` and
/// `out.channel` the conversation id, exactly as `teams_normalize` produces.
pub struct TeamsAppCloudTransport {
    pub http: Arc<dyn HttpSend>,
    pub base: String,
    pub owner: String,
    pub token: Option<String>,
}

#[async_trait]
impl ChannelTransport for TeamsAppCloudTransport {
    fn provider(&self) -> &'static str {
        "teams"
    }
    fn verify(&self, _secret: &str, _headers: &HeaderMap, _body: &[u8]) -> Result<(), String> {
        Err("the shared Teams app is authenticated at the cloud edge, not on this runtime".into())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        teams_normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        Identity { id: requested.map(str::to_string), exact: true }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        if out.channel.is_empty() {
            return Err(PostError::Rejected("no Teams conversation for this outbound".into()));
        }
        let token = self.token.clone().ok_or_else(|| {
            PostError::Rejected(format!("{CLOUD_TOKEN_ENV} is not set; the runtime cannot ask the cloud to send"))
        })?;
        let mut body = json!({ "conversationId": out.channel, "text": out.text });
        if let Some(service_url) = &out.workspace {
            body["serviceUrl"] = json!(service_url);
        }
        if let Some(name) = &out.identity {
            body["botName"] = json!(name);
        }
        let req = HttpReq {
            url: format!("{}/api/v1/channels/teams/send", self.base.trim_end_matches('/')),
            headers: vec![
                ("authorization".into(), format!("Bearer {token}")),
                ("x-allternit-user-id".into(), self.owner.clone()),
                ("content-type".into(), "application/json".into()),
            ],
            body,
        };
        let resp = self.http.post_json(req).await.map_err(PostError::Uncertain)?;
        match resp.status {
            200..=299 => Ok(Receipt { remote_id: resp.body["id"].as_str().unwrap_or_default().to_string(), relayed: false }),
            429 => Err(PostError::Rejected("rate limited".into())),
            500..=599 => Err(PostError::Uncertain(format!("cloud teams send returned {}", resp.status))),
            s => Err(PostError::Rejected(format!("cloud teams send returned {s}: {}", resp.body))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        reply: Mutex<Option<Result<crate::channel_transports::HttpResp, String>>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<crate::channel_transports::HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            self.reply.lock().unwrap().clone().unwrap_or(Ok(crate::channel_transports::HttpResp { status: 200, body: json!({ "id": "1700000000999" }) }))
        }
    }

    #[test]
    fn strips_the_app_mention_and_keeps_a_bot_mention() {
        assert_eq!(rewrite_app_mention("@Allternit Scout fix the build", &["Scout".to_string(), "Engineer".to_string()]), "@Scout fix the build");
        assert_eq!(rewrite_app_mention("@allternitbot @Engineer hi", &["Scout".to_string(), "Engineer".to_string()]), "@Engineer hi");
        assert_eq!(rewrite_app_mention("<at>Allternit</at> @engineer ping", &["Scout".to_string(), "Engineer".to_string()]), "@engineer ping");
        assert_eq!(rewrite_app_mention("<at>Allternit</at>\t@engineer ping", &[]), "@engineer ping");
        assert_eq!(rewrite_app_mention("@someone else", &["Scout".to_string(), "Engineer".to_string()]), "@someone else");
        assert_eq!(rewrite_app_mention("plain message", &["Scout".to_string(), "Engineer".to_string()]), "plain message");
        assert_eq!(rewrite_app_mention("@Allternit", &["Scout".to_string(), "Engineer".to_string()]), "");
    }

    #[test]
    fn normalized_messages_have_the_mention_hook_applied() {
        let activity = json!({ "type": "message", "id": "1", "timestamp": "t", "serviceUrl": "https://smba.test/amer/",
            "channelId": "msteams", "from": { "id": "29:user" },
            "conversation": { "id": "19:abc@thread.tacv2", "tenantId": "t1" },
            "text": "<at>Allternit</at> @Scout hello there" });
        let events = normalize_activity(&activity, &[]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text.as_deref(), Some("@Scout hello there"));
        assert_eq!(events[0].conversation, "teams:19:abc@thread.tacv2");
        assert_eq!(events[0].workspace.as_deref(), Some("https://smba.test/amer/"));
    }

    #[tokio::test]
    async fn the_shared_app_account_backstops_the_messaging_switchboard() {
        let dir = std::env::temp_dir().join(format!("allternit-ta-acct-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        st.db.connect().unwrap().execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','Scout','m','p',1,'{}')",
            [],
        ).unwrap();
        let acct = ensure_account(&st.db, "user-a").expect("account");
        assert_eq!(acct.id, "teams-app:user-a");
        // Idempotent: a second call changes nothing.
        ensure_account(&st.db, "user-a").unwrap();
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM provider_account_bindings WHERE id = 'teams-app:user-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM teams_app_connections WHERE owner = 'user-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // The Messaging switchboard turns a bot on against it.
        let conn = st.db.connect().unwrap();
        let channels = crate::channel_gateway::set_bot_channel(&conn, "user-a", "bot-1", &acct.id, true, false).unwrap();
        assert_eq!(channels[0]["enabled"], true);
        let bots = crate::channel_transports::member_bots(&st.db, &acct);
        assert_eq!(bots.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), vec!["bot-1"]);
    }

    #[tokio::test]
    async fn a_shared_app_activity_opens_a_thread_on_the_switched_on_bot() {
        let dir = std::env::temp_dir().join(format!("allternit-ta-route-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        st.db.connect().unwrap().execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','Scout','m','p',1,'{}')",
            [],
        ).unwrap();
        let acct = ensure_account(&st.db, "user-a").unwrap();
        let conn = st.db.connect().unwrap();
        crate::channel_gateway::set_bot_channel(&conn, "user-a", "bot-1", &acct.id, true, false).unwrap();

        struct Rt;
        impl crate::thread_routes::ThreadRuntime for Rt {
            async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, id: &str) -> Result<String, String> {
                Ok(format!("sess-{id}"))
            }
            async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
                Ok(())
            }
            async fn handoff(&self, _s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
                Err("no".into())
            }
        }

        let activity = json!({ "type": "message", "id": "1700000000123", "timestamp": "t", "serviceUrl": "https://smba.test/amer/",
            "channelId": "msteams", "from": { "id": "29:user", "name": "Sam" },
            "conversation": { "id": "19:abc@thread.tacv2", "tenantId": "t1" },
            "text": "@Allternit hello" });
        let e = normalize_activity(&activity, &[]).remove(0);
        assert_eq!(e.text.as_deref(), Some("hello"));
        let routed = crate::channel_transports::route_inbound(&st.db, &Rt, &acct, "teams", &e).await.unwrap();
        assert_eq!(routed.recorded, crate::channel_gateway::Recorded::New);
        let (_, bot, text) = routed.turn.expect("the switched-on bot answers");
        assert_eq!(bot, "bot-1");
        assert!(text.contains("hello"));
        let binding = routed.binding.unwrap();
        assert_eq!(binding.provider, "teams");
    }

    #[tokio::test]
    async fn the_cloud_transport_posts_to_the_cloud_send_route() {
        let http = Arc::new(FakeHttp::default());
        let tx = TeamsAppCloudTransport {
            http: http.clone(),
            base: "https://api.allternit.com".into(),
            owner: "user-a".into(),
            token: Some("dev-token".into()),
        };
        let out = Outbound {
            workspace: Some("https://smba.trafficmanager.net/amer/".into()),
            channel: "19:abc@thread.tacv2".into(),
            thread: None,
            text: "hi".into(),
            identity: Some("Scout".into()),
        };
        let receipt = tx.post(&out).await.unwrap();
        assert_eq!(receipt.remote_id, "1700000000999");
        assert!(!receipt.relayed);
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://api.allternit.com/api/v1/channels/teams/send");
        assert!(sent.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer dev-token"));
        assert!(sent.headers.iter().any(|(k, v)| k == "x-allternit-user-id" && v == "user-a"));
        assert_eq!(sent.body["conversationId"], "19:abc@thread.tacv2");
        assert_eq!(sent.body["serviceUrl"], "https://smba.trafficmanager.net/amer/");
        assert_eq!(sent.body["botName"], "Scout");
        assert_eq!(sent.body["text"], "hi");

        // No token: a clear rejection, no request attempted.
        let no_token = TeamsAppCloudTransport { http: http.clone(), base: "https://api.allternit.com".into(), owner: "user-a".into(), token: None };
        assert!(matches!(no_token.post(&out).await, Err(PostError::Rejected(_))));
        // A cloud-side refusal is a definite rejection.
        *http.reply.lock().unwrap() = Some(Ok(crate::channel_transports::HttpResp { status: 404, body: json!({ "error": "conversation reference not found" }) }));
        assert!(matches!(tx.post(&out).await, Err(PostError::Rejected(_))));
    }
}
