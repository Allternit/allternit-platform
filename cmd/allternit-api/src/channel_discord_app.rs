//! Discord through Allternit's one shared app (runtime side).
//!
//! The app's bot token lives only in cloud-api. This runtime never talks to
//! Discord for a shared-app connection: [`DiscordAppTransport::post`] asks the
//! cloud to send, `POST /api/v1/channels/discord/send`, authenticated with the
//! runtime-device token. The cloud posts through an app-owned webhook so each
//! bot speaks under its own name and avatar. The user-token Discord path in
//! `channel_transports` / `discord_gateway` stays as "Advanced".
//!
//! A connection is in app mode when its sealed secret has `"mode":"app"`
//! (plus optional `guildId`, `cloudToken`, `publicKey`, `relaySecret`).
//!
//! Mentions reach a bot three ways, all turned into the "@name" form that
//! `channel_transports::route_inbound` already understands, by [`rewrite`]
//! and [`DiscordAppTransport::normalize`]:
//!   * "@Allternit <name> ..." (Discord sends it as `<@appid> <name> ...`)
//!   * the slash command `/<name> [message]` (an interaction, type 2)
//!   * a reply to a message a bot posted (`message_reference`, mapped by
//!     message id through `discord_app_messages`)
//!
//! API: https://discord.com/developers/docs/resources/webhook#execute-webhook
//! (username, avatar_url, thread_id), https://discord.com/developers/docs/interactions/application-commands

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::channel_gateway::*;
use crate::channel_transports::{discord_normalize, mentioned_bot, pick, Account, HttpReq, HttpSend, MemberBot};
use crate::db::DbHandle;

static DB: OnceLock<DbHandle> = OnceLock::new();

/// Register the runtime database (called once at boot; [`rewrite`] also does it).
pub fn init(db: DbHandle) {
    let _ = DB.set(db);
}

/// True when the connection's secret selects the shared app.
pub fn is_app_secret(secret: &str) -> bool {
    pick(secret, "mode") == "app"
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Discord rejects webhook names containing "discord" or "clyde".
pub(crate) fn safe_name(name: &str) -> String {
    let mut out = name.to_string();
    for banned in ["discord", "clyde"] {
        while let Some(i) = out.to_lowercase().find(banned) {
            out.replace_range(i..i + banned.len(), "");
        }
    }
    let out: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out: String = out.chars().take(80).collect();
    if out.is_empty() { "Allternit".into() } else { out }
}

pub struct DiscordAppTransport {
    pub http: Arc<dyn HttpSend>,
    pub db: Option<DbHandle>,
    pub cloud_base: Option<String>,
    pub cloud_token: Option<String>,
    pub guild_id: Option<String>,
    pub public_key: Option<String>,
    pub relay_secret: Option<String>,
}

impl DiscordAppTransport {
    pub fn from_secret(secret: &str, http: Arc<dyn HttpSend>) -> Self {
        let some = |s: String| Some(s).filter(|s| !s.is_empty());
        let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Self {
            http,
            db: DB.get().cloned(),
            cloud_base: env("ALLTERNIT_CLOUD_API_URL"),
            cloud_token: some(pick(secret, "cloudToken")).or_else(|| env("ALLTERNIT_RUNTIME_DEVICE_TOKEN")),
            guild_id: some(pick(secret, "guildId")),
            public_key: some(pick(secret, "publicKey")).or_else(|| env("ALLTERNIT_DISCORD_APP_PUBLIC_KEY")),
            relay_secret: some(pick(secret, "relaySecret")),
        }
    }

    /// Bot name (and avatar) for an outbound post. `identity` names a bot
    /// explicitly; otherwise a leading "Name: " (added when several bots share
    /// the connection) or the conversation's own bot.
    fn speaker(&self, out: &Outbound) -> (String, Option<String>, String) {
        let Some(db) = self.db.as_ref().or(DB.get()) else {
            return (safe_name(out.identity.as_deref().unwrap_or("Allternit")), None, out.text.clone());
        };
        let Ok(conn) = db.connect() else { return (safe_name(out.identity.as_deref().unwrap_or("Allternit")), None, out.text.clone()) };
        let binding = find_binding(db, "discord", &format!("discord:{}", out.channel));
        let owner = binding.as_ref().map(|b| b.owner.clone());
        let bots: Vec<(String, Option<String>)> = owner
            .as_ref()
            .and_then(|o| {
                conn.prepare("SELECT COALESCE(NULLIF(name, ''), id), avatar FROM agents WHERE user_id = ?1")
                    .and_then(|mut q| q.query_map(params![o], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect())
                    .ok()
            })
            .unwrap_or_default();
        let (mut name, mut text) = (out.identity.clone(), out.text.clone());
        if name.is_none() {
            if let Some((head, rest)) = out.text.split_once(": ") {
                if bots.iter().any(|(n, _)| clean(n) == clean(head)) {
                    name = Some(head.to_string());
                    text = rest.to_string();
                }
            }
        }
        if name.is_none() {
            name = binding.as_ref().and_then(|b| {
                conn.query_row("SELECT COALESCE(NULLIF(a.name, ''), a.id) FROM bot_threads t JOIN agents a ON a.id = t.bot_id WHERE t.id = ?1", params![b.thread_id], |r| r.get::<_, String>(0)).ok()
            });
        }
        let name = name.unwrap_or_else(|| "Allternit".into());
        let avatar = bots.iter().find(|(n, _)| clean(n) == clean(&name)).and_then(|(_, a)| a.clone()).filter(|a| a.starts_with("https://"));
        (safe_name(&name), avatar, text)
    }

    fn remember(&self, message_id: &str, channel: &str, bot: &str) {
        let Some(db) = self.db.as_ref().or(DB.get()) else { return };
        let Ok(conn) = db.connect() else { return };
        let _ = conn.execute("INSERT OR REPLACE INTO discord_app_messages (message_id, channel_id, bot_name) VALUES (?1, ?2, ?3)", params![message_id, channel, bot]);
        let _ = conn.execute("DELETE FROM discord_app_messages WHERE created_at < datetime('now', '-30 days')", []);
    }
}

/// Bot a replied-to message belongs to.
fn bot_of_message(db: &DbHandle, message_id: &str) -> Option<String> {
    db.connect().ok()?.query_row("SELECT bot_name FROM discord_app_messages WHERE message_id = ?1", params![message_id], |r| r.get(0)).ok()
}

/// Slash command `/<name> [text]` (an interaction, type 2) as a message "@name text".
fn interaction_events(p: &Value) -> Vec<Inbound> {
    if p["type"].as_i64() != Some(2) {
        return vec![];
    }
    let (Some(name), Some(channel)) = (p.pointer("/data/name").and_then(Value::as_str), p["channel_id"].as_str()) else { return vec![] };
    let rest = p
        .pointer("/data/options")
        .and_then(Value::as_array)
        .and_then(|o| o.iter().find_map(|x| x["value"].as_str()))
        .unwrap_or("");
    let id = p["id"].as_str().unwrap_or_default().to_string();
    let mut e = Inbound {
        kind: InboundKind::Message,
        workspace: p["guild_id"].as_str().map(str::to_string),
        channel: channel.to_string(),
        conversation: format!("discord:{channel}"),
        thread: None,
        remote_id: id.clone(),
        message_id: id.clone(),
        text: Some(format!("@{} {rest}", name.replace(' ', "")).trim().to_string()),
        user: p.pointer("/member/user/id").or_else(|| p.pointer("/user/id")).and_then(Value::as_str).map(str::to_string),
        reaction: None,
        added: None,
        cursor: Some(id),
        own: false,
    };
    // "/name" with nothing else still needs text to start a turn.
    if rest.is_empty() {
        e.text = Some(format!("@{} hi", name.replace(' ', "")));
    }
    vec![e]
}

/// Rewrite a Discord message so its target bot is an "@name" at the front.
/// No-op for connections that are not in app mode.
pub fn rewrite(db: &DbHandle, acct: &Account, bots: &[MemberBot], e: &Inbound) -> Inbound {
    init(db.clone());
    let mut e = e.clone();
    if !is_app_secret(&acct.secret) || e.kind != InboundKind::Message || e.own {
        return e;
    }
    let text = e.text.clone().unwrap_or_default();
    if mentioned_bot(&text, bots).is_some() {
        return e;
    }
    // "<@appid> name rest" or "@Allternit name rest": drop the app mention, find a bot name at the front.
    let stripped = {
        let t = text.trim_start();
        if t.starts_with("<@") {
            t.split_once('>').map(|(_, r)| r.trim_start().to_string())
        } else {
            t.strip_prefix("@Allternit").or_else(|| t.strip_prefix("@allternit")).map(|r| r.trim_start().to_string())
        }
    };
    if let Some(rest) = stripped {
        let words: Vec<&str> = rest.split_whitespace().collect();
        for n in (1..=words.len().min(4)).rev() {
            let cand = clean(&words[..n].join(" "));
            if let Some(b) = bots.iter().find(|b| clean(&b.name) == cand || clean(&b.id) == cand) {
                e.text = Some(format!("@{} {}", clean(&b.name), words[n..].join(" ")).trim().to_string());
                return e;
            }
        }
        e.text = Some(rest);
        return e;
    }
    e
}

#[async_trait]
impl ChannelTransport for DiscordAppTransport {
    fn provider(&self) -> &'static str {
        "discord"
    }

    /// A relayed delivery is accepted when it carries either Discord's own
    /// Ed25519 signature (interactions) checked against the app's public key,
    /// or an HMAC-SHA256 of the body (`x-allternit-relay-signature`, hex)
    /// under the connection's `relaySecret` (gateway events).
    fn verify(&self, _secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
        if let (Some(sig), Some(secret)) = (h("x-allternit-relay-signature"), self.relay_secret.as_deref()) {
            let given = hex::decode(sig.trim()).map_err(|_| "bad relay signature encoding")?;
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| "bad relay secret")?;
            mac.update(body);
            return mac.verify_slice(&given).map_err(|_| "relay signature mismatch".to_string());
        }
        let key = self.public_key.clone().ok_or("no Discord app public key configured")?;
        crate::channel_transports::DiscordTransport { http: self.http.clone(), webhook_url: None, own_identity: None }.verify(&json!({ "publicKey": key }).to_string(), headers, body)
    }

    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        let mut events = interaction_events(payload);
        if events.is_empty() {
            events = discord_normalize(payload);
            // A reply to a message one of our bots posted goes to that bot.
            if let (Some(db), Some(reply_to)) = (self.db.as_ref().or(DB.get()), payload.pointer("/d/message_reference/message_id").and_then(Value::as_str)) {
                if let Some(bot) = bot_of_message(db, reply_to) {
                    for e in events.iter_mut().filter(|e| e.kind == InboundKind::Message) {
                        e.text = e.text.as_ref().map(|t| format!("@{} {t}", clean(&bot)));
                    }
                }
            }
        }
        events
    }

    fn identity(&self, requested: Option<&str>) -> Identity {
        // Every bot speaks under its own name through the app's webhook.
        Identity { id: requested.map(str::to_string), exact: requested.is_none() }
    }

    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let (Some(base), Some(token)) = (self.cloud_base.as_deref(), self.cloud_token.as_deref()) else {
            return Err(PostError::Rejected("discord_not_configured".into()));
        };
        let guild = out.workspace.clone().or_else(|| self.guild_id.clone()).ok_or_else(|| PostError::Rejected("no Discord server for this conversation".into()))?;
        let (bot, avatar, text) = self.speaker(out);
        let mut body = json!({ "guildId": guild, "channelId": out.channel, "botName": bot, "text": text });
        if let Some(t) = out.thread.as_ref().filter(|t| !t.is_empty()) {
            body["threadId"] = json!(t);
        }
        if let Some(a) = avatar {
            body["avatarUrl"] = json!(a);
        }
        let url = format!("{}/api/v1/channels/discord/send", base.trim_end_matches('/'));
        let resp = self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await.map_err(PostError::Uncertain)?;
        match resp.status {
            200..=299 => {
                let id = resp.body["messageId"].as_str().map(str::to_string).ok_or_else(|| PostError::Uncertain("the cloud accepted the post but returned no messageId".into()))?;
                self.remember(&id, &out.channel, &bot);
                Ok(Receipt { remote_id: id, relayed: true })
            }
            503 => Err(PostError::Rejected(resp.body["error"].as_str().unwrap_or("discord_not_configured").to_string())),
            429 => Err(PostError::Rejected("rate limited".into())),
            500..=599 => Err(PostError::Uncertain(format!("cloud returned {}", resp.status))),
            s => Err(PostError::Rejected(format!("cloud returned {s}: {}", resp.body))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::channel_transports::HttpResp;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        status: Mutex<Option<u16>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            let status = self.status.lock().unwrap().unwrap_or(200);
            Ok(HttpResp { status, body: if status == 200 { json!({ "messageId": "m-100" }) } else { json!({ "error": "discord_not_configured" }) } })
        }
    }

    fn bots() -> Vec<MemberBot> {
        vec![MemberBot { id: "b1".into(), name: "Live Check".into() }, MemberBot { id: "b2".into(), name: "Finance".into() }]
    }
    fn acct() -> Account {
        Account { id: "a".into(), owner: "u".into(), restricted_bot: None, secret: json!({ "mode": "app" }).to_string() }
    }
    fn msg(text: &str) -> Inbound {
        discord_normalize(&json!({ "t": "MESSAGE_CREATE", "s": 1, "d": { "id": "9", "channel_id": "c1", "guild_id": "g1", "content": text, "author": { "id": "u1" } } })).remove(0)
    }
    async fn db(tag: &str) -> DbHandle {
        let dir = std::env::temp_dir().join(format!("allternit-dapp-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await.db.clone()
    }

    fn transport(http: Arc<FakeHttp>, db: Option<DbHandle>) -> DiscordAppTransport {
        DiscordAppTransport { http, db, cloud_base: Some("https://api.test/".into()), cloud_token: Some("allternit_runtime_x".into()), guild_id: None, public_key: None, relay_secret: Some("s3".into()) }
    }

    #[tokio::test]
    async fn the_mention_hook_picks_the_right_bot() {
        let d = db("hook").await;
        let r = |t: &str| rewrite(&d, &acct(), &bots(), &msg(t)).text.unwrap();
        assert_eq!(r("<@123> finance what is margin?"), "@finance what is margin?");
        assert_eq!(r("<@!123> Live Check ping"), "@livecheck ping");
        assert_eq!(r("@Allternit finance hello"), "@finance hello");
        assert_eq!(r("<@123> nobody home"), "nobody home");
        // Not in app mode: untouched.
        let mut plain = acct();
        plain.secret = "{}".into();
        assert_eq!(rewrite(&d, &plain, &bots(), &msg("<@123> finance x")).text.unwrap(), "<@123> finance x");
    }

    #[test]
    fn a_slash_command_becomes_an_at_mention() {
        let t = transport(Arc::new(FakeHttp::default()), None);
        let evs = t.normalize(&json!({ "type": 2, "id": "i1", "channel_id": "c1", "guild_id": "g1", "member": { "user": { "id": "u1" } }, "data": { "name": "finance", "options": [{ "name": "message", "value": "what is margin?" }] } }));
        assert_eq!(evs[0].text.as_deref(), Some("@finance what is margin?"));
        assert_eq!(evs[0].conversation, "discord:c1");
        assert_eq!(evs[0].workspace.as_deref(), Some("g1"));
    }

    #[tokio::test]
    async fn the_post_goes_through_the_cloud_with_the_bots_name_and_a_reply_maps_back() {
        let d = db("post").await;
        let http = Arc::new(FakeHttp::default());
        let t = transport(http.clone(), Some(d.clone()));
        let out = Outbound { workspace: Some("g1".into()), channel: "c1".into(), thread: Some("t9".into()), text: "hi".into(), identity: Some("Finance".into()) };
        let r = t.post(&out).await.unwrap();
        assert_eq!(r.remote_id, "m-100");
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].url, "https://api.test/api/v1/channels/discord/send");
        assert_eq!(sent[0].headers[0], ("Authorization".into(), "Bearer allternit_runtime_x".into()));
        assert_eq!(sent[0].body, json!({ "guildId": "g1", "channelId": "c1", "threadId": "t9", "botName": "Finance", "text": "hi" }));
        drop(sent);
        // A reply to message m-100 is routed to Finance.
        let evs = t.normalize(&json!({ "t": "MESSAGE_CREATE", "s": 2, "d": { "id": "10", "channel_id": "c1", "guild_id": "g1", "content": "and tax?", "author": { "id": "u1" }, "message_reference": { "message_id": "m-100" } } }));
        assert_eq!(evs[0].text.as_deref(), Some("@finance and tax?"));
        // Unknown replied-to message: unchanged.
        let evs = t.normalize(&json!({ "t": "MESSAGE_CREATE", "s": 3, "d": { "id": "11", "channel_id": "c1", "content": "x", "author": { "id": "u1" }, "message_reference": { "message_id": "other" } } }));
        assert_eq!(evs[0].text.as_deref(), Some("x"));
    }

    #[tokio::test]
    async fn unconfigured_cloud_is_a_definite_rejection() {
        let http = Arc::new(FakeHttp::default());
        let out = Outbound { workspace: Some("g1".into()), channel: "c1".into(), thread: None, text: "hi".into(), identity: None };
        let mut t = transport(http.clone(), None);
        *http.status.lock().unwrap() = Some(503);
        assert_eq!(t.post(&out).await, Err(PostError::Rejected("discord_not_configured".into())));
        t.cloud_base = None;
        assert!(matches!(t.post(&out).await, Err(PostError::Rejected(_))));
        assert!(http.sent.lock().unwrap().len() == 1);
    }

    #[test]
    fn webhook_names_never_contain_discord_or_clyde() {
        assert_eq!(safe_name("Discord Helper"), "Helper");
        assert_eq!(safe_name("CLYDE"), "Allternit");
        assert_eq!(safe_name("Finance"), "Finance");
    }

    #[test]
    fn relayed_events_need_a_valid_signature() {
        let t = transport(Arc::new(FakeHttp::default()), None);
        let body = br#"{"t":"MESSAGE_CREATE"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"s3").unwrap();
        mac.update(body);
        let mut h = HeaderMap::new();
        h.insert("x-allternit-relay-signature", hex::encode(mac.finalize().into_bytes()).parse().unwrap());
        assert!(t.verify("", &h, body).is_ok());
        assert!(t.verify("", &h, b"tampered").is_err());
        assert!(t.verify("", &HeaderMap::new(), body).is_err());
    }
}
