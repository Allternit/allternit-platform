//! One-step connect: a subscription (or key, or local endpoint, or channel) in,
//! ready Allternit bots out.
//!
//! Before this, turning "my ChatGPT Plus" into a bot took about ten calls across
//! two systems: sign in on the subscription gateway, create a provider account,
//! walk it through four connection states by hand, discover the vendor's
//! agents, create a bot with the full agent checklist, bind it, and PATCH the
//! binding READY. This module runs that chain server-side:
//!
//! * `GET  /gateway/connect` — every connectable product, with this user's
//!   state for it (account, bots).
//! * `POST /gateway/connect {product, ...}` — idempotent. Each call moves the
//!   connection as far as it can and answers with `status`:
//!   `ready` (bots listed), `needs_consent` (a website/app product: show
//!   `action.terms` and call again with `acceptTerms: true`; recorded once per
//!   account in `connection_audit`), `needs_sign_in` (a login window was opened on the
//!   gateway's machine; sign in, then call again), `needs_key`,
//!   `needs_setup` (the vendor app or gateway isn't reachable; `action.steps`
//!   says what to do), or `blocked` (the vendor shows a verification check).
//!
//! Nothing here is a new mechanism. It drives the existing pieces: the
//! gateway's own login flow (`/v1/accounts`, via `subscription_routes::forward`),
//! `discover_agents`, `apply_account_state` along `connection_path`,
//! `upsert_exec_binding`, `set_exec_state`, and `POST /agents`' own handler.
//! An account only reaches CONNECTED, and a binding READY, after a real round
//! trip to the vendor (discovery) succeeded, never by fiat. The login itself is
//! always done by the person in a plain browser window; this never sees or
//! automates credentials, and never clears a verification check.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_gateway_routes::{
    apply_account_state, audit, connection_path, discover_agents, exec_next, get_account, now, one, rows, s, seal_strict,
    set_exec_state, upsert_exec_binding, PutExec, EXEC_COLS,
};
use crate::auth::AuthUser;
use crate::AppState;

/// How the person proves they own the product.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SignIn {
    /// A web subscription: the gateway opens the vendor's site, the person logs in.
    Browser,
    /// The vendor's desktop app (or a browser window the adapter owns) on the gateway's machine.
    DesktopApp,
    /// An API key, sealed server-side.
    ApiKey,
    /// A server the person runs (OpenAI-compatible).
    LocalEndpoint,
    /// A messaging channel account the vendor answers on (Muse over WhatsApp).
    Channel,
}

/// One connectable product.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Product {
    id: &'static str,
    name: &'static str,
    /// What the person gets once connected.
    offers: &'static str,
    /// Agent Gateway vendor id on the provider account.
    vendor: &'static str,
    auth_type: &'static str,
    adapter: &'static str,
    mode: &'static str,
    lane: &'static str,
    sign_in: SignIn,
    /// The subscription gateway's provider id, for browser sign-in.
    gateway_provider: Option<&'static str>,
    /// Whether a real vendor session has ever been run through this adapter.
    verified_live: bool,
    /// What to do when the vendor can't be reached.
    setup: &'static [&'static str],
}

pub(crate) const PRODUCTS: &[Product] = &[
    Product {
        id: "chatgpt",
        name: "ChatGPT",
        offers: "Your ChatGPT chat, plus your pinned GPTs and Projects",
        vendor: "openai",
        auth_type: "browser_session",
        adapter: "chatgpt-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("chatgpt"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed."],
    },
    Product {
        id: "chatgpt-dots",
        name: "ChatGPT dots",
        offers: "Your dots",
        vendor: "openai",
        auth_type: "desktop_session",
        adapter: "chatgpt-dots",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::DesktopApp,
        gateway_provider: None,
        verified_live: false,
        setup: &[
            "Restart the subscription gateway with SUBS_GATEWAY_DOTS_CONSENT=1 (your OK for it to open a ChatGPT window).",
            "Sign in to ChatGPT in the window it opens, with an account that has dots.",
            "Call connect again.",
        ],
    },
    Product {
        id: "claude",
        name: "Claude",
        offers: "Your Claude chat, plus your Projects",
        vendor: "anthropic",
        auth_type: "browser_session",
        adapter: "claude-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("claude"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed."],
    },
    Product {
        id: "claude-duet",
        name: "Claude Duet",
        offers: "Your Claude Duet thread (Anthropic's proactive agent; research preview, Claude Pro or Max in the US)",
        vendor: "claude-duet",
        auth_type: "browser_session",
        adapter: "claude-duet-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("claude-duet"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed.", "Sign in to claude.ai with the account that has Duet, then open Duet once."],
    },
    Product {
        id: "claude-managed-agents",
        name: "Claude Managed Agents",
        offers: "Agents you build on the Claude API (developers; needs an API key and an environment). Not Claude Duet",
        vendor: "anthropic",
        auth_type: "api_key",
        adapter: "claude-managed-agents",
        mode: "hosted",
        lane: "official",
        sign_in: SignIn::ApiKey,
        gateway_provider: None,
        verified_live: false,
        setup: &[
            "Create an agent and an environment in the Claude Console.",
            "Start the subscription gateway with SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID=<env id>.",
            "Call connect again with apiKey.",
        ],
    },
    Product {
        id: "gemini",
        name: "Gemini",
        offers: "Your Gemini chat, plus your Gems",
        vendor: "google",
        auth_type: "browser_session",
        adapter: "gemini-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("google"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed."],
    },
    Product {
        id: "copilot",
        name: "Microsoft Copilot",
        offers: "Your Copilot chat, plus your agents and pages",
        vendor: "microsoft",
        auth_type: "browser_session",
        adapter: "copilot-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("microsoft"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed."],
    },
    Product {
        id: "kimi",
        name: "Kimi",
        offers: "Your Kimi chat, plus saved Kimi+ agents",
        vendor: "kimi",
        auth_type: "browser_session",
        adapter: "kimi-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("kimi"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed."],
    },
    Product {
        id: "gemini-spark",
        name: "Gemini Spark",
        offers: "Your Gemini Spark tasks (beta; needs a Google AI Ultra or Pro plan)",
        vendor: "gemini-spark",
        auth_type: "browser_session",
        adapter: "gemini-spark-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("gemini-spark"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed.", "Sign in to Gemini with the account that has Spark, then open the Spark tab once."],
    },
    Product {
        id: "slashy",
        name: "Slashy",
        offers: "Your Slashy assistant chat (www.slashy.com)",
        vendor: "slashy",
        auth_type: "browser_session",
        adapter: "slashy-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("slashy"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed."],
    },
    Product {
        id: "abacusai-bot",
        name: "AbacusAI Bot",
        offers: "Your AbacusAI bots, through your own Abacus.AI sign-in",
        vendor: "abacusai",
        auth_type: "browser_session",
        adapter: "abacusai-bot-subscription",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::Browser,
        gateway_provider: Some("abacusai"),
        verified_live: false,
        setup: &["Run the subscription gateway on this machine with Google Chrome installed.", "Sign in to Abacus.AI at apps.abacus.ai/bot in the login browser."],
    },
    Product {
        id: "grok-bot",
        name: "Grok Bot",
        offers: "Grok Bot and the bots in its picker",
        vendor: "grok",
        auth_type: "desktop_session",
        adapter: "grok-bot",
        mode: "linked",
        lane: "ui_bridge",
        sign_in: SignIn::DesktopApp,
        gateway_provider: None,
        verified_live: true,
        setup: &[
            "Quit Grok Bot: osascript -e 'quit app \"Grok Bot\"'",
            "Relaunch it with a debug port: open -a \"Grok Bot\" --args --remote-debugging-port=9231",
            "Make sure you're signed in, then call connect again.",
        ],
    },
    Product {
        id: "openclaw",
        name: "OpenClaw",
        offers: "Your OpenClaw agents",
        vendor: "openclaw",
        auth_type: "local_endpoint",
        adapter: "openclaw",
        mode: "hosted",
        lane: "local",
        sign_in: SignIn::LocalEndpoint,
        gateway_provider: None,
        verified_live: false,
        setup: &["Start OpenClaw (default http://127.0.0.1:18789), or set SUBS_GATEWAY_OPENCLAW_URL on the gateway."],
    },
    Product {
        id: "hermes",
        name: "Hermes",
        offers: "Your Hermes profiles",
        vendor: "hermes",
        auth_type: "local_endpoint",
        adapter: "hermes",
        mode: "hosted",
        lane: "local",
        sign_in: SignIn::LocalEndpoint,
        gateway_provider: None,
        verified_live: false,
        setup: &["Start Hermes (default http://127.0.0.1:8642), or set SUBS_GATEWAY_HERMES_URL on the gateway."],
    },
    Product {
        id: "muse",
        name: "Meta Muse",
        offers: "Muse, reached over your WhatsApp Business number",
        vendor: "meta",
        auth_type: "api_key",
        adapter: "whatsapp",
        mode: "linked",
        lane: "channel",
        sign_in: SignIn::Channel,
        gateway_provider: None,
        verified_live: false,
        setup: &[
            "Connect a WhatsApp Business number (POST /gateway/channel-accounts/whatsapp), or pass accessToken, phoneNumberId and appSecret.",
            "Pass agentAddress: the WhatsApp number Muse answers on.",
            "The API must be reachable from the internet for WhatsApp webhooks.",
        ],
    },
];

pub(crate) fn product(id: &str) -> Option<&'static Product> {
    PRODUCTS.iter().find(|p| p.id == id)
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/connect", get(catalog_h).post(connect_h))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectBody {
    product: String,
    /// `claude-managed-agents`: the Anthropic API key.
    api_key: Option<String>,
    /// Which discovered agents become bots: ids from `agents`. Omitted = all of them.
    agents: Option<Vec<String>>,
    /// `muse`: an existing WhatsApp channel account to use.
    channel_account_id: Option<String>,
    /// `muse`: WhatsApp Cloud API credentials, when no channel account exists yet.
    access_token: Option<String>,
    phone_number_id: Option<String>,
    app_secret: Option<String>,
    /// `muse`: the number the vendor answers on.
    agent_address: Option<String>,
    /// The person read `terms` and agrees to Allternit driving the vendor's
    /// website or app with their own login (UI-bridge products only).
    accept_terms: Option<bool>,
}

/// Whether Allternit drives the vendor's own website or app for this product,
/// which needs the person's informed OK first.
fn needs_consent(p: &Product) -> bool {
    p.lane == "ui_bridge"
}

/// What the person agrees to before a UI-bridge product connects.
fn terms(p: &Product) -> String {
    // chatgpt-dots drives ChatGPT in a browser window by default; Grok Bot is a desktop app.
    let surface = if p.id == "grok-bot" { "app" } else { "website" };
    format!(
        "Allternit will use the {name} {surface} for you, signed in as you, the way you would by hand. \
         {name} has no official API for this, so it may break when {name} changes, it runs one conversation at a time, \
         and {name}'s terms may not allow automated use: your {name} account could be limited. \
         Your password never passes through Allternit, and you can disconnect at any time.",
        name = p.name
    )
}

fn terms_accepted(conn: &rusqlite::Connection, owner: &str, aid: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM connection_audit WHERE owner = ?1 AND account_binding_id = ?2 AND event = 'terms_accepted'",
        params![owner, aid],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------- catalog

async fn catalog_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    let owner = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Value> {
        let conn = db.connect()?;
        let mut out = vec![];
        for p in PRODUCTS {
            let acct = find_account(&conn, &owner, p, None)?;
            let bots = match &acct {
                Some(a) => bots_of(&conn, &owner, &s(a, "id"))?,
                None => vec![],
            };
            out.push(json!({
                "product": p.id,
                "name": p.name,
                "offers": p.offers,
                "signIn": sign_in_name(p.sign_in),
                "verifiedLive": p.verified_live,
                "terms": needs_consent(p).then(|| terms(p)),
                "account": acct.map(|a| json!({ "id": s(&a, "id"), "state": s(&a, "state") })),
                "bots": bots,
            }));
        }
        Ok(json!({ "products": out }))
    })
    .await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response(),
    }
}

fn sign_in_name(k: SignIn) -> &'static str {
    match k {
        SignIn::Browser => "browser",
        SignIn::DesktopApp => "desktop_app",
        SignIn::ApiKey => "api_key",
        SignIn::LocalEndpoint => "local_endpoint",
        SignIn::Channel => "channel",
    }
}

// ---------------------------------------------------------------- connect

/// The answer to one connect call.
struct Outcome {
    status: &'static str,
    message: String,
    action: Option<Value>,
}

impl Outcome {
    fn new(status: &'static str, message: impl Into<String>, action: Option<Value>) -> Self {
        Self { status, message: message.into(), action }
    }
    fn setup(p: &Product, message: impl Into<String>) -> Self {
        Self::new("needs_setup", message, Some(json!({ "kind": "setup", "steps": p.setup })))
    }
}

async fn connect_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<ConnectBody>) -> Response {
    let Some(p) = product(b.product.trim()) else {
        let ids: Vec<&str> = PRODUCTS.iter().map(|p| p.id).collect();
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "unknown product", "products": ids }))).into_response();
    };
    match connect(&state, &user, p, &b).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

/// Move `p`'s connection for `user` as far as it can go. Idempotent.
pub(crate) async fn connect(state: &Arc<AppState>, user: &AuthUser, p: &Product, b: &ConnectBody) -> Result<Value, String> {
    let owner = user.user_id.clone();
    let db = state.db.clone();
    let dberr = |e: rusqlite::Error| format!("database error: {e}");

    // 1. The provider account (found or created), with its credential.
    let aid = {
        let conn = db.connect().map_err(dberr)?;
        match find_account(&conn, &owner, p, b.channel_account_id.as_deref()).map_err(dberr)? {
            Some(a) => s(&a, "id"),
            // A named account that isn't yours is an error, not a reason to make a new one.
            None if b.channel_account_id.is_some() => {
                return Ok(json!({ "product": p.id, "name": p.name, "status": "needs_setup", "message": "channelAccountId is not one of your accounts.", "action": null, "agents": [], "bots": [] }));
            }
            None => create_account(&conn, &owner, p).map_err(dberr)?,
        }
    };
    // 1b. UI-bridge products: the person's informed OK, once per account.
    if needs_consent(p) {
        let conn = db.connect().map_err(dberr)?;
        if !terms_accepted(&conn, &owner, &aid).map_err(dberr)? {
            if b.accept_terms == Some(true) {
                audit(&conn, &owner, &aid, "terms_accepted", None, None, json!({ "product": p.id, "lane": p.lane, "terms": terms(p) }));
            } else {
                let o = Outcome::new(
                    "needs_consent",
                    format!("Before connecting {}, read this and agree to it.", p.name),
                    Some(json!({ "kind": "consent", "terms": terms(p) })),
                );
                return respond(state, &owner, p, &aid, o, vec![]);
            }
        }
    }
    if let Some(early) = store_credential(state, &owner, p, &aid, b)? {
        return respond(state, &owner, p, &aid, early, vec![]);
    }

    // 2. Browser subscriptions: the gateway's login must be signed in.
    if p.sign_in == SignIn::Browser {
        if let Some(early) = ensure_browser_login(state, user, p, &aid).await? {
            return respond(state, &owner, p, &aid, early, vec![]);
        }
    }

    // 3. A real round trip to the vendor: what agents does this account have?
    let agents = if p.sign_in == SignIn::Channel {
        let Some(addr) = b.agent_address.as_deref().map(str::trim).filter(|a| !a.is_empty()) else {
            return respond(state, &owner, p, &aid, Outcome::setup(p, "Muse needs agentAddress: the WhatsApp number it answers on."), vec![]);
        };
        vec![json!({ "externalAgentId": addr, "name": "Muse", "kind": "channel", "kindLabel": "WhatsApp" })]
    } else {
        let tx = crate::gateway_runner::transport(state);
        match discover_agents(&db, tx.as_ref(), &owner, &aid).await {
            Ok(a) => a,
            Err((_, code, msg)) => return respond(state, &owner, p, &aid, discovery_failure(p, &code, &msg), vec![]),
        }
    };
    if agents.is_empty() {
        return respond(state, &owner, p, &aid, Outcome::setup(p, format!("{} answered but listed no agents.", p.name)), vec![]);
    }
    // Some adapters list a fixed default agent without touching the vendor, so
    // the listing alone proves nothing: ask the adapter whether the vendor is up.
    if p.sign_in != SignIn::Channel {
        if let Some(down) = probe_health(state, &owner, p, &aid, &s(&agents[0], "externalAgentId")).await {
            return respond(state, &owner, p, &aid, down, vec![]);
        }
    }

    // 4. Verified: the account goes to CONNECTED along legal hops.
    {
        let conn = db.connect().map_err(dberr)?;
        walk_account(&db, &conn, &owner, &aid, "CONNECTED", "connect: vendor reachable").map_err(dberr)?;
        crate::agent_gateway_routes::refresh_agent_identity(&conn, &owner, &aid, &agents);
    }

    // 5. One READY bot per chosen agent.
    let chosen: Vec<Value> = match &b.agents {
        Some(ids) => agents.iter().filter(|a| ids.iter().any(|i| i == &s(a, "externalAgentId"))).cloned().collect(),
        None => agents.clone(),
    };
    for agent in &chosen {
        ensure_bot(state, user, p, &aid, agent).await?;
    }
    let n = chosen.len();
    let msg = if n == 0 {
        format!("{} is connected, but none of the requested agents were found. Pick from `agents`.", p.name)
    } else {
        format!("{} is connected. {n} bot{} ready.", p.name, if n == 1 { " is" } else { "s are" })
    };
    respond(state, &owner, p, &aid, Outcome::new("ready", msg, None), agents)
}

fn respond(state: &Arc<AppState>, owner: &str, p: &Product, aid: &str, o: Outcome, agents: Vec<Value>) -> Result<Value, String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    let acct = get_account(&conn, owner, aid).map_err(|_| "account vanished".to_string())?;
    let bots = bots_of(&conn, owner, aid).map_err(|e| e.to_string())?;
    Ok(json!({
        "product": p.id,
        "name": p.name,
        "status": o.status,
        "message": o.message,
        "action": o.action,
        "verifiedLive": p.verified_live,
        "account": { "id": aid, "state": s(&acct, "state") },
        "agents": agents,
        "bots": bots,
    }))
}

/// The product's account for this owner: the one named, else the most recent
/// with the product's vendor and auth type.
fn find_account(conn: &rusqlite::Connection, owner: &str, p: &Product, named: Option<&str>) -> rusqlite::Result<Option<Value>> {
    if let Some(id) = named {
        return Ok(get_account(conn, owner, id).ok());
    }
    let vendor = account_vendor(p);
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM provider_account_bindings WHERE owner = ?1 AND vendor = ?2 AND auth_type = ?3
               AND state NOT IN ('REVOKED') ORDER BY created_at DESC LIMIT 1",
            params![owner, vendor, p.auth_type],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id.and_then(|id| get_account(conn, owner, &id).ok()))
}

/// The vendor recorded on the account. Muse's account is the WhatsApp channel
/// account (the binding carries `meta`), so `ChannelLaneTransport` finds it.
fn account_vendor(p: &Product) -> &'static str {
    if p.sign_in == SignIn::Channel {
        "whatsapp"
    } else {
        p.vendor
    }
}

fn create_account(conn: &rusqlite::Connection, owner: &str, p: &Product) -> rusqlite::Result<String> {
    let aid = crate::agent_gateway_routes::id("acct");
    let t = now();
    conn.execute(
        "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, display_name, scopes_json, state, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,'[]','DISCONNECTED',?6,?6)",
        params![aid, owner, account_vendor(p), p.auth_type, format!("My {}", p.name), t],
    )?;
    audit(conn, owner, &aid, "account.created", None, Some("DISCONNECTED"), json!({ "vendor": account_vendor(p), "authType": p.auth_type, "via": "connect" }));
    Ok(aid)
}

/// Seal a key (or channel credentials) when given. `Some(outcome)` when the
/// account still lacks one.
fn store_credential(state: &Arc<AppState>, owner: &str, p: &Product, aid: &str, b: &ConnectBody) -> Result<Option<Outcome>, String> {
    let plain = match p.sign_in {
        SignIn::ApiKey => b.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(str::to_string),
        SignIn::Channel => match (&b.access_token, &b.phone_number_id) {
            (Some(tok), Some(pn)) if !tok.trim().is_empty() && !pn.trim().is_empty() => Some(
                json!({ "accessToken": tok.trim(), "phoneNumberId": pn.trim(), "appSecret": b.app_secret.as_deref().unwrap_or("").trim() }).to_string(),
            ),
            _ => None,
        },
        _ => return Ok(None),
    };
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    if let Some(plain) = plain {
        let Some(sealed) = seal_strict(&plain) else {
            return Ok(Some(Outcome::new(
                "needs_setup",
                "No encryption key is configured on the API, so the credential was not stored. Set ALLTERNIT_ENCRYPTION_KEY.",
                None,
            )));
        };
        conn.execute("UPDATE provider_account_bindings SET secret_ref = ?1, updated_at = ?2 WHERE id = ?3 AND owner = ?4", params![sealed, now(), aid, owner])
            .map_err(|e| e.to_string())?;
        audit(&conn, owner, aid, "secret_set", None, None, json!({ "via": "connect" }));
        return Ok(None);
    }
    let has: Option<String> = conn
        .query_row("SELECT secret_ref FROM provider_account_bindings WHERE id = ?1 AND owner = ?2", params![aid, owner], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    if has.filter(|x| !x.is_empty()).is_some() {
        return Ok(None);
    }
    Ok(Some(match p.sign_in {
        SignIn::ApiKey => Outcome::new("needs_key", format!("{} needs your API key: call connect again with apiKey.", p.name), Some(json!({ "kind": "api_key", "steps": p.setup }))),
        _ => Outcome::setup(p, format!("{} needs a WhatsApp Business number.", p.name)),
    }))
}

/// Ensure a signed-in gateway login for a browser subscription and link it to
/// the account (`externalAccountId = subsfab:<login>`, which the subscription
/// sync then keeps in step). Opens the login window when one is needed.
async fn ensure_browser_login(state: &Arc<AppState>, user: &AuthUser, p: &Product, aid: &str) -> Result<Option<Outcome>, String> {
    let provider = p.gateway_provider.unwrap_or(p.id);
    let accounts = match gw(state, user, Method::GET, "v1/accounts", None).await {
        Ok(v) => v,
        Err((status, body)) => return Ok(Some(gateway_unreachable(p, status, &body))),
    };
    let linked = linked_login(state, &user.user_id, aid);
    let list = accounts.as_array().cloned().unwrap_or_default();
    let login = list
        .iter()
        .find(|a| linked.as_deref() == a["account_id"].as_str())
        .or_else(|| list.iter().find(|a| a["provider"].as_str() == Some(provider) && a["enabled"].as_bool() != Some(false)))
        .cloned();
    let login = match login {
        Some(l) => l,
        None => match gw(state, user, Method::POST, "v1/accounts", Some(json!({ "provider": provider, "label": p.name, "login": true }))).await {
            Ok(v) => v,
            Err((status, body)) => return Ok(Some(gateway_unreachable(p, status, &body))),
        },
    };
    let login_id = s(&login, "account_id");
    if linked.as_deref() != Some(login_id.as_str()) {
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE provider_account_bindings SET external_account_id = ?1, updated_at = ?2 WHERE id = ?3 AND owner = ?4",
            params![format!("subsfab:{login_id}"), now(), aid, user.user_id],
        )
        .map_err(|e| e.to_string())?;
        // The person is now in the middle of authenticating.
        walk_account(&state.db, &conn, &user.user_id, aid, "AUTHENTICATING", "connect: login window").map_err(|e| e.to_string())?;
    }
    match login["session_health"].as_str().unwrap_or("auth_required") {
        "ready" | "degraded" => Ok(None),
        "challenge_presented" => Ok(Some(Outcome::new(
            "blocked",
            format!("{} is showing a verification check. Clear it in the {} window, then call connect again.", p.name, p.name),
            Some(json!({ "kind": "sign_in", "loginId": login_id })),
        ))),
        health => {
            // Open (or re-open) the login window unless one is already up.
            let open = gw(state, user, Method::GET, &format!("v1/accounts/{login_id}/login"), None).await.ok();
            let waiting = open.as_ref().and_then(|v| v["state"].as_str()) == Some("waiting");
            if !waiting {
                if let Err((status, body)) = gw(state, user, Method::POST, &format!("v1/accounts/{login_id}/login"), Some(json!({}))).await {
                    return Ok(Some(gateway_unreachable(p, status, &body)));
                }
            }
            Ok(Some(Outcome::new(
                "needs_sign_in",
                format!("A {} window is open on the machine running your gateway. Sign in there, then call connect again.", p.name),
                Some(json!({ "kind": "sign_in", "loginId": login_id, "health": health })),
            )))
        }
    }
}

fn linked_login(state: &Arc<AppState>, owner: &str, aid: &str) -> Option<String> {
    let conn = state.db.connect().ok()?;
    let ext: Option<String> = conn
        .query_row("SELECT external_account_id FROM provider_account_bindings WHERE id = ?1 AND owner = ?2", params![aid, owner], |r| r.get(0))
        .optional()
        .ok()??;
    ext.and_then(|e| e.strip_prefix("subsfab:").map(str::to_string))
}

/// `agent.health` through the adapter. `Some(outcome)` when the vendor is down;
/// an adapter without a health op passes (discovery already answered).
async fn probe_health(state: &Arc<AppState>, owner: &str, p: &Product, aid: &str, agent_id: &str) -> Option<Outcome> {
    let binding = json!({ "type": "vendor", "vendor": p.vendor, "adapterId": p.adapter, "accountBindingId": aid });
    let tx = crate::gateway_runner::transport(state);
    match crate::gateway_runner::vcall(&state.db, tx.as_ref(), owner, "agent.health", &binding, json!({ "agentId": agent_id })).await {
        Ok(v) if v["status"].as_str() == Some("down") => {
            // Adapters report "CODE: message".
            let detail = v["detail"].as_str().unwrap_or("no detail");
            let (code, msg) = detail.split_once(':').map(|(c, m)| (c.trim(), m.trim())).unwrap_or(("", detail));
            Some(discovery_failure(p, code, msg))
        }
        Ok(_) => None,
        Err(e) if e.code == "UNSUPPORTED" => None,
        Err(e) => Some(discovery_failure(p, &e.code, &e.human_message)),
    }
}

/// A gateway call that failed. 502/503 and "no Sessions computer" mean the
/// gateway can't be reached; anything else is the gateway saying no.
fn gateway_unreachable(p: &Product, status: u16, body: &Value) -> Outcome {
    let code = body["error"].as_str().or_else(|| body["code"].as_str()).unwrap_or("");
    if code == "login_browser_unavailable" {
        return Outcome::new(
            "needs_setup",
            format!("The gateway can't open a {} sign-in window: no login browser is installed on its machine.", p.name),
            Some(json!({ "kind": "setup", "steps": ["Install Google Chrome on the machine running the subscription gateway, or set SUBS_GATEWAY_LOGIN_BROWSER to a Chrome/Chromium binary.", "Call connect again."] })),
        );
    }
    let reachable = !(status == 502 || status == 503 || code.starts_with("sessions_computer") || code == "GATEWAY_OFFLINE");
    if reachable {
        let detail = body["detail"].as_str().or_else(|| body["error"].as_str()).unwrap_or("no detail");
        return Outcome::setup(p, format!("The subscription gateway refused the {} sign-in ({status}: {detail}).", p.name));
    }
    let detail = body["detail"].as_str().or_else(|| body["error"].as_str()).unwrap_or("no reply");
    let mut steps = vec![
        "Start the subscription gateway (services/subscription-gateway: pnpm start).".to_string(),
        "Start the API with ALLTERNIT_LOCAL_SUBS_GATEWAY=1 and ALLTERNIT_LOCAL_SUBS_GATEWAY_STATE_DIR=<the gateway's state dir> to use a gateway on this machine, or bind a Sessions computer (PUT /api/v1/subscriptions/binding).".to_string(),
    ];
    steps.extend(p.setup.iter().map(|s| s.to_string()));
    Outcome::new(
        "needs_setup",
        format!("The subscription gateway isn't reachable ({status}: {detail})."),
        Some(json!({ "kind": "setup", "steps": steps })),
    )
}

fn discovery_failure(p: &Product, code: &str, msg: &str) -> Outcome {
    match code {
        "AUTH_REQUIRED" if p.sign_in == SignIn::ApiKey => {
            Outcome::new("needs_key", format!("{}: {msg}", p.name), Some(json!({ "kind": "api_key", "steps": p.setup })))
        }
        "AUTH_REQUIRED" => Outcome::new(
            "needs_sign_in",
            format!("{} isn't signed in: {msg}", p.name),
            Some(json!({ "kind": "sign_in", "steps": p.setup })),
        ),
        // chatgpt-dots reports "not allowed to open its window yet" with the
        // same code as a verification banner; that one is a setup step.
        "LANE_BLOCKED" if msg.contains("your OK") => Outcome::setup(p, format!("{}: {msg}", p.name)),
        "LANE_BLOCKED" => Outcome::new(
            "blocked",
            format!("{} is showing a verification check: {msg}. Clear it yourself, then call connect again.", p.name),
            None,
        ),
        "GATEWAY_OFFLINE" | "GATEWAY_UNAVAILABLE" => gateway_unreachable(p, 503, &json!({ "detail": msg })),
        _ => Outcome::setup(p, format!("{} isn't reachable ({code}): {msg}", p.name)),
    }
}

/// Move the account to `to` along the shortest legal path. A no-op when it is
/// already there; when no path exists (e.g. REVOKED) it goes back through
/// DISCONNECTED first.
fn walk_account(db: &crate::db::DbHandle, conn: &rusqlite::Connection, owner: &str, aid: &str, to: &str, reason: &str) -> rusqlite::Result<()> {
    let acct = match get_account(conn, owner, aid) {
        Ok(a) => a,
        Err(_) => return Ok(()),
    };
    let mut from = s(&acct, "state");
    // Never step backwards from CONNECTED to AUTHENTICATING.
    if to == "AUTHENTICATING" && from == "CONNECTED" {
        return Ok(());
    }
    let path = match connection_path(&from, to) {
        Some(p) => p,
        None => {
            let mut p = connection_path(&from, "DISCONNECTED").unwrap_or_default();
            p.extend(connection_path("DISCONNECTED", to).unwrap_or_default());
            p
        }
    };
    for hop in path {
        apply_account_state(db, conn, owner, aid, &from, hop, false, json!({ "reason": reason }))?;
        from = hop.to_string();
    }
    Ok(())
}

/// Bots already bound to this account: `[{botId, name, externalAgentId, state}]`.
fn bots_of(conn: &rusqlite::Connection, owner: &str, aid: &str) -> rusqlite::Result<Vec<Value>> {
    rows(
        conn,
        "SELECT b.bot_id AS botId, a.name AS name, b.external_agent_id AS externalAgentId, b.state AS state
           FROM bot_execution_bindings b JOIN agents a ON a.id = b.bot_id
          WHERE b.owner = ?1 AND b.account_binding_id = ?2 ORDER BY b.created_at",
        &[&owner, &aid],
    )
}

/// The bot for one vendor agent: found or created, bound, READY.
async fn ensure_bot(state: &Arc<AppState>, user: &AuthUser, p: &Product, aid: &str, agent: &Value) -> Result<(), String> {
    let owner = user.user_id.as_str();
    let ext = s(agent, "externalAgentId");
    let existing: Option<String> = {
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT bot_id FROM bot_execution_bindings WHERE owner = ?1 AND account_binding_id = ?2 AND external_agent_id = ?3",
            params![owner, aid, ext],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    };
    let bot_id = match existing {
        Some(b) => b,
        None => {
            let name = bot_name(p, agent);
            let body = json!({
                "name": name,
                "description": format!("{name}, running on your {} account through the Agent Gateway.", p.name),
                "type": "assistant",
                "model": "",
                "provider": "",
                "is_bot": true,
                "harness_config": { "mode": "cloud" },
                "enabled_modes": ["chat"],
                "trust_tier": "standard",
                "avatar": agent["avatarUrl"].as_str(),
            });
            let (status, v) = crate::agent_routes::create_agent_json(state.clone(), user.clone(), body).await;
            if !status.is_success() {
                return Err(format!("creating the bot for {ext} failed: {}", v["error"].as_str().unwrap_or("unknown error")));
            }
            v["agent"]["id"].as_str().ok_or("bot create returned no id")?.to_string()
        }
    };
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    let put = PutExec {
        r#type: Some("vendor".into()),
        mode: Some(p.mode.into()),
        vendor: Some(p.vendor.into()),
        adapter_id: Some(p.adapter.into()),
        account_binding_id: Some(aid.into()),
        preferred_lane: Some(p.lane.into()),
        external_agent_id: Some(ext.clone()),
        external_agent_name: agent["name"].as_str().map(str::to_string),
        external_agent_avatar: agent["avatarUrl"].as_str().map(str::to_string),
        directing_bot_id: None,
        capabilities: (p.sign_in == SignIn::Channel).then(|| json!({ "identity": { "who": "Muse" } })),
        health: None,
        harness: None,
        machine: None,
        pane_id: None,
    };
    upsert_exec_binding(&state.db, &conn, owner, &bot_id, &put).map_err(|e| format!("binding failed: {}", e.message()))?;
    let row = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&bot_id, &owner])
        .map_err(|e| e.to_string())?
        .ok_or("binding vanished")?;
    let from = s(&row, "state");
    if from != "READY" && exec_next(&from).contains(&"READY") {
        set_exec_state(&state.db, &conn, owner, &row, "READY", "connect: vendor reachable").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// "ChatGPT", "Code Reviewer (GPT)", "Muse".
fn bot_name(p: &Product, agent: &Value) -> String {
    let name = agent["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(p.name).trim();
    let mut out = match agent["kindLabel"].as_str() {
        Some(k) if !k.is_empty() && !matches!(agent["kind"].as_str(), Some("default" | "chat" | "channel")) && name != p.name => format!("{name} ({k})"),
        _ => name.to_string(),
    };
    if out.chars().count() < 3 {
        out = format!("{} {out}", p.name);
    }
    out.chars().take(80).collect()
}

/// A call to the subscription gateway through the API's own forwarder (local
/// gateway or bound Sessions computer). Non-2xx comes back as `(status, body)`.
async fn gw(state: &Arc<AppState>, user: &AuthUser, method: Method, path: &str, body: Option<Value>) -> Result<Value, (u16, Value)> {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let bytes = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
    let resp = crate::subscription_routes::forward(state, user, path, method, &headers, None, bytes).await;
    let status = resp.status();
    let raw = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap_or_default();
    let v: Value = serde_json::from_slice(&raw).unwrap_or_else(|_| json!({ "error": String::from_utf8_lossy(&raw).chars().take(300).collect::<String>() }));
    if status.is_success() {
        Ok(v)
    } else {
        Err((status.as_u16(), v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_product_is_unique_and_complete() {
        let mut ids: Vec<&str> = PRODUCTS.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), PRODUCTS.len());
        for p in PRODUCTS {
            assert!(crate::agent_gateway_routes::AUTH_TYPES.contains(&p.auth_type), "{}", p.id);
            assert!(!p.setup.is_empty(), "{}", p.id);
            assert_eq!(p.sign_in == SignIn::Browser, p.gateway_provider.is_some(), "{}", p.id);
            // The adapter connect picks must be the one discovery and turns pick.
            if let Some(a) = crate::agent_gateway_routes::adapter_for_auth(p.vendor, p.auth_type) {
                assert_eq!(a, p.adapter, "{}", p.id);
            }
        }
    }

    #[test]
    fn website_and_app_products_need_consent_and_api_products_do_not() {
        for id in ["chatgpt", "chatgpt-dots", "claude", "gemini", "copilot", "kimi", "grok-bot"] {
            let p = product(id).unwrap();
            assert!(needs_consent(p), "{id}");
            assert!(terms(p).contains(p.name), "{id}");
        }
        for id in ["claude-managed-agents", "openclaw", "hermes", "muse"] {
            assert!(!needs_consent(product(id).unwrap()), "{id}");
        }
        assert!(terms(product("grok-bot").unwrap()).contains("Grok Bot app"));
        assert!(terms(product("chatgpt").unwrap()).contains("ChatGPT website"));
    }

    #[test]
    fn bot_names_read_well() {
        let p = product("chatgpt").unwrap();
        assert_eq!(bot_name(p, &json!({ "name": "ChatGPT", "kind": "default" })), "ChatGPT");
        assert_eq!(bot_name(p, &json!({ "name": "Code Reviewer", "kind": "gpt", "kindLabel": "GPT" })), "Code Reviewer (GPT)");
        assert_eq!(bot_name(p, &json!({ "name": "" })), "ChatGPT");
        assert_eq!(bot_name(product("muse").unwrap(), &json!({ "name": "Muse", "kind": "channel", "kindLabel": "WhatsApp" })), "Muse");
    }
}
