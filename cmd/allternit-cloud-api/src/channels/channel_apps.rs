//! Owner-side status of Allternit's own channel apps.
//!
//! * `GET /api/v1/admin/channel-apps` (Clerk user in `ALLTERNIT_ADMIN_USER_IDS`):
//!   per channel, whether the shared app is configured, which env names are
//!   still missing, the public webhook URL and (with `?check=true`) whether
//!   that URL answers. Never returns secret values.
//! * `GET /api/v1/channels/availability` (public): a boolean per channel so the
//!   app's connect wizards can show "Coming soon" instead of a broken button.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::app_env;
use crate::{error::ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/admin/channel-apps", get(status_h))
        .route("/api/v1/channels/availability", get(availability_h))
}

/// One credential a channel needs: the canonical env name plus accepted
/// fallbacks (read by `app_env`).
struct Var {
    names: &'static [&'static str],
    required: bool,
}

struct Spec {
    channel: &'static str,
    vars: &'static [Var],
    /// Path on the cloud API the vendor posts to; None when the channel has no vendor webhook.
    webhook_path: Option<&'static str>,
    notes: &'static str,
}

const fn req(names: &'static [&'static str]) -> Var {
    Var { names, required: true }
}
const fn opt(names: &'static [&'static str]) -> Var {
    Var { names, required: false }
}

const TELEGRAM_TOKEN: &[&str] = &["ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN"];
const TELEGRAM_SECRET: &[&str] = &["ALLTERNIT_TELEGRAM_MANAGER_WEBHOOK_SECRET"];
const DISCORD_APP_ID: &[&str] = &["ALLTERNIT_DISCORD_APP_ID"];
const DISCORD_PUBLIC_KEY: &[&str] = &["ALLTERNIT_DISCORD_PUBLIC_KEY"];
const DISCORD_BOT_TOKEN: &[&str] = &["ALLTERNIT_DISCORD_BOT_TOKEN"];
const DISCORD_CLIENT_SECRET: &[&str] = &["ALLTERNIT_DISCORD_CLIENT_SECRET"];
const TELNYX_KEY: &[&str] = &["ALLTERNIT_TELNYX_API_KEY"];
const TELNYX_PUBLIC: &[&str] = &["ALLTERNIT_TELNYX_PUBLIC_KEY"];

const SPECS: &[Spec] = &[
    Spec {
        channel: "telegram",
        vars: &[req(TELEGRAM_TOKEN), req(TELEGRAM_SECRET)],
        webhook_path: Some("/channels/telegram-manager/<webhook secret>"),
        notes: "Manager bot with Managed Bots enabled (BotFather). Webhook is registered by scripts/channel-apps/setup.mjs.",
    },
    Spec {
        channel: "discord",
        vars: &[req(DISCORD_APP_ID), req(DISCORD_PUBLIC_KEY), req(DISCORD_BOT_TOKEN), req(DISCORD_CLIENT_SECRET)],
        webhook_path: Some("/channels/discord/interactions"),
        notes: "Set as the Interactions Endpoint URL in the Developer Portal; OAuth redirect is /channels/discord/oauth/callback. Verification is needed past 100 servers.",
    },
    Spec {
        channel: "slack",
        vars: &[req(app_env::SLACK_CLIENT_ID), req(app_env::SLACK_CLIENT_SECRET), req(app_env::SLACK_SIGNING_SECRET)],
        webhook_path: Some("/api/v1/channels/slack/events"),
        notes: "Create from docs/channel-apps/slack-app-manifest.json. Unlisted apps install into any workspace; Marketplace listing is optional.",
    },
    Spec {
        channel: "teams",
        vars: &[req(app_env::TEAMS_APP_ID), req(app_env::TEAMS_APP_PASSWORD), opt(app_env::TEAMS_TENANT_ID)],
        webhook_path: Some("/channels/teams/messages"),
        notes: "Azure Bot messaging endpoint. Tenant id is only for a single-tenant bot. Package with scripts/channel-apps/teams-package.sh.",
    },
    Spec {
        channel: "whatsapp",
        vars: &[req(app_env::META_APP_ID), req(app_env::META_APP_SECRET), req(app_env::META_ES_CONFIG_ID), opt(app_env::META_SYSTEM_TOKEN)],
        webhook_path: None,
        notes: "Meta app with WhatsApp Embedded Signup; needs business verification and app review for other businesses.",
    },
    Spec {
        channel: "phone",
        vars: &[req(TELNYX_KEY), opt(TELNYX_PUBLIC)],
        webhook_path: Some("/api/v1/phone/webhooks/telnyx"),
        notes: "Telnyx (default carrier). Set ALLTERNIT_PHONE_CARRIER=twilio to use Twilio instead; this report checks the Telnyx names.",
    },
    Spec {
        channel: "email",
        vars: &[],
        webhook_path: None,
        notes: "Agent email runs on the mail rail; no owner app credentials in cloud-api.",
    },
];

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAppStatus {
    pub channel: &'static str,
    pub configured: bool,
    /// Canonical env names still missing (required ones only).
    pub missing: Vec<&'static str>,
    pub webhook_url: Option<String>,
    /// None unless `?check=true` was passed (or no webhook exists).
    pub webhook_reachable: Option<bool>,
    pub notes: &'static str,
}

fn public_base() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL")
        .unwrap_or_else(|_| "https://api.allternit.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Pure status computation; `get` reads env so tests don't touch the process.
fn statuses(get: &dyn Fn(&str) -> Option<String>, base: &str) -> Vec<ChannelAppStatus> {
    SPECS
        .iter()
        .map(|spec| {
            let missing: Vec<&'static str> = spec
                .vars
                .iter()
                .filter(|var| var.required && app_env::first_with(get, var.names).is_none())
                .map(|var| var.names[0])
                .collect();
            ChannelAppStatus {
                channel: spec.channel,
                configured: missing.is_empty(),
                missing,
                webhook_url: spec.webhook_path.map(|path| format!("{base}{path}")),
                webhook_reachable: None,
                notes: spec.notes,
            }
        })
        .collect()
}

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[derive(Deserialize)]
struct StatusQuery {
    check: Option<bool>,
}

/// Any HTTP answer (even 401/404/405) proves the host + path reach the service.
/// Telegram's URL embeds a secret, so it is never probed.
async fn probe(url: &str) -> bool {
    let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(4)).build() {
        Ok(client) => client,
        Err(_) => return false,
    };
    match client.get(url).send().await {
        Ok(response) => response.status().as_u16() != 502 && response.status().as_u16() != 503 && response.status().as_u16() != 504,
        Err(_) => false,
    }
}

async fn status_h(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<StatusQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    if !crate::auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admins only".to_string()));
    }
    let mut rows = statuses(&process_env, &public_base());
    if query.check.unwrap_or(false) {
        for row in rows.iter_mut() {
            if row.channel == "telegram" {
                continue;
            }
            if let Some(url) = row.webhook_url.clone() {
                row.webhook_reachable = Some(probe(&url).await);
            }
        }
    }
    Ok(Json(json!({ "channels": rows })))
}

fn availability(get: &dyn Fn(&str) -> Option<String>) -> serde_json::Value {
    let rows = statuses(get, "");
    let on = |channel: &str| rows.iter().any(|row| row.channel == channel && row.configured);
    json!({
        "telegram": on("telegram"),
        "discord": on("discord"),
        "slack": on("slack"),
        "teams": on("teams"),
        "whatsapp": on("whatsapp"),
        "email": on("email"),
        "phone": on("phone"),
    })
}

async fn availability_h() -> Response {
    let mut body = availability(&process_env);
    // Phone also depends on the chosen carrier's own env being valid.
    if let Some(phone) = body.get_mut("phone") {
        let carrier_ok = crate::carriers::from_env(Arc::new(crate::carriers::ReqwestHttp::new())).is_ok();
        *phone = json!(phone.as_bool().unwrap_or(false) || carrier_ok);
    }
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response
        .headers_mut()
        .insert("cache-control", axum::http::HeaderValue::from_static("public, max-age=60"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn nothing_configured_reports_canonical_missing_names() {
        let rows = statuses(&env(&[]), "https://api.example");
        let teams = rows.iter().find(|r| r.channel == "teams").unwrap();
        assert!(!teams.configured);
        assert_eq!(teams.missing, vec!["ALLTERNIT_TEAMS_APP_ID", "ALLTERNIT_TEAMS_APP_PASSWORD"]);
        assert_eq!(teams.webhook_url.as_deref(), Some("https://api.example/channels/teams/messages"));
        let email = rows.iter().find(|r| r.channel == "email").unwrap();
        assert!(email.configured);
        assert_eq!(email.webhook_url, None);
    }

    #[test]
    fn legacy_names_count_as_configured() {
        let rows = statuses(&env(&[("APP_ID", "a"), ("APP_PASSWORD", "p")]), "");
        assert!(rows.iter().find(|r| r.channel == "teams").unwrap().configured);
        let rows = statuses(&env(&[("SLACK_CLIENT_ID", "a"), ("SLACK_CLIENT_SECRET", "b")]), "");
        let slack = rows.iter().find(|r| r.channel == "slack").unwrap();
        assert_eq!(slack.missing, vec!["ALLTERNIT_SLACK_SIGNING_SECRET"]);
    }

    #[test]
    fn availability_is_booleans_and_hides_values() {
        let value = availability(&env(&[
            ("ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN", "t0ps3cret"),
            ("ALLTERNIT_TELEGRAM_MANAGER_WEBHOOK_SECRET", "s"),
        ]));
        assert_eq!(value["telegram"], true);
        assert_eq!(value["discord"], false);
        assert_eq!(value["email"], true);
        assert!(!value.to_string().contains("t0ps3cret"));
    }

    #[test]
    fn optional_vars_never_block_configured() {
        let rows = statuses(&env(&[("ALLTERNIT_TELNYX_API_KEY", "k")]), "");
        assert!(rows.iter().find(|r| r.channel == "phone").unwrap().configured);
    }
}
