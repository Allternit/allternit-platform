//! Applying a team plan (`agents up|down|restore`): read the live panes,
//! deliver each Terminal bot into its pane workdir, spawn / stop panes through
//! the pane engine, and register bots with allternit-api.
//!
//! The plan itself stays pure ([`super::team_plan`]); this module is the only
//! place that acts on one. Every step reports what actually happened
//! ([`StepResult`]): nothing is retried, nothing falls back silently.
//!
//! * **Live state** comes from `allternit-factory pane status`, the pane
//!   engine's own list of `ao-<slug>` sessions with their workdir. A team bot's
//!   pane slug is `<bot>-<team>` ([`pane_slug`]); the pane id this module
//!   reports is that slug (it is what `pane kill|send|status` take, and what
//!   the pane's `ALLTERNIT_FACTORY_PANE_ID` holds). `pane visibility` lists
//!   engine agents without their workspace label, so it cannot be mapped back
//!   to a bot and is not used here.
//! * **Spawn** runs [`super::delivery::deliver`] into the workdir, then
//!   `pane spawn <bot>-<team> <workdir> env ALLTERNIT_FACTORY_BOT=… <harness> <argv…>`.
//!   No credential is ever put in that argv or env; the API tokens this
//!   process holds are removed from the pane engine's environment.
//! * **Bind** (hosted / vendor) and the registration of Terminal bots go
//!   through allternit-api `POST /api/v1/factory/bots` ([`FactoryApi`]).
//! * **`--on <computer>`**: the pane engine has no remote spawn yet (saved
//!   machines are SSH profiles you attach to by hand), so a spawn step with a
//!   machine is refused with that fact. The plan still shows the machine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::delivery::{self, BotProfile, DeliveryRequest, Field, GateTarget, Label};
use super::team::{self, Binding, EffectiveBot, LoadedTeam};
use super::team_plan::{LiveState, PlanAction, TeamPlanStep};
use super::whoami::{ENV_BOT, ENV_BOT_ID, ENV_PANE_ID, ENV_TEAM};

/// Engine binary override (else the current executable).
pub const ENV_FACTORY_BIN: &str = "ALLTERNIT_FACTORY_BIN";
/// Allternit MCP connector URL delivered into Terminal bots' tool config.
pub const ENV_MCP_URL: &str = "ALLTERNIT_FACTORY_MCP_URL";
/// allternit-api base URL (`http://127.0.0.1:3000`).
pub const ENV_API_URL: &str = "ALLTERNIT_API_URL";
/// Desktop access token (sent as `x-allternit-desktop-access-token`).
pub const ENV_DESKTOP_TOKEN: &str = "ALLTERNIT_DESKTOP_ACCESS_TOKEN";
/// Owner user id sent with the desktop token (`x-allternit-user-id`).
pub const ENV_USER_ID: &str = "ALLTERNIT_USER_ID";
/// Bearer token (`Authorization: Bearer …`), the alternative to the desktop pair.
pub const ENV_API_TOKEN: &str = "ALLTERNIT_API_TOKEN";

/// Credential env vars: never forwarded to the pane engine, and checked as a
/// backstop against every file delivery writes.
const SECRET_ENV: [&str; 2] = [ENV_DESKTOP_TOKEN, ENV_API_TOKEN];

/// What to do when the API is not configured, for the step facts and the CLI.
pub const API_NOT_SET_FACT: &str = "ALLTERNIT_API_URL is not set";
pub const API_ENV_ACTION: &str = "Set ALLTERNIT_API_URL (allternit-api, e.g. http://127.0.0.1:3000) and either ALLTERNIT_API_TOKEN, or ALLTERNIT_DESKTOP_ACCESS_TOKEN with ALLTERNIT_USER_ID.";

/// The engine binary: `$ALLTERNIT_FACTORY_BIN`, else the current executable.
pub fn engine_bin() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(ENV_FACTORY_BIN).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    std::env::current_exe().context("cannot locate the allternit-factory binary (set ALLTERNIT_FACTORY_BIN)")
}

/// Pane slug of a team bot: `<bot>-<team>`.
pub fn pane_slug(bot: &str, team: &str) -> String {
    format!("{bot}-{team}")
}

/// Pane slug for a bot address `bot@team` (or a bare slug).
pub fn pane_slug_for_address(address: &str) -> String {
    match address.split_once('@') {
        Some((bot, team)) => pane_slug(bot, team),
        None => address.to_string(),
    }
}

// ─── Live state ────────────────────────────────────────────────────────────────

/// One `ao-<slug>` session as `pane status` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSession {
    pub slug: String,
    pub alive: bool,
    pub cwd: PathBuf,
}

/// Parse `pane status` (no args): `ao-<slug>  alive|DEAD  <cwd>` per line.
pub fn parse_pane_status(text: &str) -> Vec<PaneSession> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once("  ")?;
            let slug = name.trim().strip_prefix("ao-")?;
            let (state, cwd) = rest.trim_start().split_once("  ").unwrap_or((rest.trim(), "-"));
            Some(PaneSession {
                slug: slug.to_string(),
                alive: state.trim() == "alive",
                cwd: PathBuf::from(cwd.trim()),
            })
        })
        .collect()
}

/// Run `allternit-factory pane status` and parse it. A pane engine that is
/// not running lists no live session (that is a fact, not an error).
pub fn pane_sessions() -> Result<Vec<PaneSession>> {
    let bin = engine_bin()?;
    let out = Command::new(&bin)
        .args(["pane", "status"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {} pane status", bin.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        return Err(anyhow!("pane status failed ({}): {last}", out.status));
    }
    Ok(parse_pane_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Map live sessions to a team's [`LiveState`]: a session `<bot>-<team>` is
/// the bot `bot@team`. Sessions for bots no longer in `team.yaml` are
/// included (so `down` stops them) unless the slug belongs to a bot of
/// another team on disk (`other_teams`: `(team, bots)`), which keeps `down`
/// of team `build` away from `x-product-build`.
pub fn live_state_from(
    team: &LoadedTeam,
    sessions: &[PaneSession],
    other_teams: &[(String, Vec<String>)],
) -> LiveState {
    let mut live = LiveState::default();
    let suffix = format!("-{}", team.name);
    let others: BTreeSet<String> = other_teams
        .iter()
        .filter(|(t, _)| t != &team.name)
        .flat_map(|(t, bots)| bots.iter().map(move |b| pane_slug(b, t)))
        .collect();
    let listed: BTreeSet<&str> = team.file.bots.iter().map(|b| b.bot.as_str()).collect();
    for s in sessions.iter().filter(|s| s.alive) {
        let Some(bot) = s.slug.strip_suffix(&suffix) else { continue };
        if !team::valid_slug(bot) {
            continue;
        }
        if !listed.contains(bot) && others.contains(&s.slug) {
            continue;
        }
        let addr = team::address(bot, &team.name);
        live.panes.insert(addr.clone(), s.slug.clone());
        if s.cwd.is_absolute() {
            live.workdirs.insert(addr, s.cwd.clone());
        }
    }
    live
}

/// The live state of `team` under `root`, read from the pane engine.
pub fn live_state(root: &Path, team: &LoadedTeam) -> Result<LiveState> {
    let sessions = pane_sessions()?;
    let others: Vec<(String, Vec<String>)> = team::list_teams(root)
        .into_iter()
        .filter(|t| t != &team.name)
        .filter_map(|t| {
            let loaded = team::load_team(root, &t).ok()?;
            Some((t, loaded.file.bots.iter().map(|b| b.bot.clone()).collect()))
        })
        .collect();
    Ok(live_state_from(team, &sessions, &others))
}

// ─── allternit-api client ──────────────────────────────────────────────────────

/// An allternit-api failure, classified with the CLI's error codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// `transport` | `timeout` | `refused` | `not_found` | `usage` | `internal`.
    pub code: String,
    pub status: Option<u16>,
    pub fact: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.fact)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    pub fn new(code: &str, status: Option<u16>, fact: impl Into<String>) -> Self {
        Self { code: code.to_string(), status, fact: fact.into() }
    }
}

/// The allternit-api calls the engine makes. A trait so drive and `up` can be
/// tested with a fake.
pub trait FactoryApi: Send + Sync {
    /// `POST /api/v1/factory/bots` → `{bot, binding, created}`.
    fn upsert_bot(&self, body: &Value) -> std::result::Result<Value, ApiError>;
    /// `GET /api/v1/factory/bots` → the owner's Factory bots.
    fn list_bots(&self) -> std::result::Result<Vec<Value>, ApiError>;
    /// `POST /api/v1/factory/node-tickets` → `{ticket, lane, guarantee, created, …}`.
    fn create_node_ticket(&self, body: &Value) -> std::result::Result<Value, ApiError>;
}

#[derive(Debug, Clone)]
enum Auth {
    Desktop { token: String, user: String },
    Bearer(String),
}

/// Blocking allternit-api client. Each call runs on its own thread, so it is
/// safe to use from inside an async runtime too.
#[derive(Debug, Clone)]
pub struct ApiClient {
    base: String,
    auth: Auth,
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl ApiClient {
    /// From the environment. `Ok(None)` when `ALLTERNIT_API_URL` is unset;
    /// an error when it is set without credentials.
    pub fn from_env() -> std::result::Result<Option<Self>, ApiError> {
        Self::from_lookup(env_nonempty)
    }

    /// [`ApiClient::from_env`] over any lookup (tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> std::result::Result<Option<Self>, ApiError> {
        let Some(base) = get(ENV_API_URL) else { return Ok(None) };
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(ApiError::new("usage", None, format!("{ENV_API_URL} must be an http(s) URL, got {base:?}")));
        }
        let auth = match (get(ENV_DESKTOP_TOKEN), get(ENV_USER_ID), get(ENV_API_TOKEN)) {
            (Some(token), Some(user), _) => Auth::Desktop { token, user },
            (_, _, Some(token)) => Auth::Bearer(token),
            (Some(_), None, None) => {
                return Err(ApiError::new("usage", None, format!("{ENV_DESKTOP_TOKEN} is set but {ENV_USER_ID} is not")))
            }
            _ => {
                return Err(ApiError::new(
                    "usage",
                    None,
                    format!("{ENV_API_URL} is set but no credentials are: set {ENV_API_TOKEN}, or {ENV_DESKTOP_TOKEN} with {ENV_USER_ID}"),
                ))
            }
        };
        Ok(Some(Self { base: base.trim_end_matches('/').to_string(), auth }))
    }

    /// From the link allternit-api's proxy passes per request (its own base and
    /// the caller's credentials). `None` without a base or credentials.
    pub fn from_link(link: &crate::send::ApiLink) -> Option<Self> {
        let base = link.base.as_deref()?.trim_end_matches('/').to_string();
        let auth = match (&link.desktop, &link.authorization) {
            (Some((token, user)), _) => Auth::Desktop { token: token.clone(), user: user.clone() },
            (None, Some(h)) => {
                let token = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")).unwrap_or(h);
                Auth::Bearer(token.trim().to_string())
            }
            (None, None) => return None,
        };
        Some(Self { base, auth })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn call(&self, method: &'static str, path: &str, body: Option<Value>) -> std::result::Result<Value, ApiError> {
        let url = format!("{}{}", self.base, path);
        let auth = self.auth.clone();
        // reqwest::blocking must not run on an async worker thread.
        std::thread::scope(|s| {
            s.spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .map_err(|e| ApiError::new("internal", None, format!("http client: {e}")))?;
                let mut req = match method {
                    "GET" => client.get(&url),
                    _ => client.post(&url),
                };
                req = match &auth {
                    Auth::Desktop { token, user } => req
                        .header("x-allternit-desktop-access-token", token)
                        .header("x-allternit-user-id", user),
                    Auth::Bearer(token) => req.bearer_auth(token),
                };
                if let Some(b) = body {
                    req = req.json(&b);
                }
                let resp = req.send().map_err(|e| {
                    if e.is_timeout() {
                        ApiError::new("timeout", None, format!("allternit-api at {url} timed out"))
                    } else {
                        ApiError::new("transport", None, format!("allternit-api at {url} is not reachable: {e}"))
                    }
                })?;
                let status = resp.status().as_u16();
                let text = resp.text().unwrap_or_default();
                let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "text": text }));
                if (200..300).contains(&status) {
                    return Ok(value);
                }
                let msg = match &value["error"] {
                    Value::String(s) => s.clone(),
                    Value::Object(o) => o.get("fact").and_then(Value::as_str).unwrap_or("request failed").to_string(),
                    _ => value["text"].as_str().map(str::to_string).unwrap_or_else(|| "request failed".into()),
                };
                let code = match status {
                    400 | 422 => "usage",
                    401 | 403 | 409 => "refused",
                    404 => "not_found",
                    504 => "timeout",
                    502 | 503 => "transport",
                    _ => "internal",
                };
                Err(ApiError::new(code, Some(status), format!("allternit-api {method} {path} → {status}: {msg}")))
            })
            .join()
            .unwrap_or_else(|_| Err(ApiError::new("internal", None, "http thread panicked")))
        })
    }
}

impl FactoryApi for ApiClient {
    fn upsert_bot(&self, body: &Value) -> std::result::Result<Value, ApiError> {
        self.call("POST", "/api/v1/factory/bots", Some(body.clone()))
    }
    fn list_bots(&self) -> std::result::Result<Vec<Value>, ApiError> {
        let v = self.call("GET", "/api/v1/factory/bots", None)?;
        Ok(v["bots"].as_array().cloned().or_else(|| v.as_array().cloned()).unwrap_or_default())
    }
    fn create_node_ticket(&self, body: &Value) -> std::result::Result<Value, ApiError> {
        self.call("POST", "/api/v1/factory/node-tickets", Some(body.clone()))
    }
}

/// A node delivered to a vendor bot as a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorTicket {
    /// `T-n`.
    pub ticket: String,
    pub lane: Option<String>,
    /// `exact` | `best_effort` | `read_only`; `None` when no lane could take it.
    pub guarantee: Option<String>,
    pub created: bool,
    pub nudge_sent: bool,
}

/// `POST /api/v1/factory/node-tickets` for a picked-up node.
#[allow(clippy::too_many_arguments)]
pub fn vendor_ticket_for_node(
    api: &dyn FactoryApi,
    bot_slug: &str,
    dag_id: &str,
    node_id: &str,
    wih_id: &str,
    workspace_root: &Path,
    title: &str,
    instructions: &str,
) -> std::result::Result<VendorTicket, ApiError> {
    let root = workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
    let body = json!({
        "botSlug": bot_slug,
        "dagId": dag_id,
        "nodeId": node_id,
        "wihId": wih_id,
        "workspaceRoot": root.to_string_lossy(),
        "title": title,
        "instructions": instructions,
    });
    let v = api.create_node_ticket(&body)?;
    let ticket = v["ticket"]
        .as_str()
        .or_else(|| v["ticketId"].as_str())
        .map(str::to_string)
        .ok_or_else(|| ApiError::new("internal", None, format!("allternit-api returned no ticket id: {v}")))?;
    Ok(VendorTicket {
        ticket,
        lane: v["lane"].as_str().map(str::to_string),
        guarantee: v["guarantee"].as_str().map(str::to_string),
        created: v["created"].as_bool().unwrap_or(false),
        nudge_sent: v["nudgeSent"].as_bool().unwrap_or(false),
    })
}

/// The `binding` body for `POST /api/v1/factory/bots` from a team bot.
pub fn binding_body(b: &EffectiveBot, team: &str, pane: Option<&str>, directing_bot_id: Option<&str>) -> Value {
    match b.binding {
        Binding::Terminal => json!({
            "type": "terminal",
            "harness": b.harness,
            "machine": b.machine,
            "paneId": pane,
        }),
        Binding::Hosted => json!({ "type": "hosted" }),
        Binding::Vendor => json!({
            "type": "vendor",
            "vendor": b.vendor,
            "mode": b.mode,
            "lane": b.lane,
            "directingBotId": directing_bot_id,
            // The directing bot's address, so a reader can see who directs it
            // even before the id resolves.
            "directedBy": b.directed_by.as_deref().map(|d| team::address(d, team)),
        }),
    }
}

/// Vendor bots of `team` under `preset`, by slug (for `drive --team`).
pub fn vendor_bots(team: &LoadedTeam, preset: Option<&str>) -> Result<BTreeMap<String, crate::drive::VendorInfo>> {
    Ok(team
        .effective_bots(preset)?
        .into_iter()
        .filter(|b| b.binding == Binding::Vendor)
        .map(|b| {
            (
                b.slug.clone(),
                crate::drive::VendorInfo { vendor: b.vendor, lane: b.lane, team: Some(team.name.clone()) },
            )
        })
        .collect())
}

// ─── Apply ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepOutcome {
    Ok,
    Failed,
    Skipped,
}

/// What one plan step did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepResult {
    pub step: TeamPlanStep,
    pub outcome: StepOutcome,
    /// Error code (`transport`, `refused`, `not_found`, `internal`, …) when failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub fact: String,
    /// Delivery labels for a spawned Terminal bot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<BTreeMap<Field, Label>>,
    /// Whether the bot is registered with allternit-api (null when not attempted).
    pub registered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<PathBuf>,
}

impl StepResult {
    fn new(step: &TeamPlanStep, outcome: StepOutcome, fact: impl Into<String>) -> Self {
        Self {
            step: step.clone(),
            outcome,
            code: None,
            fact: fact.into(),
            delivered: None,
            registered: None,
            bot_id: None,
            pane: None,
            workdir: None,
        }
    }

    fn failed(step: &TeamPlanStep, code: &str, fact: impl Into<String>) -> Self {
        let mut r = Self::new(step, StepOutcome::Failed, fact);
        r.code = Some(code.to_string());
        r
    }
}

/// Options for [`apply`].
#[derive(Clone, Default)]
pub struct ApplyOptions {
    /// Directory Terminal bots run in (default: the workspace root).
    pub workdir: Option<PathBuf>,
    /// Report what would happen; touch nothing.
    pub dry_run: bool,
    /// allternit-api, when configured.
    pub api: Option<Arc<dyn FactoryApi>>,
    /// `down`: also remove each pane's worktree.
    pub rm_worktree: bool,
}

/// The profile a team bot carries into its pane, from the team folder:
/// `bots/<bot>/PERSONA.md`, the role and `CULTURE.md`, skill folders from
/// `bots/<bot>/skills/*` (winning by name) and `skills/*`, `bots/<bot>/LEARNED.md`,
/// the model, and `$ALLTERNIT_FACTORY_MCP_URL`.
pub fn bot_profile(team: &LoadedTeam, bot: &EffectiveBot, bot_id: Option<&str>) -> BotProfile {
    let bot_dir = team.dir.join("bots").join(&bot.slug);
    let persona = std::fs::read_to_string(bot_dir.join("PERSONA.md")).unwrap_or_default();
    let role_instructions = team
        .culture
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(|c| format!("Team {} culture (CULTURE.md):\n\n{}", team.name, c.trim()));
    let mut skills: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in [team.dir.join("skills"), bot_dir.join("skills")] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    skills.insert(e.file_name().to_string_lossy().to_string(), e.path());
                }
            }
        }
    }
    BotProfile {
        id: bot_id.map(str::to_string).unwrap_or_else(|| bot.address.clone()),
        slug: bot.slug.clone(),
        team: Some(team.name.clone()),
        role: Some(bot.role.clone()),
        persona,
        role_instructions,
        twin_persona: None,
        memory_items: vec![],
        learned: std::fs::read_to_string(bot_dir.join("LEARNED.md")).ok(),
        skills_dirs: skills.into_values().collect(),
        model: bot.model.clone(),
        mcp_url: env_nonempty(ENV_MCP_URL),
        mcp_auth_env: None,
        autonomy: Default::default(),
        secret_values: SECRET_ENV.iter().filter_map(|k| env_nonempty(k)).collect(),
    }
}

/// Harnesses whose pane line the pane engine's spawn gate runs through
/// `/bin/sh` (claude: hook settings; codex: sandbox), so their args must be
/// shell-quoted. Other harnesses get their argv exec'd as is.
fn gate_runs_through_shell(harness: &str) -> bool {
    matches!(harness, "claude" | "claude-code" | "codex")
}

fn sh_quote(a: &str) -> String {
    if !a.is_empty() && a.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+%".contains(&b)) {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

/// The `pane spawn` argv (after the engine binary) for a Terminal bot.
pub fn spawn_argv(bot: &EffectiveBot, team: &str, workdir: &Path, bot_id: Option<&str>, harness_argv: &[String]) -> Vec<String> {
    let slug = pane_slug(&bot.slug, team);
    let harness = bot.harness.clone().unwrap_or_default();
    let mut argv = vec![
        "pane".to_string(),
        "spawn".into(),
        slug.clone(),
        workdir.display().to_string(),
        "env".into(),
        format!("{ENV_BOT}={}", bot.slug),
        format!("{ENV_TEAM}={team}"),
        format!("{ENV_PANE_ID}={slug}"),
    ];
    if let Some(id) = bot_id {
        argv.push(format!("{ENV_BOT_ID}={id}"));
    }
    argv.push(harness.clone());
    let quote = gate_runs_through_shell(&harness);
    argv.extend(harness_argv.iter().map(|a| if quote { sh_quote(a) } else { a.clone() }));
    argv
}

fn classify_pane_failure(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not running") || lower.contains("could not start") || lower.contains("connection refused") {
        "transport"
    } else if lower.contains("spawn gate") || lower.contains("refus") || lower.contains("already exists") {
        "refused"
    } else if lower.contains("no session") || lower.contains("not found") {
        "not_found"
    } else {
        "internal"
    }
}

fn run_pane(bin: &Path, root: &Path, argv: &[String]) -> std::result::Result<(), (String, String)> {
    let mut cmd = Command::new(bin);
    cmd.args(argv)
        .stdin(Stdio::null())
        .env("ALLTERNIT_FACTORY_ROOT", root)
        .env("ALLTERNIT_FACTORY_BIN", bin);
    for k in SECRET_ENV {
        cmd.env_remove(k);
    }
    let out = cmd.output().map_err(|e| ("internal".to_string(), format!("run {}: {e}", bin.display())))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let last = err
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches("error: ")
        .to_string();
    let code = classify_pane_failure(&err);
    Err((code.to_string(), if last.is_empty() { format!("pane {} exited {}", argv.first().map(String::as_str).unwrap_or(""), out.status) } else { last }))
}

/// Resolve `slug`'s bot id through the API (a director bound earlier in this
/// run, or already registered).
fn bot_id_via_api(api: &dyn FactoryApi, ids: &BTreeMap<String, String>, slug: &str) -> Option<String> {
    if let Some(id) = ids.get(slug) {
        return Some(id.clone());
    }
    api.list_bots()
        .ok()?
        .into_iter()
        .find(|b| b["slug"].as_str() == Some(slug) || b["bot"]["slug"].as_str() == Some(slug))
        .and_then(|b| b["id"].as_str().or_else(|| b["bot"]["id"].as_str()).map(str::to_string))
}

fn register(
    api: &dyn FactoryApi,
    bot: &EffectiveBot,
    team: &str,
    pane: Option<&str>,
    ids: &BTreeMap<String, String>,
) -> std::result::Result<(String, bool), ApiError> {
    let directing = bot.directed_by.as_deref().and_then(|d| bot_id_via_api(api, ids, d));
    let body = json!({
        "slug": bot.slug,
        "name": bot.slug,
        "role": bot.role,
        "binding": binding_body(bot, team, pane, directing.as_deref()),
        "idempotencyKey": format!("team:{team}:{}", bot.slug),
    });
    let v = api.upsert_bot(&body)?;
    let id = v["bot"]["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ApiError::new("internal", None, format!("allternit-api returned no bot id: {v}")))?;
    Ok((id, v["created"].as_bool().unwrap_or(false)))
}

/// Apply plan `steps` for `team` under `preset`. Steps run in order; a failed
/// step does not stop the next one (each result says what happened).
pub fn apply(
    root: &Path,
    team: &LoadedTeam,
    preset: Option<&str>,
    steps: &[TeamPlanStep],
    opts: &ApplyOptions,
) -> Vec<StepResult> {
    let bots: BTreeMap<String, EffectiveBot> = match team.effective_bots(preset) {
        Ok(b) => b.into_iter().map(|b| (b.address.clone(), b)).collect(),
        Err(e) => return steps.iter().map(|s| StepResult::failed(s, "usage", e.to_string())).collect(),
    };
    let bin = match engine_bin() {
        Ok(b) => b,
        Err(e) => return steps.iter().map(|s| StepResult::failed(s, "transport", format!("{e:#}"))).collect(),
    };
    // Absolute paths: they go into hook commands and the pane's cwd.
    let abs = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = &abs(root);
    let workdir = abs(&opts.workdir.clone().unwrap_or_else(|| root.to_path_buf()));
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    // Instruction files already claimed in a workdir this run (two bots must
    // not share one managed block).
    let mut claimed: BTreeMap<(PathBuf, &'static str), String> = BTreeMap::new();
    let mut out = Vec::with_capacity(steps.len());

    for step in steps {
        if opts.dry_run {
            out.push(StepResult::new(step, StepOutcome::Skipped, format!("dry run: would {}", step.reason)));
            continue;
        }
        let result = match step.action {
            PlanAction::Skip => StepResult::new(step, StepOutcome::Skipped, step.reason.clone()),
            PlanAction::Stop => {
                let slug = pane_slug_for_address(&step.agent);
                let mut argv = vec!["pane".to_string(), "kill".into(), slug.clone()];
                if opts.rm_worktree {
                    argv.push("--rm-worktree".into());
                }
                match run_pane(&bin, root, &argv) {
                    Ok(()) => {
                        let mut r = StepResult::new(step, StepOutcome::Ok, format!("stopped pane {slug}"));
                        r.pane = Some(slug);
                        r
                    }
                    Err((code, fact)) => StepResult::failed(step, &code, fact),
                }
            }
            PlanAction::Bind => {
                let Some(bot) = bots.get(&step.agent) else {
                    out.push(StepResult::failed(step, "not_found", format!("{} is not in team.yaml", step.agent)));
                    continue;
                };
                match &opts.api {
                    None => StepResult::failed(
                        step,
                        "transport",
                        format!("{API_NOT_SET_FACT}: {} bots are bound through allternit-api", bot.binding.as_str()),
                    ),
                    Some(api) => match register(api.as_ref(), bot, &team.name, None, &ids) {
                        Ok((id, created)) => {
                            ids.insert(bot.slug.clone(), id.clone());
                            let mut r = StepResult::new(
                                step,
                                StepOutcome::Ok,
                                format!("{} bot {} ({id})", bot.binding.as_str(), if created { "created and bound" } else { "rebound" }),
                            );
                            r.registered = Some(true);
                            r.bot_id = Some(id);
                            r
                        }
                        Err(e) => {
                            let mut r = StepResult::failed(step, &e.code, e.fact);
                            r.registered = Some(false);
                            r
                        }
                    },
                }
            }
            PlanAction::Spawn => {
                let Some(bot) = bots.get(&step.agent) else {
                    out.push(StepResult::failed(step, "not_found", format!("{} is not in team.yaml", step.agent)));
                    continue;
                };
                spawn_one(root, team, bot, step, &bin, &workdir, opts, &mut ids, &mut claimed)
            }
        };
        out.push(result);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn spawn_one(
    root: &Path,
    team: &LoadedTeam,
    bot: &EffectiveBot,
    step: &TeamPlanStep,
    bin: &Path,
    workdir: &Path,
    opts: &ApplyOptions,
    ids: &mut BTreeMap<String, String>,
    claimed: &mut BTreeMap<(PathBuf, &'static str), String>,
) -> StepResult {
    if let Some(machine) = &step.machine {
        return StepResult::failed(
            step,
            "not_found",
            format!(
                "spawning on another computer ({machine}) is not built yet: the pane engine has no remote spawn. \
                 Run `allternit-factory agents up {}` on {machine} itself, or drop --on / the bot's machine",
                team.name
            ),
        );
    }
    let harness = bot.harness.clone().unwrap_or_default();
    let slug = pane_slug(&bot.slug, &team.name);
    if let Some(file) = delivery::instruction_file(&harness) {
        let key = (workdir.to_path_buf(), file);
        if let Some(other) = claimed.get(&key) {
            return StepResult::failed(
                step,
                "refused",
                format!(
                    "{} and {other} would share {file} in {} (one managed block per file); give them separate workdirs (--workdir) or different harnesses",
                    bot.address,
                    workdir.display()
                ),
            );
        }
        claimed.insert(key, bot.address.clone());
    }

    // Register first so the pane carries its bot id.
    let (registered, bot_id, reg_fact) = match &opts.api {
        None => (false, None, format!("not registered: {API_NOT_SET_FACT}")),
        Some(api) => match register(api.as_ref(), bot, &team.name, Some(&slug), ids) {
            Ok((id, _)) => {
                ids.insert(bot.slug.clone(), id.clone());
                (true, Some(id.clone()), format!("registered as {id}"))
            }
            Err(e) => {
                let mut r = StepResult::failed(step, &e.code, format!("registration failed, pane not started: {}", e.fact));
                r.registered = Some(false);
                return r;
            }
        },
    };

    let profile = bot_profile(team, bot, bot_id.as_deref());
    let report = match delivery::deliver(&DeliveryRequest {
        profile: &profile,
        harness: &harness,
        workdir,
        context_pack_path: None,
        gate: Some(GateTarget { bin: bin.to_path_buf(), root: root.to_path_buf(), wih_id: None }),
    }) {
        Ok(r) => r,
        Err(e) => {
            let mut r = StepResult::failed(step, "internal", format!("delivery into {} failed: {e:#}", workdir.display()));
            r.registered = Some(registered);
            r.bot_id = bot_id;
            return r;
        }
    };

    let argv = spawn_argv(bot, &team.name, workdir, bot_id.as_deref(), &report.argv);
    let mut r = match run_pane(bin, root, &argv) {
        Ok(()) => StepResult::new(step, StepOutcome::Ok, format!("started pane {slug} ({harness}) in {}; {reg_fact}", workdir.display())),
        Err((code, fact)) => StepResult::failed(step, &code, format!("pane spawn failed: {fact}")),
    };
    r.delivered = Some(report.fields);
    r.registered = Some(registered);
    r.bot_id = bot_id;
    r.pane = Some(slug);
    r.workdir = Some(workdir.to_path_buf());
    r
}

/// Refuse a plan whose Terminal bots would share one instruction file in one
/// workdir (one managed block per file: the second delivery would replace
/// the first bot's persona). Counts the spawn steps and the team's bots
/// already live in `workdir`. Pure; `up --dry-run` and `up` both run it.
pub fn check_workdirs(
    team: &LoadedTeam,
    preset: Option<&str>,
    plan: &[TeamPlanStep],
    workdir: &Path,
    live: &LiveState,
) -> std::result::Result<(), String> {
    let bots = team.effective_bots(preset).map_err(|e| e.to_string())?;
    let workdir = &workdir.canonicalize().unwrap_or_else(|_| workdir.to_path_buf());
    let mut users: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for b in bots.iter().filter(|b| b.binding == Binding::Terminal) {
        let spawning = plan.iter().any(|s| s.action == PlanAction::Spawn && s.agent == b.address && s.machine.is_none());
        let live_here = live.workdirs.get(&b.address).map(|w| w == workdir).unwrap_or(false);
        if !(spawning || live_here) {
            continue;
        }
        if let Some(file) = b.harness.as_deref().and_then(delivery::instruction_file) {
            users.entry(file).or_default().push(b.address.clone());
        }
    }
    let clashes: Vec<String> = users
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(f, v)| format!("{} would share {f}", v.join(" and ")))
        .collect();
    if clashes.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} in {} (one managed block per file, so one bot's persona would replace the other's); give them separate workdirs (--workdir) or harnesses that read different files",
            clashes.join("; "),
            workdir.display()
        ))
    }
}

/// The most severe failure code among `results` (`None` when all ok/skipped).
/// Severity: internal, transport, timeout, refused, needs_person, not_found,
/// usage (an engine failure outranks an environment one, which outranks a
/// refusal or a missing thing).
pub fn worst_code(results: &[StepResult]) -> Option<String> {
    const ORDER: [&str; 7] = ["internal", "transport", "timeout", "refused", "needs_person", "not_found", "usage"];
    results
        .iter()
        .filter(|r| r.outcome == StepOutcome::Failed)
        .filter_map(|r| r.code.as_deref())
        .min_by_key(|c| ORDER.iter().position(|o| o == c).unwrap_or(0))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::team::{parse_team, tests::GOOD};
    use crate::agents::team_plan::plan_up;
    use std::sync::Mutex;

    #[test]
    fn pane_status_lines_parse() {
        let text = "ao-builder-product-build  alive  /w/repo\nao-old-x  DEAD  /tmp/a b\nno ao-* sessions\n";
        let s = parse_pane_status(text);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], PaneSession { slug: "builder-product-build".into(), alive: true, cwd: "/w/repo".into() });
        assert_eq!(s[1].cwd, PathBuf::from("/tmp/a b"));
        assert!(!s[1].alive);
    }

    #[test]
    fn live_state_maps_slugs_to_addresses_and_spares_other_teams() {
        let t = parse_team("product-build", GOOD).unwrap();
        let sessions = parse_pane_status(
            "ao-builder-product-build  alive  /w\nao-ghost-product-build  alive  /w\nao-x-product-build  alive  /w\nao-checker-product-build  DEAD  /w\n",
        );
        let others = vec![("build".to_string(), vec!["x-product".to_string()]), ("product".to_string(), vec!["x".to_string()])];
        // `x-product-build` is bot `x-product` of team `build`; this team
        // doesn't list `x`, so it stays out.
        let live = live_state_from(&t, &sessions, &others);
        assert_eq!(live.panes.get("builder@product-build").map(String::as_str), Some("builder-product-build"));
        assert!(live.panes.contains_key("ghost@product-build"));
        assert!(!live.panes.contains_key("x@product-build"));
        assert!(!live.panes.contains_key("checker@product-build"));
        assert_eq!(live.workdirs["builder@product-build"], PathBuf::from("/w"));
    }

    #[test]
    fn spawn_argv_carries_identity_and_no_secret() {
        let t = parse_team("product-build", GOOD).unwrap();
        let bots = t.effective_bots(Some("cheap")).unwrap();
        let argv = spawn_argv(&bots[1], "product-build", Path::new("/w"), Some("bot_1"), &["-c".into(), "hooks.x=[a b]".into()]);
        assert_eq!(&argv[..4], ["pane", "spawn", "builder-product-build", "/w"]);
        assert!(argv.contains(&"ALLTERNIT_FACTORY_BOT=builder".to_string()));
        assert!(argv.contains(&"ALLTERNIT_FACTORY_PANE_ID=builder-product-build".to_string()));
        assert!(argv.contains(&"ALLTERNIT_FACTORY_BOT_ID=bot_1".to_string()));
        assert_eq!(argv.last().unwrap(), "'hooks.x=[a b]'", "codex runs through sh: quoted");
    }

    #[test]
    fn api_from_env_needs_credentials() {
        assert!(ApiClient::from_lookup(|_| None).unwrap().is_none());
        let only_url = |k: &str| (k == ENV_API_URL).then(|| "http://127.0.0.1:3000".to_string());
        assert_eq!(ApiClient::from_lookup(only_url).unwrap_err().code, "usage");
        let bearer = |k: &str| match k {
            ENV_API_URL => Some("http://127.0.0.1:3000/".to_string()),
            ENV_API_TOKEN => Some("tok".to_string()),
            _ => None,
        };
        assert_eq!(ApiClient::from_lookup(bearer).unwrap().unwrap().base_url(), "http://127.0.0.1:3000");
    }

    struct FakeApi {
        calls: Mutex<Vec<Value>>,
    }
    impl FactoryApi for FakeApi {
        fn upsert_bot(&self, body: &Value) -> std::result::Result<Value, ApiError> {
            self.calls.lock().unwrap().push(body.clone());
            let slug = body["slug"].as_str().unwrap();
            Ok(json!({ "bot": { "id": format!("id-{slug}"), "slug": slug }, "created": true }))
        }
        fn list_bots(&self) -> std::result::Result<Vec<Value>, ApiError> {
            Ok(vec![])
        }
        fn create_node_ticket(&self, _: &Value) -> std::result::Result<Value, ApiError> {
            Err(ApiError::new("refused", Some(409), "not a vendor bot"))
        }
    }

    #[test]
    fn bind_steps_register_and_fail_without_api() {
        let t = parse_team("product-build", GOOD).unwrap();
        let plan = plan_up(&t, None, None, &LiveState::default()).unwrap();
        let binds: Vec<TeamPlanStep> = plan.iter().filter(|s| s.action == PlanAction::Bind).cloned().collect();
        let root = tempfile::tempdir().unwrap();

        let no_api = apply(root.path(), &t, None, &binds, &ApplyOptions::default());
        assert!(no_api.iter().all(|r| r.outcome == StepOutcome::Failed && r.code.as_deref() == Some("transport")));
        assert!(no_api[0].fact.contains(API_NOT_SET_FACT));

        let api = Arc::new(FakeApi { calls: Mutex::new(vec![]) });
        let opts = ApplyOptions { api: Some(api.clone()), ..Default::default() };
        let res = apply(root.path(), &t, None, &binds, &opts);
        assert!(res.iter().all(|r| r.outcome == StepOutcome::Ok), "{res:?}");
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls[0]["binding"]["type"], "hosted");
        // research is directed by al, bound earlier in the same run.
        assert_eq!(calls[1]["binding"]["type"], "vendor");
        assert_eq!(calls[1]["binding"]["directingBotId"], "id-al");
        assert_eq!(worst_code(&res), None);
        assert_eq!(worst_code(&no_api).as_deref(), Some("transport"));
    }

    #[test]
    fn spawn_on_a_machine_is_refused_as_not_built() {
        let t = parse_team("product-build", GOOD).unwrap();
        let plan = plan_up(&t, None, Some("mac-mini"), &LiveState::default()).unwrap();
        let spawn: Vec<TeamPlanStep> = plan.iter().filter(|s| s.action == PlanAction::Spawn).cloned().collect();
        let root = tempfile::tempdir().unwrap();
        let res = apply(root.path(), &t, None, &spawn, &ApplyOptions::default());
        assert_eq!(res[0].code.as_deref(), Some("not_found"));
        assert!(res[0].fact.contains("not built yet"), "{}", res[0].fact);
        // Nothing was delivered.
        assert!(!root.path().join("CLAUDE.md").exists());
    }
}
