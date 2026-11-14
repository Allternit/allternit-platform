//! Account connections for the connect kit (CONTRACT-connections-2026-11,
//! sections 3–5): one row per connectable thing, built from the records the
//! account already has. No table of its own.
//!
//! [`load_facts`] reads the sources; [`build_rows`] turns them into rows and
//! holds the rules (status words, unavailable rows, fallbacks), so the rules
//! are testable without a database.
//!
//! Sources (cloud-api, v1):
//! - `allternit`: `billing_subscriptions` (plan name).
//! - `*-key`, `gemini`, `api-key`: `user_inference_keys` (`gemini` = `google`).
//! - `openai-voice`, `elevenlabs`: `user_voice_keys` (migration 128).
//! - `cloud-computer`: `provisioned_instances` (paid and credit computers) and
//!   an active plan whose computer is not made yet.
//! - `this-mac`, `server`: the account's own relay-capable `runtime_devices`
//!   (`runtime_type` `desktop` / `vps`) that no provisioned computer owns.
//! - `phone`: `apns_device_tokens`, `runtime_devices` of type `ios`.
//! - `phone-control`: `app_phones`, `registered_phones`.
//! - `slack`: `slack_installs`. `discord`: `discord_installs`. `whatsapp`:
//!   `whatsapp_es_accounts`. `telegram`: `telegram_onboarding`.
//! - `gmail`, `drive`, `github`, `notion`: no cloud record; their connectors
//!   live on the computer, so the cloud row is always `off` and the runtime
//!   row answers.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::error::ApiError;
use crate::services::brain_runners::{Vendor, VendorState, DISCLOSURE_VERSION};

pub const VERSION: u32 = 1;

/// Same heartbeat grace as node resolution and compute-status.
const ENV_STALE_AFTER_SECS: &str = "ALLTERNIT_NODE_STALE_AFTER_SECS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Off,
    Connecting,
    On,
    Needs,
}

/// One row of `GET /api/v1/connections` (contract section 3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub id: String,
    pub group: &'static str,
    pub state: State,
    pub found: Option<String>,
    pub reason: Option<String>,
    pub reason_code: Option<&'static str>,
    pub runs_on: &'static str,
    pub available: bool,
    pub unavailable_reason: Option<&'static str>,
    pub fallbacks: Vec<&'static str>,
    pub terms: &'static str,
    pub saved: Option<&'static str>,
    pub models: Vec<String>,
    pub source: &'static str,
    /// Runner rows only (contract 9.1): `{version, accepted_at}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disclosure: Option<DisclosureState>,
    /// Runner rows only (contract 9.1): keep these models out of bot,
    /// routine, schedule, mission and sub-agent pickers.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub person_only: bool,
}

/// A runner row's `disclosure` field (contract 9.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DisclosureState {
    pub version: &'static str,
    pub accepted_at: Option<String>,
}

/// What [`build_rows`] knows about one brain runner vendor (contract 9).
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerVendor {
    pub vendor: Vendor,
    /// The server offers it (kill switch on, vendor in `ALLTERNIT_RUNNER_VENDORS`).
    pub offered: bool,
    pub state: VendorState,
    /// When this account accepted the current disclosure version.
    pub accepted_at: Option<DateTime<Utc>>,
}

/// The vendor ids each plan offers in chat, as `plan/<vendor>/<model>`
/// (FINDINGS-brain-runner-0.2: the `--model` / `-m` values that worked).
pub fn plan_models(vendor: Vendor) -> &'static [&'static str] {
    match vendor {
        Vendor::Claude => &["sonnet", "opus", "haiku"],
        Vendor::Chatgpt => &["gpt-6.1-sol", "gpt-6-luna"],
        Vendor::Grok => &["grok-4.6", "grok-4.5"],
        Vendor::Kimi => &["kimi-code/kimi-for-coding", "kimi-code/k3"],
    }
}

/// The key row a plan row falls back to (contract 9.1).
pub fn plan_fallback(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Claude => "anthropic-key",
        Vendor::Chatgpt => "openai-key",
        Vendor::Grok => "xai-key",
        Vendor::Kimi => "kimi-key",
    }
}

impl Row {
    fn new(id: &str, group: &'static str, runs_on: &'static str) -> Self {
        Self {
            id: id.to_string(),
            group,
            state: State::Off,
            found: None,
            reason: None,
            reason_code: None,
            runs_on,
            available: true,
            unavailable_reason: None,
            fallbacks: Vec::new(),
            terms: "cleared",
            saved: None,
            models: Vec::new(),
            source: "cloud",
            disclosure: None,
            person_only: false,
        }
    }

    fn on(mut self, found: impl Into<String>) -> Self {
        self.state = State::On;
        self.found = Some(found.into());
        self
    }

    fn connecting(mut self) -> Self {
        self.state = State::Connecting;
        self
    }

    fn needs(mut self, code: &'static str) -> Self {
        self.state = State::Needs;
        self.reason = Some(reason_text(code).to_string());
        self.reason_code = Some(code);
        self
    }

    fn unavailable(mut self, reason: &'static str, code: &'static str, fallbacks: &[&'static str]) -> Self {
        self.available = false;
        self.state = State::Off;
        self.unavailable_reason = Some(reason);
        self.reason_code = Some(code);
        self.fallbacks = fallbacks.to_vec();
        self
    }
}

/// Contract section 5. `vendor_limited` takes the vendor's name at the client.
pub fn reason_text(code: &str) -> &'static str {
    match code {
        "login_expired" => "Sign-in expired",
        "key_rejected" => "The key was rejected",
        "vendor_limited" => "This vendor limited this plan",
        "no_computer" => "Needs your own computer for now",
        "not_available_yet" => "Not available yet",
        "computer_offline" => "Your computer is offline",
        "update_app" => "Update the app to connect this plan.",
        "runner_busy" => "Busy. Try again",
        _ => "Could not start. Try again",
    }
}

/// Key rows: (row id, `user_inference_keys.provider_id`).
pub const KEY_ROWS: &[(&str, &str)] = &[
    ("openai-key", "openai"),
    ("anthropic-key", "anthropic"),
    ("google-key", "google"),
    ("xai-key", "xai"),
    ("together-key", "together"),
    ("fireworks-key", "fireworks"),
    ("deepinfra-key", "deepinfra"),
    ("groq-key", "groq"),
    ("deepseek-key", "deepseek"),
    ("kimi-key", "kimi"),
    ("openrouter-key", "openrouter"),
];

/// Voice rows: (row id, `user_voice_keys.provider`).
pub const VOICE_ROWS: &[(&str, &str)] = &[("openai-voice", "openai"), ("elevenlabs", "elevenlabs")];

/// Coding tools whose live state comes from the runtime.
pub const TOOL_ROWS: &[&str] = &["claude-code", "codex", "gemini-cli", "cursor", "grok-cli", "kimi-cli"];

/// Rows the person's computer owns; the cloud lists them only while no
/// computer is online (as unavailable).
pub fn runtime_owned(id: &str) -> bool {
    id == "chatgpt" || id == "local" || TOOL_ROWS.contains(&id)
}

/// The inference provider a key row (or the `gemini` alias) stores into.
pub fn key_provider_for(id: &str) -> Option<&'static str> {
    if id == "gemini" {
        return Some("google");
    }
    KEY_ROWS.iter().find(|(row, _)| *row == id).map(|(_, provider)| *provider)
}

pub fn voice_provider_for(id: &str) -> Option<&'static str> {
    VOICE_ROWS.iter().find(|(row, _)| *row == id).map(|(_, provider)| *provider)
}

/// Where to get a key for a key or voice row (the sheet's "Open the key page").
pub fn key_page(id: &str) -> &'static str {
    match key_provider_for(id).or_else(|| voice_provider_for(id)) {
        Some("openai") => "https://platform.openai.com/api-keys",
        Some("anthropic") => "https://console.anthropic.com/settings/keys",
        Some("google") => "https://aistudio.google.com/apikey",
        Some("xai") => "https://console.x.ai",
        Some("together") => "https://api.together.ai/settings/api-keys",
        Some("fireworks") => "https://fireworks.ai/account/api-keys",
        Some("deepinfra") => "https://deepinfra.com/dash/api_keys",
        Some("groq") => "https://console.groq.com/keys",
        Some("deepseek") => "https://platform.deepseek.com/api_keys",
        Some("kimi") => "https://platform.moonshot.ai/console/api-keys",
        Some("openrouter") => "https://openrouter.ai/settings/keys",
        Some("elevenlabs") => "https://elevenlabs.io/app/settings/api-keys",
        _ => "https://docs.allternit.com/guides/connections",
    }
}

/// A saved inference key, masked.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedKey {
    pub provider: String,
    pub masked: String,
    pub active: bool,
    /// The vendor's own model ids from the last check (no prefix).
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudComputer {
    None,
    Live(&'static str),
    Starting,
    Stopped,
    Failed,
}

/// Everything [`build_rows`] needs, read once per request.
#[derive(Debug, Clone)]
pub struct Facts {
    /// `Free`, `Plus`, `Max` or `Ultra`.
    pub plan: &'static str,
    pub keys: Vec<SavedKey>,
    pub voice: Vec<String>,
    pub cloud_computer: CloudComputer,
    /// The cloud computer is running and its runtime is connected.
    pub cloud_computer_online: bool,
    /// Own computers by kind (`desktop` → this-mac, `vps` → server): (name, online).
    pub desktops: Vec<(String, bool)>,
    pub servers: Vec<(String, bool)>,
    pub phone_app: bool,
    pub controlled_phones: Vec<String>,
    pub slack: Vec<String>,
    pub discord: Vec<String>,
    pub whatsapp: bool,
    pub telegram: Option<TelegramState>,
    /// Brain runner vendors (contract 9). Empty = the section 3 rows.
    pub runners: Vec<RunnerVendor>,
    /// The client sent `caps=runner` (contract 9.1, D10).
    pub runner_caps: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramState {
    Connected(Option<String>),
    Waiting,
}

impl Facts {
    /// Some computer of the account is online right now.
    pub fn computer_online(&self) -> bool {
        self.cloud_computer_online
            || self.desktops.iter().any(|(_, online)| *online)
            || self.servers.iter().any(|(_, online)| *online)
    }
}

fn plan_label(plan_id: &str) -> &'static str {
    // Same names as billing_subscriptions::product_label.
    match plan_id {
        "plus" => "Plus",
        "super" => "Max",
        "ultra" => "Ultra",
        _ => "Free",
    }
}

const RUNS_ON_COMPUTER: &str = "Runs on a computer.";
const NOT_YET: &str = "Not available yet. Use a key for now.";

/// One plan row (contract 9.1). `runner` is `None` when the server does not
/// offer the vendor (kill switch, vendor list): the section 3 row.
fn runner_row(vendor: Vendor, runner: Option<&RunnerVendor>, caps: bool) -> Row {
    let fallback = plan_fallback(vendor);
    let mut row = Row::new(vendor.row_id(), "brains", "runner");
    row.terms = "pending";
    row.saved = Some("runner");
    let Some(runner) = runner else {
        return row.unavailable(NOT_YET, "not_available_yet", &[fallback]);
    };
    if !caps {
        return row.unavailable("Update the app to connect this plan.", "update_app", &[fallback]);
    }
    row.person_only = true;
    row.fallbacks = vec![fallback];
    row.disclosure = Some(DisclosureState {
        version: DISCLOSURE_VERSION,
        accepted_at: runner.accepted_at.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    });
    match &runner.state {
        VendorState::Off => row,
        VendorState::Connecting => row.connecting(),
        VendorState::On { found } => {
            row.models = plan_models(vendor).iter().map(|m| format!("plan/{}/{m}", vendor.row_id())).collect();
            row.on(found.clone().unwrap_or_else(|| "Signed in".to_string()))
        }
        VendorState::Expired => row.needs("login_expired"),
        VendorState::Limited => row.needs("vendor_limited"),
    }
}

/// The plan rows' facts for an account: what the server offers, each
/// vendor's sign-in state (without waking the box) and the acceptances of
/// the current disclosure.
pub async fn load_runner_facts(
    db: &PgPool,
    service: &crate::services::brain_runners::BrainRunnerService,
    user_id: &str,
) -> Result<Vec<RunnerVendor>, ApiError> {
    let states = service.vendor_state(user_id).await?;
    let accepted: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT vendor, accepted_at FROM plan_runner_consents WHERE user_id = $1 AND disclosure_version = $2",
    )
    .bind(user_id)
    .bind(DISCLOSURE_VERSION)
    .fetch_all(db)
    .await?;
    Ok(states
        .into_iter()
        .map(|(vendor, state)| RunnerVendor {
            vendor,
            offered: service.offers(vendor),
            state,
            accepted_at: accepted.iter().find(|(v, _)| v == vendor.row_id()).map(|(_, at)| *at),
        })
        .collect())
}

/// The rows for an account. `aliases_for` gives the catalog models a key for
/// a provider unlocks.
pub fn build_rows(facts: &Facts, aliases_for: &dyn Fn(&str) -> Vec<String>) -> Vec<Row> {
    let mut rows = Vec::new();
    let online = facts.computer_online();

    // Brains.
    let found = if facts.plan == "Free" { "Free · $2 a month free".to_string() } else { facts.plan.to_string() };
    rows.push(Row::new("allternit", "brains", "server").on(found));

    // Rows the computer owns (ChatGPT, the local brain, coding tools): with a
    // computer online they are left out, so the runtime's row is the one the
    // client shows (clients let a runtime row win over a cloud row only when
    // it is `on`; a cloud row would hide a usable, not yet signed-in one).
    let chatgpt_runner = facts.runners.iter().any(|r| r.vendor == Vendor::Chatgpt && r.offered);
    if !online && !chatgpt_runner {
        rows.push(Row::new("chatgpt", "brains", "computer").unavailable(
            "Needs your own computer for now. Or use an OpenAI key.",
            "no_computer",
            &["this-mac", "openai-key"],
        ));
    }
    for vendor in Vendor::ALL {
        let runner = facts.runners.iter().find(|r| r.vendor == vendor).filter(|r| r.offered);
        // ChatGPT with a computer online is the runtime's row (contract 9.9
        // precedence); without the runner it is the section 3 row above.
        if vendor == Vendor::Chatgpt && (runner.is_none() || online) {
            continue;
        }
        rows.push(runner_row(vendor, runner, facts.runner_caps));
    }
    let mut copilot = Row::new("copilot", "brains", "server").unavailable("Coming soon.", "not_available_yet", &["github"]);
    copilot.terms = "pending";
    rows.push(copilot);
    if !online {
        rows.push(Row::new("local", "brains", "computer").unavailable(RUNS_ON_COMPUTER, "no_computer", &["this-mac"]));
    }

    let key_row = |id: &str, provider: &str| -> Row {
        let mut row = Row::new(id, "brains", "server");
        row.saved = Some("account");
        match facts.keys.iter().find(|key| key.provider == provider) {
            Some(key) if key.active => {
                // Catalog aliases where the router has them (OpenRouter and the
                // other hosts); otherwise the vendor's own list as
                // `<provider>/<model>`, which routes on this key.
                row.models = aliases_for(provider);
                if row.models.is_empty() {
                    row.models = key.models.iter().map(|model| format!("{provider}/{model}")).collect();
                }
                row.on(format!("Key works · {}", key.masked))
            }
            Some(_) => row.needs("key_rejected"),
            None => row,
        }
    };
    for (id, provider) in KEY_ROWS {
        rows.push(key_row(id, provider));
    }
    rows.push(key_row("gemini", "google"));
    let mut api_key = Row::new("api-key", "brains", "server");
    api_key.saved = Some("account");
    let working = facts.keys.iter().filter(|key| key.active).count();
    rows.push(match working {
        0 => api_key,
        1 => api_key.on("1 key works"),
        n => api_key.on(format!("{n} keys work")),
    });

    // Computers.
    let cloud = Row::new("cloud-computer", "computers", "server");
    rows.push(match facts.cloud_computer {
        CloudComputer::None => cloud,
        CloudComputer::Live(found) => cloud.on(found),
        CloudComputer::Starting => cloud.connecting(),
        CloudComputer::Stopped => {
            let mut row = cloud;
            row.state = State::Needs;
            row.reason = Some("Stopped. Add hours or credits to start it".to_string());
            row
        }
        CloudComputer::Failed => cloud.needs("could_not_start"),
    });
    for (id, machines) in [("this-mac", &facts.desktops), ("server", &facts.servers)] {
        let row = Row::new(id, "computers", "computer");
        rows.push(match machines.iter().find(|(_, online)| *online).or(machines.first()) {
            None => row,
            Some((name, true)) => row.on(name.clone()),
            Some((name, false)) => {
                let mut row = row.needs("computer_offline");
                row.found = Some(name.clone());
                row
            }
        });
    }

    // Phones.
    let phone = Row::new("phone", "phones", "device");
    rows.push(if facts.phone_app { phone.on("Signed in on your phone") } else { phone });
    let control = Row::new("phone-control", "phones", "device");
    rows.push(match facts.controlled_phones.as_slice() {
        [] => control,
        [one] => control.on(one.clone()),
        many => control.on(format!("{} phones", many.len())),
    });

    // Coding tools: live state comes from the runtime (left out when online).
    if !online {
        for id in TOOL_ROWS {
            rows.push(Row::new(id, "tools", "computer").unavailable(RUNS_ON_COMPUTER, "no_computer", &["this-mac"]));
        }
    }

    // Voice keys saved to the account.
    for (id, provider) in VOICE_ROWS {
        let mut row = Row::new(id, "voice", "server");
        row.saved = Some("account");
        rows.push(if facts.voice.iter().any(|p| p == provider) { row.on("Key saved") } else { row });
    }

    // Apps: connectors live on the computer (runtime rows), except Slack.
    for id in ["gmail", "drive", "github", "notion"] {
        rows.push(Row::new(id, "apps", "server"));
    }
    let slack = Row::new("slack", "apps", "server");
    rows.push(match facts.slack.as_slice() {
        [] => slack,
        [one] => slack.on(one.clone()),
        many => slack.on(format!("{} workspaces", many.len())),
    });

    // Channels.
    let telegram = Row::new("telegram", "channels", "server");
    rows.push(match &facts.telegram {
        None => telegram,
        Some(TelegramState::Waiting) => telegram.connecting(),
        Some(TelegramState::Connected(Some(name))) => telegram.on(format!("@{name}")),
        Some(TelegramState::Connected(None)) => telegram.on("Bot connected"),
    });
    let whatsapp = Row::new("whatsapp", "channels", "server");
    rows.push(if facts.whatsapp { whatsapp.on("Number connected") } else { whatsapp });
    let discord = Row::new("discord", "channels", "server");
    rows.push(match facts.discord.as_slice() {
        [] => discord,
        [one] => discord.on(one.clone()),
        many => discord.on(format!("{} servers", many.len())),
    });

    rows
}

/// Read every source for one account. `connected` = runtime ids with a live
/// relay connection.
pub async fn load_facts(
    db: &PgPool,
    user_id: &str,
    keys: Vec<SavedKey>,
    connected: &HashSet<String>,
    now: DateTime<Utc>,
) -> Result<Facts, ApiError> {
    let plan_id: Option<String> = sqlx::query_scalar(
        "SELECT plan_id FROM billing_subscriptions WHERE user_id = $1 AND status IN ('active', 'trialing') ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    let voice: Vec<String> = sqlx::query_scalar("SELECT provider FROM user_voice_keys WHERE user_id = $1 ORDER BY provider")
        .bind(user_id)
        .fetch_all(db)
        .await?;

    let stale_after = Duration::seconds(
        std::env::var(ENV_STALE_AFTER_SECS).ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(600),
    );
    let fresh = |id: &str, status: &Option<String>, seen: Option<DateTime<Utc>>| {
        connected.contains(id)
            || (status.as_deref() == Some("online") && seen.is_some_and(|seen| now - seen <= stale_after))
    };

    // Own computers: relay-capable, not revoked, not a provisioned computer.
    let devices: Vec<(String, Option<String>, Option<String>, Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT d.id, d.name, d.runtime_type, d.status, d.last_seen_at
        FROM runtime_devices d
        WHERE d.user_id = $1 AND d.revoked_at IS NULL
          AND d.capabilities LIKE '%"runtime:connect"%'
          AND NOT EXISTS (SELECT 1 FROM provisioned_instances i WHERE i.device_id = d.id)
        ORDER BY d.last_seen_at DESC NULLS LAST, d.id
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let mut desktops = Vec::new();
    let mut servers = Vec::new();
    for (id, name, runtime_type, status, seen) in &devices {
        let entry = (name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Your computer".to_string()), fresh(id, status, *seen));
        match runtime_type.as_deref() {
            Some("desktop") => desktops.push(entry),
            Some("vps") => servers.push(entry),
            _ => {}
        }
    }

    // Cloud computer: the newest live paid/credit computer.
    let instances: Vec<(String, Option<String>, Option<DateTime<Utc>>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT status, device_id, plan_overage_stopped_at, updated_at
        FROM provisioned_instances
        WHERE user_id = $1 AND tier IN ('paid', 'credit') AND status <> 'deleted' AND replaced_by IS NULL
        ORDER BY created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let live = instances
        .iter()
        .find(|(status, ..)| matches!(status.as_str(), "running" | "sleeping" | "waking" | "stopped"));
    let cloud_computer_online = live.is_some_and(|(status, device, ..)| {
        status == "running" && device.as_ref().is_some_and(|d| connected.contains(d))
    });
    let cloud_computer = if let Some((status, ..)) = live {
        CloudComputer::Live(match status.as_str() {
            "running" => "Running",
            "sleeping" => "Sleeping · wakes when needed",
            "waking" => "Waking up",
            _ => "Stopped · starts when needed",
        })
    } else if instances.iter().any(|(status, ..)| status == "provisioning") || plan_id.is_some() && instances.is_empty() {
        CloudComputer::Starting
    } else if instances.iter().any(|(status, ..)| status == "suspended") {
        CloudComputer::Stopped
    } else if instances
        .first()
        .is_some_and(|(status, _, _, updated)| status == "error" && now - *updated <= Duration::hours(24))
    {
        CloudComputer::Failed
    } else {
        CloudComputer::None
    };

    let phone_app: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (SELECT 1 FROM apns_device_tokens WHERE user_id = $1)
            OR EXISTS (SELECT 1 FROM runtime_devices WHERE user_id = $1 AND runtime_type = 'ios' AND revoked_at IS NULL)
        "#,
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;

    let controlled_phones: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(NULLIF(name, ''), 'Your phone') FROM app_phones WHERE account = $1 AND revoked_at IS NULL
        UNION ALL
        SELECT COALESCE(NULLIF(name, ''), NULLIF(model, ''), 'Your phone') FROM registered_phones WHERE account_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;

    let slack: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(team_name, ''), 'Slack workspace') FROM slack_installs WHERE user_id = $1 ORDER BY installed_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let discord: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(guild_name, ''), 'Discord server') FROM discord_installs WHERE user_id = $1 AND revoked_at IS NULL ORDER BY installed_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let whatsapp: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM whatsapp_es_accounts WHERE user_id = $1 AND revoked_at IS NULL)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    let telegram_rows: Vec<(String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT state, tg_bot_username, expires_at FROM telegram_onboarding WHERE user_id = $1 ORDER BY created_at DESC LIMIT 20",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let telegram = if let Some((_, name, _)) = telegram_rows.iter().find(|(state, ..)| state == "connected" || state == "paired") {
        Some(TelegramState::Connected(name.clone()))
    } else if telegram_rows
        .first()
        .is_some_and(|(state, _, expires)| (state == "waiting" || state == "created") && *expires > now)
    {
        Some(TelegramState::Waiting)
    } else {
        None
    };

    Ok(Facts {
        plan: plan_label(plan_id.as_deref().unwrap_or("")),
        keys,
        voice,
        cloud_computer,
        cloud_computer_online,
        desktops,
        servers,
        phone_app,
        controlled_phones,
        slack,
        discord,
        whatsapp,
        telegram,
        runners: Vec::new(),
        runner_caps: false,
    })
}

/// Masked saved keys from the key service's list (never the plaintext).
pub fn saved_keys(list: Vec<crate::services::InferenceKeyInfo>) -> Vec<SavedKey> {
    list.into_iter()
        .map(|key| SavedKey {
            active: key.status == "active",
            provider: key.provider_id,
            masked: key.masked,
            models: key.models,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            plan: "Free",
            keys: Vec::new(),
            voice: Vec::new(),
            cloud_computer: CloudComputer::None,
            cloud_computer_online: false,
            desktops: Vec::new(),
            servers: Vec::new(),
            phone_app: false,
            controlled_phones: Vec::new(),
            slack: Vec::new(),
            discord: Vec::new(),
            whatsapp: false,
            telegram: None,
            runners: Vec::new(),
            runner_caps: false,
        }
    }

    fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
        rows.iter().find(|row| row.id == id).unwrap_or_else(|| panic!("no row {id}"))
    }

    fn no_models(_: &str) -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn free_account_without_a_computer_gets_honest_unavailable_rows() {
        let rows = build_rows(&facts(), &no_models);
        assert_eq!(row(&rows, "allternit").found.as_deref(), Some("Free · $2 a month free"));
        let chatgpt = row(&rows, "chatgpt");
        assert!(!chatgpt.available);
        assert_eq!(chatgpt.fallbacks, vec!["this-mac", "openai-key"]);
        assert_eq!(chatgpt.reason_code, Some("no_computer"));
        let claude = row(&rows, "claude");
        assert_eq!((claude.available, claude.terms, claude.fallbacks.clone()), (false, "pending", vec!["anthropic-key"]));
        assert_eq!(row(&rows, "grok").fallbacks, vec!["xai-key"]);
        assert_eq!(row(&rows, "copilot").unavailable_reason, Some("Coming soon."));
        for id in TOOL_ROWS.iter().chain(["local"].iter()) {
            let tool = row(&rows, id);
            assert_eq!((tool.available, tool.unavailable_reason), (false, Some("Runs on a computer.")), "{id}");
        }
        // Every row uses one of the four words, and ids are unique.
        let ids: HashSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids.len(), rows.len());
    }

    #[test]
    fn an_online_computer_leaves_computer_rows_to_the_runtime() {
        let mut facts = facts();
        facts.desktops = vec![("Joe's Mac".to_string(), true)];
        let rows = build_rows(&facts, &no_models);
        for id in TOOL_ROWS.iter().chain(["chatgpt", "local"].iter()) {
            assert!(rows.iter().all(|row| row.id != *id), "{id} is the runtime's row while a computer is online");
        }
        // A running cloud computer counts the same.
        let mut cloud = super::tests::facts();
        cloud.cloud_computer = CloudComputer::Live("Running");
        cloud.cloud_computer_online = true;
        assert!(build_rows(&cloud, &no_models).iter().all(|row| row.id != "codex"));
        assert_eq!(row(&rows, "this-mac").state, State::On);
        assert_eq!(row(&rows, "this-mac").found.as_deref(), Some("Joe's Mac"));
        // The brain runner rows stay unavailable whatever computer is online.
        assert!(!row(&rows, "claude").available);

        facts.desktops = vec![("Joe's Mac".to_string(), false)];
        let rows = build_rows(&facts, &no_models);
        assert_eq!(row(&rows, "this-mac").reason_code, Some("computer_offline"));
        assert!(!row(&rows, "codex").available, "an offline computer does not count");
        assert!(!row(&rows, "chatgpt").available);
    }

    #[test]
    fn keys_show_masked_and_gemini_follows_the_google_key() {
        let mut facts = facts();
        facts.keys = vec![
            SavedKey { provider: "google".into(), masked: "AIz…9xQ1".into(), active: true, models: vec!["gemini-2.5-pro".into()] },
            SavedKey { provider: "groq".into(), masked: "gsk…0000".into(), active: false, models: Vec::new() },
            SavedKey { provider: "openrouter".into(), masked: "sk-…beef".into(), active: true, models: Vec::new() },
        ];
        let rows = build_rows(&facts, &|provider| {
            if provider == "openrouter" { vec!["claude-sonnet-5.5".to_string()] } else { Vec::new() }
        });
        let google = row(&rows, "google-key");
        assert_eq!(google.found.as_deref(), Some("Key works · AIz…9xQ1"));
        assert_eq!(google.models, vec!["google/gemini-2.5-pro"], "vendor models, as routable ids");
        assert_eq!(row(&rows, "openrouter-key").models, vec!["claude-sonnet-5.5"], "catalog aliases win");
        assert_eq!(row(&rows, "gemini").state, State::On);
        let groq = row(&rows, "groq-key");
        assert_eq!((groq.state, groq.reason_code), (State::Needs, Some("key_rejected")));
        assert_eq!(groq.reason.as_deref(), Some("The key was rejected"));
        assert_eq!(row(&rows, "api-key").found.as_deref(), Some("2 keys work"));
    }

    fn offered(vendor: Vendor, state: VendorState) -> RunnerVendor {
        RunnerVendor { vendor, offered: true, state, accepted_at: None }
    }

    #[test]
    fn runner_rows_need_caps_and_list_plan_models_only_when_on() {
        let mut facts = facts();
        facts.runners = vec![
            offered(Vendor::Claude, VendorState::On { found: Some("Max".into()) }),
            offered(Vendor::Chatgpt, VendorState::Off),
            offered(Vendor::Grok, VendorState::Expired),
            RunnerVendor { vendor: Vendor::Kimi, offered: false, state: VendorState::Off, accepted_at: None },
        ];
        // An older client (no caps=runner): every offered plan says update.
        let rows = build_rows(&facts, &no_models);
        let claude = row(&rows, "claude");
        assert_eq!((claude.available, claude.reason_code, claude.unavailable_reason), (false, Some("update_app"), Some("Update the app to connect this plan.")));
        assert!(claude.models.is_empty() && claude.disclosure.is_none() && !claude.person_only);
        assert_eq!(row(&rows, "chatgpt").reason_code, Some("update_app"), "the runner row replaces the no-computer text");
        assert_eq!(row(&rows, "kimi").reason_code, Some("not_available_yet"), "a vendor off the list keeps the section 3 row");

        facts.runner_caps = true;
        let rows = build_rows(&facts, &no_models);
        let claude = row(&rows, "claude");
        assert!(claude.available && claude.person_only);
        assert_eq!((claude.state, claude.found.as_deref(), claude.runs_on, claude.terms), (State::On, Some("Max"), "runner", "pending"));
        assert_eq!(claude.models, vec!["plan/claude/sonnet", "plan/claude/opus", "plan/claude/haiku"]);
        assert_eq!(claude.disclosure, Some(DisclosureState { version: DISCLOSURE_VERSION, accepted_at: None }));
        let chatgpt = row(&rows, "chatgpt");
        assert!(chatgpt.available && chatgpt.models.is_empty(), "no models until signed in");
        assert_eq!(chatgpt.fallbacks, vec!["openai-key"]);
        assert_eq!((row(&rows, "grok").state, row(&rows, "grok").reason_code), (State::Needs, Some("login_expired")));
        assert!(!row(&rows, "kimi").available);
        let json = serde_json::to_value(row(&rows, "allternit")).unwrap();
        assert!(json.get("disclosure").is_none() && json.get("person_only").is_none(), "other rows never carry them");
    }
}
