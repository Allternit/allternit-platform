//! Discord direct messages from a bot, through the shared Allternit app.
//!
//! The app's bot token lives only in cloud-api, so the runtime asks the cloud:
//! `POST /api/v1/channels/discord/dm` `{ guildId?, userId, text?, botName? }`
//! (device-token auth, like `/send`). Without `text` the cloud only opens the
//! DM channel and answers `{ channelId }`, which is how a conversation start
//! learns the channel its replies will arrive on. With `text` it also posts and
//! answers `{ channelId, messageId }`.
//!
//! A DM conversation is bound as `discord:<dm channel id>` with the workspace
//! `@dm:<user id>`; [`dm_user`] reads that back so `DiscordAppTransport::post`
//! sends through here instead of the channel webhook.

use serde_json::{json, Value};

use crate::channel_transports::{pick, HttpReq, HttpSend};

/// `Outbound.workspace` / binding workspace of a Discord DM conversation.
const DM_WORKSPACE: &str = "@dm:";

pub fn dm_workspace(user_id: &str) -> String {
    format!("{DM_WORKSPACE}{user_id}")
}

/// The Discord user a DM conversation's workspace names.
pub fn dm_user(workspace: Option<&str>) -> Option<&str> {
    workspace.and_then(|w| w.strip_prefix(DM_WORKSPACE)).filter(|u| !u.is_empty())
}

/// Why the cloud would not open or send a DM.
#[derive(Debug, Clone, PartialEq)]
pub enum DmError {
    /// The cloud has no Discord app configured (503) or this runtime has no cloud address/token.
    NotConfigured,
    /// No Discord server of the user's has the Allternit app installed.
    NotInstalled,
    /// The person isn't in a server where the app is installed (the bot may only DM people it shares a server with).
    NotInServer,
    /// The person's privacy settings refuse DMs from this app (Discord error 50007).
    Closed,
    RateLimited,
    /// The cloud answered with a server error or nothing: the DM may or may not exist.
    Uncertain(String),
    Rejected(String),
}

pub fn cloud_of(secret: &str) -> Option<(String, String)> {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let token = Some(pick(secret, "cloudToken")).filter(|s| !s.is_empty()).or_else(|| env("ALLTERNIT_RUNTIME_DEVICE_TOKEN"))?;
    Some((env("ALLTERNIT_CLOUD_API_URL")?, token))
}

/// Interpret the cloud's answer; `Ok` is the body of a 2xx.
pub fn judge(status: u16, body: &Value) -> Result<Value, DmError> {
    let code = body["error"].as_str().unwrap_or_default();
    match status {
        200..=299 => Ok(body.clone()),
        503 => Err(DmError::NotConfigured),
        404 if code == "discord_not_installed" => Err(DmError::NotInstalled),
        403 if code == "user_not_in_server" => Err(DmError::NotInServer),
        403 | 400 if code == "dm_closed" => Err(DmError::Closed),
        429 => Err(DmError::RateLimited),
        500..=599 => Err(DmError::Uncertain(format!("cloud returned {status}"))),
        s => Err(DmError::Rejected(format!("cloud returned {s}: {code}"))),
    }
}

/// POST the DM request to the cloud and interpret the answer.
pub async fn call(http: &dyn HttpSend, secret: &str, body: Value) -> Result<Value, DmError> {
    let Some((base, token)) = cloud_of(secret) else { return Err(DmError::NotConfigured) };
    let url = format!("{}/api/v1/channels/discord/dm", base.trim_end_matches('/'));
    let resp = http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await.map_err(DmError::Uncertain)?;
    judge(resp.status, &resp.body)
}

/// Open (or find) the DM channel with `user_id`. Sends nothing.
pub async fn open(http: &dyn HttpSend, secret: &str, user_id: &str) -> Result<String, DmError> {
    let mut body = json!({ "userId": user_id });
    let guild = pick(secret, "guildId");
    if !guild.is_empty() {
        body["guildId"] = json!(guild);
    }
    let v = call(http, secret, body).await?;
    v["channelId"].as_str().map(str::to_string).ok_or_else(|| DmError::Uncertain("the cloud opened the DM but returned no channelId".into()))
}

/// The word the runtime shows for each failure, and the code the start route answers with.
pub fn sentence(e: &DmError) -> (&'static str, String) {
    match e {
        DmError::NotConfigured => ("discord_not_configured", "Discord isn't set up on this computer yet.".into()),
        DmError::NotInstalled => ("not_connected", "The Allternit Discord app isn't installed in any of your servers. Connect Discord first.".into()),
        DmError::NotInServer => ("discord_user_not_in_server", "That person isn't in a Discord server where the Allternit app is installed, so the bot can't message them.".into()),
        DmError::Closed => ("discord_dm_closed", "That person doesn't accept direct messages from this server's apps. They can allow them in their Discord privacy settings.".into()),
        DmError::RateLimited => ("channel_rejected", "Discord is rate limiting direct messages right now. Try again in a moment.".into()),
        DmError::Uncertain(why) | DmError::Rejected(why) => ("channel_rejected", why.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::HttpResp;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct Fake {
        sent: Mutex<Vec<HttpReq>>,
        reply: (u16, Value),
    }
    #[async_trait]
    impl HttpSend for Fake {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            Ok(HttpResp { status: self.reply.0, body: self.reply.1.clone() })
        }
    }

    fn secret() -> String {
        json!({ "mode": "app", "guildId": "g1", "cloudToken": "allternit_runtime_x" }).to_string()
    }

    #[test]
    fn the_dm_workspace_round_trips() {
        assert_eq!(dm_user(Some(&dm_workspace("42"))), Some("42"));
        assert_eq!(dm_user(Some("g1")), None);
        assert_eq!(dm_user(Some("@dm:")), None);
        assert_eq!(dm_user(None), None);
    }

    #[tokio::test]
    async fn open_asks_the_cloud_with_the_guild_and_returns_the_channel() {
        let _cloud_url = crate::test_helpers::cloud_url_env(Some("https://api.test/"));
        let http = Fake { sent: Mutex::new(vec![]), reply: (200, json!({ "channelId": "dm-9" })) };
        assert_eq!(open(&http, &secret(), "42").await.unwrap(), "dm-9");
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://api.test/api/v1/channels/discord/dm");
        assert_eq!(sent.body, json!({ "userId": "42", "guildId": "g1" }));
        assert_eq!(sent.headers, vec![("Authorization".to_string(), "Bearer allternit_runtime_x".to_string())]);
    }

    #[test]
    fn cloud_answers_become_plain_failures() {
        assert_eq!(judge(503, &json!({ "error": "discord_not_configured" })), Err(DmError::NotConfigured));
        assert_eq!(judge(404, &json!({ "error": "discord_not_installed" })), Err(DmError::NotInstalled));
        assert_eq!(judge(403, &json!({ "error": "user_not_in_server" })), Err(DmError::NotInServer));
        assert_eq!(judge(403, &json!({ "error": "dm_closed" })), Err(DmError::Closed));
        assert_eq!(judge(429, &json!({})), Err(DmError::RateLimited));
        assert!(matches!(judge(502, &json!({})), Err(DmError::Uncertain(_))));
        assert_eq!(sentence(&DmError::Closed).0, "discord_dm_closed");
        assert!(judge(200, &json!({ "channelId": "c" })).is_ok());
    }
}
