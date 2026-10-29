//! Worker bots and vendor connect from a bot's chat: gizzi's `bot_create` and
//! `vendor_connect` tools.
//!
//! Whose bot is calling, and so who owns the new bot, comes from the calling
//! session (`bot_routing::authorize`, the same check `message_agent` uses),
//! never from the model.
//!
//! * `POST /bot-routing/bot/create` `{sessionId, name, purpose?, owns: [..],
//!   brain?: {provider, model?}, computer?}` — a new bot for the owner that
//!   owns kinds of tasks (`agents.owns`, V282). Teammates see `owns` on the
//!   roster and hand that work to it. `brain.provider` must be a CLI catalog
//!   id. Returns `{bot: {id, name, owns, brain}}`.
//! * `POST /bot-routing/vendor/connect` `{sessionId, provider, computer?}` —
//!   validates the vendor and returns the card the app renders:
//!   `{allternit_card: "vendor_connect", provider, label, signIn, computer}`.
//!   It does not start a sign-in and carries no link or code: the card calls
//!   the install/connect endpoints itself, so any code the user sees comes
//!   from the vendor CLI's own output.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::bot_routing::{authorize, Caller};
use crate::AppState;

pub fn worker_bot_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/bot/create", post(create_route))
        .route("/bot-routing/vendor/connect", post(connect_route))
}

#[derive(Debug, Deserialize)]
pub struct Brain {
    pub provider: String,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    pub session_id: String,
    pub name: String,
    pub purpose: Option<String>,
    #[serde(default)]
    pub owns: Vec<String>,
    pub brain: Option<Brain>,
    pub computer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectBody {
    pub session_id: String,
    pub provider: String,
    pub computer: Option<String>,
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

async fn caller(state: &AppState, user: &AuthUser, session_id: &str) -> Result<Caller, Response> {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), session_id.to_string());
    match tokio::task::spawn_blocking(move || authorize(&db, &uid, &sid)).await {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(err(StatusCode::NOT_FOUND, e.message())),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, e.to_string())),
    }
}

fn owner(caller: &Caller) -> AuthUser {
    AuthUser {
        user_id: caller.user_id.clone(),
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    }
}

/// Trimmed, de-duplicated (case-insensitive) task kinds, at most 20 of 80 chars.
pub fn clean_owns(owns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for o in owns {
        let o: String = o.trim().chars().take(80).collect();
        if !o.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&o)) {
            out.push(o);
        }
    }
    out.truncate(20);
    out
}

/// `cloud` | `local` | a computer id; blank = none.
fn clean_computer(c: Option<String>) -> Option<String> {
    c.map(|v| v.trim().chars().take(128).collect::<String>()).filter(|v| !v.is_empty())
}

async fn create_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<CreateBody>) -> Response {
    let c = match caller(&state, &user, &body.session_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let name = body.name.trim().to_string();
    let owns = clean_owns(&body.owns);
    let computer = clean_computer(body.computer);
    let (provider, model) = match &body.brain {
        Some(b) => match crate::cli_catalog::find(b.provider.trim()) {
            Some(tool) => {
                let model = b.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).unwrap_or(tool.default_model()).to_string();
                (tool.id.clone(), model)
            }
            None => return err(StatusCode::BAD_REQUEST, format!("'{}' isn't a vendor in the CLI catalog.", b.provider.trim())),
        },
        // No brain: the bot follows the platform default model.
        None => (String::new(), String::new()),
    };
    let description = body
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if owns.is_empty() { format!("{name}, a worker bot.") } else { format!("Worker bot that owns: {}.", owns.join(", ")) }
        });
    let harness_mode = if computer.as_deref() == Some("local") { "local" } else { "cloud" };
    let mut config = json!({ "isBot": true, "createdBy": { "botId": c.bot_id, "via": "bot_create" } });
    if let Some(comp) = &computer {
        config["computer"] = json!(comp);
    }
    let create = json!({
        "name": name,
        "description": description,
        "type": "worker",
        "model": model,
        "provider": provider,
        "is_bot": true,
        "bot_profile": { "displayName": name },
        "config": config,
        "harness_config": { "mode": harness_mode },
        "enabled_modes": ["chat"],
        "trust_tier": "standard",
        "owns": owns,
    });
    let (status, created) = crate::agent_routes::create_agent_json(state.clone(), owner(&c), create).await;
    if !status.is_success() {
        let message = created.get("error").and_then(Value::as_str).unwrap_or("couldn't create the bot").to_string();
        return err(status, message);
    }
    let id = created.pointer("/agent/id").and_then(Value::as_str).unwrap_or_default().to_string();
    let brain = if provider.is_empty() { Value::Null } else { json!({ "provider": provider, "model": model }) };
    Json(json!({ "bot": { "id": id, "name": name, "owns": owns, "brain": brain } })).into_response()
}

/// The card for `provider`, or `None` when it isn't a known vendor.
pub fn vendor_card(provider: &str, computer: Option<String>) -> Option<Value> {
    let (id, label, sign_in) = crate::provider_routes::connect_card_meta(provider)?;
    Some(json!({
        "allternit_card": "vendor_connect",
        "provider": id,
        "label": label,
        "signIn": sign_in,
        "computer": clean_computer(computer),
    }))
}

async fn connect_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<ConnectBody>) -> Response {
    if let Err(r) = caller(&state, &user, &body.session_id).await {
        return r;
    }
    match vendor_card(&body.provider, body.computer) {
        Some(card) => Json(card).into_response(),
        None => err(StatusCode::BAD_REQUEST, format!("'{}' isn't a vendor in the CLI catalog.", body.provider.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_card_is_exactly_the_contract_and_rejects_unknown_vendors() {
        let card = vendor_card("codex", Some(" cloud ".into())).unwrap();
        assert_eq!(
            card,
            json!({"allternit_card": "vendor_connect", "provider": "codex-cli", "label": "Codex", "signIn": "subscription", "computer": "cloud"})
        );
        assert_eq!(vendor_card("opencode", None).unwrap()["signIn"], "subscription");
        assert_eq!(vendor_card("zai", None).unwrap()["signIn"], "api_key");
        assert!(vendor_card("not-a-vendor", None).is_none());
    }

    #[test]
    fn owns_is_trimmed_and_deduped() {
        let owns = clean_owns(&[" invoices ".into(), "Invoices".into(), "".into(), "code review".into()]);
        assert_eq!(owns, vec!["invoices", "code review"]);
    }
}
