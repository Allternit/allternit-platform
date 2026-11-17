//! Per-app access tiers for computer use (Wave C, decisions 1-3; the
//! `access` block of `contracts/computer-toolset/allternit-computer-v2.json`).
//!
//! Three tiers: `read` (see the app), `click` (also pointer input) and `full`
//! (also keyboard and text entry). Every app category has a ceiling: browsers
//! top out at read, terminals and IDEs at click, everything else at full, and
//! an app that can't be identified at read. The rules:
//!
//! - Tiers apply to targets outside the sandbox (this device). Cloud/bot
//!   computers, gateway browsers, and an API that itself runs on a cloud
//!   computer are exempt and always run at full.
//! - `computer_safety::assess` checks every call against the app actually in
//!   front (the observed context), never the app the model named. A call
//!   above that app's granted tier is denied with `access_required`.
//! - An app with no grant is read only. An unknown app fails closed.
//! - Grants last for one session (the run id), are held in memory only and
//!   expire after 12 hours. Nothing is stored, so there is no migration.
//! - Existing `allow_apps` entries are standing grants, capped at the ceiling.
//! - Deny lists win over grants.
//!
//! The two members, `resolve_access` and `request_access`, run here, not in
//! the driver. `request_access` is `confirm: always`, so it goes through the
//! existing hash-bound approval path in `computer_toolset::execute`: the first
//! call answers `409 approval_required` with an `access` block (the apps and
//! tiers asked for) so the runtime can show the person an ask; the retry with
//! the approval grant records the grants. A tier above an app's ceiling is
//! refused with `tier_exceeded` before anyone is asked.
//!
//! This module also holds the Rust side of the shared action-class table
//! (`contracts/action-classes/vectors.json`): [`classify_action`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::computer_routes::ComputerResponse;
use crate::computer_safety::{Context, SafetySettings};
use crate::computer_toolset::{screen_info, text, Fail, Target, ToolsetResult};
use crate::AppState;

/// The two access members of allternit.computer.v2 (run in allternit-api).
pub const ACCESS_MEMBERS: [&str; 2] = ["resolve_access", "request_access"];

pub fn is_access_member(member: &str) -> bool {
    ACCESS_MEMBERS.contains(&member)
}

// ---------------------------------------------------------------------------
// Tiers and categories.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Read,
    Click,
    Full,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::Click => "click",
            Tier::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "read" => Some(Tier::Read),
            "click" => Some(Tier::Click),
            "full" => Some(Tier::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Browser,
    Terminal,
    Ide,
    Other,
    Unknown,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Browser => "browser",
            Category::Terminal => "terminal",
            Category::Ide => "ide",
            Category::Other => "other",
            Category::Unknown => "unknown",
        }
    }

    /// The highest tier an app of this category can be granted.
    pub fn ceiling(self) -> Tier {
        match self {
            Category::Browser | Category::Unknown => Tier::Read,
            Category::Terminal | Category::Ide => Tier::Click,
            Category::Other => Tier::Full,
        }
    }
}

/// Apps resolve_access knows by name and bundle id. Anything else is
/// categorized from its identifiers by [`categorize`].
const KNOWN_APPS: &[(&str, &str)] = &[
    ("com.apple.Safari", "Safari"),
    ("com.google.Chrome", "Google Chrome"),
    ("org.mozilla.firefox", "Firefox"),
    ("com.microsoft.edgemac", "Microsoft Edge"),
    ("company.thebrowser.Browser", "Arc"),
    ("company.thebrowser.dia", "Dia"),
    ("com.brave.Browser", "Brave Browser"),
    ("com.operasoftware.Opera", "Opera"),
    ("com.vivaldi.Vivaldi", "Vivaldi"),
    ("com.openai.atlas", "ChatGPT Atlas"),
    ("ai.perplexity.comet", "Comet"),
    ("com.apple.Terminal", "Terminal"),
    ("com.googlecode.iterm2", "iTerm2"),
    ("dev.warp.Warp-Stable", "Warp"),
    ("com.mitchellh.ghostty", "Ghostty"),
    ("net.kovidgoyal.kitty", "kitty"),
    ("org.alacritty", "Alacritty"),
    ("com.github.wez.wezterm", "WezTerm"),
    ("co.zeit.hyper", "Hyper"),
    ("com.microsoft.VSCode", "Visual Studio Code"),
    ("com.todesktop.230313mzl4w4u92", "Cursor"),
    ("com.exafunction.windsurf", "Windsurf"),
    ("dev.zed.Zed", "Zed"),
    ("com.apple.dt.Xcode", "Xcode"),
    ("com.jetbrains.intellij", "IntelliJ IDEA"),
    ("com.jetbrains.pycharm", "PyCharm"),
    ("com.google.android.studio", "Android Studio"),
    ("com.sublimetext.4", "Sublime Text"),
    ("com.apple.finder", "Finder"),
    ("com.apple.mail", "Mail"),
    ("com.apple.MobileSMS", "Messages"),
    ("com.apple.Notes", "Notes"),
    ("com.apple.iCal", "Calendar"),
    ("com.apple.reminders", "Reminders"),
    ("com.apple.Preview", "Preview"),
    ("com.apple.TextEdit", "TextEdit"),
    ("com.apple.systempreferences", "System Settings"),
    ("com.apple.iWork.Pages", "Pages"),
    ("com.apple.iWork.Numbers", "Numbers"),
    ("com.apple.iWork.Keynote", "Keynote"),
    ("com.apple.Music", "Music"),
    ("com.apple.Photos", "Photos"),
    ("com.microsoft.Word", "Microsoft Word"),
    ("com.microsoft.Excel", "Microsoft Excel"),
    ("com.microsoft.Powerpoint", "Microsoft PowerPoint"),
    ("com.microsoft.Outlook", "Microsoft Outlook"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.figma.Desktop", "Figma"),
    ("notion.id", "Notion"),
    ("com.spotify.client", "Spotify"),
    ("us.zoom.xos", "zoom.us"),
];

const BROWSER_WORDS: &[&str] = &[
    "safari", "chrome", "chromium", "firefox", "edge", "edgemac", "brave", "opera", "vivaldi", "arc", "browser", "orion", "comet",
    "dia", "zen", "atlas", "duckduckgo", "librewolf", "waterfox", "tor",
];
const TERMINAL_WORDS: &[&str] = &[
    "terminal", "iterm", "iterm2", "warp", "ghostty", "kitty", "alacritty", "wezterm", "hyper", "termius", "tabby", "konsole",
    "xterm", "powershell", "pwsh", "cmd", "conhost", "windowsterminal", "tilix", "terminator", "rio",
];
const IDE_WORDS: &[&str] = &[
    "vscode", "vscodium", "cursor", "windsurf", "zed", "xcode", "intellij", "pycharm", "webstorm", "goland", "rider", "clion",
    "rubymine", "phpstorm", "datagrip", "fleet", "sublime", "sublimetext", "nova", "bbedit", "macvim", "emacs", "kiro", "trae",
    "antigravity", "jetbrains", "eclipse", "netbeans",
];
/// Multi-word IDE names ("Visual Studio Code", "Android Studio").
const IDE_PHRASES: &[&str] = &["visual studio", "android studio"];

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// The category of an app from its name and/or bundle id. No identifier at
/// all is `unknown`; an identified app that is not a browser, terminal or
/// IDE is `other`. When in doubt the stricter category wins (browser before
/// terminal before IDE before other).
pub fn categorize(app: Option<&str>, bundle_id: Option<&str>) -> Category {
    let app = app.map(str::trim).filter(|s| !s.is_empty());
    let bundle = bundle_id.map(str::trim).filter(|s| !s.is_empty());
    if app.is_none() && bundle.is_none() {
        return Category::Unknown;
    }
    let mut toks: Vec<String> = Vec::new();
    let mut joined = String::new();
    for s in [app, bundle].into_iter().flatten() {
        toks.extend(words(s));
        joined.push_str(&s.to_lowercase());
        joined.push(' ');
    }
    let has = |list: &[&str]| toks.iter().any(|t| list.contains(&t.as_str()));
    if has(BROWSER_WORDS) {
        return Category::Browser;
    }
    if has(TERMINAL_WORDS) {
        return Category::Terminal;
    }
    if has(IDE_WORDS) || IDE_PHRASES.iter().any(|p| joined.contains(p)) || joined.contains("com.jetbrains.") {
        return Category::Ide;
    }
    Category::Other
}

/// The tier a computer-toolset call needs, or `None` for members that don't
/// act in an app (request_human, the access members, subtask loops whose
/// steps are each checked on their own). Unknown members and ops fail closed
/// at full.
pub fn required_tier(member: &str, input: &Value) -> Option<Tier> {
    match member {
        "screenshot" | "zoom" | "read_ui" | "verify" | "cursor_position" | "wait" => Some(Tier::Read),
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" | "left_click_drag" | "left_mouse_down"
        | "left_mouse_up" | "scroll" | "mouse_move" => Some(Tier::Click),
        "type" | "key" | "hold_key" | "use_credential" => Some(Tier::Full),
        "act" => Some(act_tier(input)),
        "run_batch" => Some(
            input
                .get("steps")
                .and_then(Value::as_array)
                .map(|steps| steps.iter().map(batch_step_tier).max().unwrap_or(Tier::Read))
                .unwrap_or(Tier::Full),
        ),
        "request_human" | "run_subtask" | "run_parallel" | "run_skill" | "skills" | "resolve_access" | "request_access" => None,
        _ => Some(Tier::Full),
    }
}

fn act_tier(a: &Value) -> Tier {
    match a.get("op").and_then(Value::as_str) {
        Some("click" | "menu" | "focus") => Tier::Click,
        _ => Tier::Full,
    }
}

fn batch_step_tier(step: &Value) -> Tier {
    let mut t = Tier::Read;
    if let Some(a) = step.get("act") {
        t = t.max(act_tier(a));
    }
    if step.get("menu").is_some() {
        t = t.max(Tier::Click);
    }
    if let Some(p) = step.get("pixel") {
        let tool = p.get("tool").and_then(Value::as_str).unwrap_or("");
        t = t.max(required_tier(tool, p.get("args").unwrap_or(&Value::Null)).unwrap_or(Tier::Full));
    }
    let known = ["act", "menu", "pixel", "wait", "wait_for", "expect", "timeout_ms"];
    if step.as_object().is_some_and(|o| o.keys().any(|k| !known.contains(&k.as_str()))) {
        t = Tier::Full;
    }
    t
}

static ON_CLOUD: Lazy<bool> = Lazy::new(crate::cloud_computer_peer::running_on_cloud_computer);

/// Targets the tiers don't apply to: sandboxed targets (cloud/bot guests,
/// gateway browsers) and an API that itself runs on a cloud computer.
pub fn exempt(target: &Target) -> bool {
    target.sandboxed() || *ON_CLOUD
}

// ---------------------------------------------------------------------------
// Session grants (memory only, 12 h).
// ---------------------------------------------------------------------------

pub const GRANT_TTL_SECS: u64 = 12 * 3600;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Grant {
    /// The run (session) the grant belongs to; "" for calls without one.
    pub session: String,
    pub bundle_id: String,
    pub name: String,
    pub tier: Tier,
    pub granted_at: u64,
    pub expires_at: u64,
}

type GrantKey = (String, String);
static GRANTS: Lazy<Mutex<HashMap<GrantKey, Vec<Grant>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The session a call's grants live under. Calls without a run id (and the
/// per-call ids `execute` makes up for them, `toolset-…`) share one
/// session-less slot per owner and computer.
pub fn session_key(run_id: Option<&str>) -> String {
    match run_id.map(str::trim) {
        Some(r) if !r.is_empty() && !r.starts_with("toolset-") => r.to_string(),
        _ => String::new(),
    }
}

/// Record (or replace) a grant for one app in one session.
pub fn record_grant(owner: &str, computer_id: &str, session: &str, bundle_id: &str, name: &str, tier: Tier, now: u64) -> Grant {
    let g = Grant {
        session: session.to_string(),
        bundle_id: bundle_id.to_string(),
        name: name.to_string(),
        tier,
        granted_at: now,
        expires_at: now + GRANT_TTL_SECS,
    };
    let mut all = GRANTS.lock().unwrap_or_else(|p| p.into_inner());
    let list = all.entry((owner.to_string(), computer_id.to_string())).or_default();
    list.retain(|x| x.expires_at > now && !(x.session == session && x.bundle_id.eq_ignore_ascii_case(bundle_id)));
    list.push(g.clone());
    g
}

/// Live grants on one computer, every session (expired ones are dropped).
pub fn list_grants(owner: &str, computer_id: &str, now: u64) -> Vec<Grant> {
    let mut all = GRANTS.lock().unwrap_or_else(|p| p.into_inner());
    let key = (owner.to_string(), computer_id.to_string());
    let Some(list) = all.get_mut(&key) else { return Vec::new() };
    list.retain(|g| g.expires_at > now);
    let out = list.clone();
    if list.is_empty() {
        all.remove(&key);
    }
    out
}

/// Live grants of one session on one computer.
pub fn session_grants(owner: &str, computer_id: &str, session: &str, now: u64) -> Vec<Grant> {
    list_grants(owner, computer_id, now).into_iter().filter(|g| g.session == session).collect()
}

/// Revoke grants: all of them, or those matching an app and/or a session.
pub fn revoke(owner: &str, computer_id: &str, bundle_id: Option<&str>, session: Option<&str>) -> usize {
    let mut all = GRANTS.lock().unwrap_or_else(|p| p.into_inner());
    let Some(list) = all.get_mut(&(owner.to_string(), computer_id.to_string())) else { return 0 };
    let before = list.len();
    list.retain(|g| {
        let app_hit = bundle_id.map_or(true, |b| g.bundle_id.eq_ignore_ascii_case(b));
        let session_hit = session.map_or(true, |s| g.session == s);
        !(app_hit && session_hit)
    });
    before - list.len()
}

/// Does a grant cover the app in front? By bundle id when the context has
/// one, else by name.
fn grant_matches(g: &Grant, ctx: &Context) -> bool {
    match ctx.bundle_id.as_deref().filter(|b| !b.is_empty()) {
        Some(b) => g.bundle_id.eq_ignore_ascii_case(b),
        None => ctx.app.as_deref().is_some_and(|a| g.name.eq_ignore_ascii_case(a) || g.bundle_id.eq_ignore_ascii_case(a)),
    }
}

// ---------------------------------------------------------------------------
// The tier check (called from computer_safety::assess).
// ---------------------------------------------------------------------------

/// What the tier check saw, for the audit row, the live view and the
/// `access_required` reply.
#[derive(Debug, Clone, Serialize)]
pub struct AccessCheck {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    pub category: Category,
    pub max_tier: Tier,
    pub granted: Tier,
    pub required: Tier,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exempt: bool,
}

/// The tier an app in front holds: the session grant or the standing
/// `allow_apps` grant, never below read, capped at the category ceiling, and
/// nothing above read when a deny list names the app.
pub fn granted_tier(ctx: &Context, settings: &SafetySettings, grants: &[Grant]) -> (Category, Tier) {
    let category = if ctx.known { categorize(ctx.app.as_deref(), ctx.bundle_id.as_deref()) } else { Category::Unknown };
    let ceiling = category.ceiling();
    if !ctx.known {
        return (category, Tier::Read);
    }
    let denied = settings.deny_apps.iter().flatten().any(|p| crate::computer_safety::app_matches(p, ctx));
    if denied {
        return (category, Tier::Read);
    }
    let standing = settings.allow_apps.iter().flatten().any(|p| crate::computer_safety::app_matches(p, ctx)).then_some(ceiling);
    let session = grants.iter().filter(|g| grant_matches(g, ctx)).map(|g| g.tier).max();
    let best = [Some(Tier::Read), standing, session].into_iter().flatten().max().unwrap_or(Tier::Read);
    (category, best.min(ceiling))
}

/// Check one call's tier against the app in front. `Err` carries the reason
/// the call is refused with `access_required`.
pub fn decide(ctx: &Context, settings: &SafetySettings, grants: &[Grant], required: Tier, exempt: bool) -> (AccessCheck, Result<(), String>) {
    if exempt {
        let category = if ctx.known { categorize(ctx.app.as_deref(), ctx.bundle_id.as_deref()) } else { Category::Unknown };
        let check = AccessCheck {
            app: ctx.app.clone(),
            bundle_id: ctx.bundle_id.clone(),
            category,
            max_tier: Tier::Full,
            granted: Tier::Full,
            required,
            exempt: true,
        };
        return (check, Ok(()));
    }
    let (category, granted) = granted_tier(ctx, settings, grants);
    let ceiling = category.ceiling();
    let check = AccessCheck {
        app: ctx.app.clone(),
        bundle_id: ctx.bundle_id.clone(),
        category,
        max_tier: ceiling,
        granted,
        required,
        exempt: false,
    };
    if required <= granted {
        return (check, Ok(()));
    }
    let app = ctx.describe();
    let why = if !ctx.known {
        format!(
            "The app in front couldn't be identified, so only reading is allowed, and this step needs {} access. It did not run.",
            required.as_str()
        )
    } else if required > ceiling {
        format!(
            "{app} is a {} app; agents can use it at most at {} access, and this step needs {}. It did not run. Read it instead, or ask the person to do this step.",
            category.as_str(),
            ceiling.as_str(),
            required.as_str()
        )
    } else {
        format!(
            "{app} has {} access in this session and this step needs {}. It did not run. Call resolve_access for this app, then request_access for {}.",
            granted.as_str(),
            required.as_str(),
            required.as_str()
        )
    };
    (check, Err(why))
}

/// The `access` block of an `access_required` reply: the app in front as a
/// resolve_access entry and the tier the step needed, so the runtime can ask.
pub fn access_required_payload(check: &AccessCheck) -> Value {
    let bundle = check.bundle_id.clone().or_else(|| check.app.clone()).unwrap_or_default();
    let name = check.app.clone().unwrap_or_else(|| bundle.clone());
    json!({
        "needed_tier": check.required,
        "granted_tier": check.granted,
        "entries": [{ "name": name, "bundle_id": bundle, "category": check.category, "max_tier": check.max_tier }],
    })
}

// ---------------------------------------------------------------------------
// resolve_access / request_access.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub bundle_id: String,
    pub category: Category,
    pub max_tier: Tier,
}

fn looks_like_bundle_id(s: &str) -> bool {
    s.contains('.') && !s.contains(char::is_whitespace) && !s.starts_with('.') && !s.ends_with('.')
}

fn known_app(s: &str) -> Option<(&'static str, &'static str)> {
    KNOWN_APPS.iter().copied().find(|(b, n)| b.eq_ignore_ascii_case(s) || n.eq_ignore_ascii_case(s))
}

/// Resolve one app name or bundle id without touching the system.
pub fn resolve_static(s: &str, exempt: bool) -> Option<Entry> {
    let s = s.trim();
    let (name, bundle) = match known_app(s) {
        Some((b, n)) => (n.to_string(), b.to_string()),
        None if looks_like_bundle_id(s) => (s.to_string(), s.to_string()),
        None => return None,
    };
    let category = categorize(Some(name.as_str()), Some(bundle.as_str()));
    Some(Entry { name, bundle_id: bundle, category, max_tier: if exempt { Tier::Full } else { category.ceiling() } })
}

/// An entry for an app that couldn't be identified: unknown, read only (full
/// on exempt targets).
fn unknown_entry(s: &str, exempt: bool) -> Entry {
    let s = s.trim().to_string();
    Entry {
        name: s.clone(),
        bundle_id: s,
        category: Category::Unknown,
        max_tier: if exempt { Tier::Full } else { Tier::Read },
    }
}

/// Spotlight lookup of an installed app by display name (macOS, this device
/// only): its display name and bundle id.
async fn spotlight_app(name: &str) -> Option<(String, String)> {
    if !cfg!(target_os = "macos") || name.is_empty() || name.len() > 100 || name.contains(['\'', '"', '\\', '*']) {
        return None;
    }
    let query = format!("kMDItemContentType == 'com.apple.application-bundle' && kMDItemDisplayName == '{name}'cd");
    let out = tokio::time::timeout(Duration::from_secs(3), tokio::process::Command::new("mdfind").arg(&query).output()).await.ok()?.ok()?;
    let path = String::from_utf8_lossy(&out.stdout).lines().map(str::trim).find(|l| l.ends_with(".app"))?.to_string();
    let out = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("mdls").args(["-raw", "-name", "kMDItemCFBundleIdentifier", &path]).output(),
    )
    .await
    .ok()?
    .ok()?;
    let bundle = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if bundle.is_empty() || bundle == "(null)" {
        return None;
    }
    let display = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.to_string());
    Some((display, bundle))
}

/// resolve_access: look up each app's name, bundle id, category and ceiling.
pub async fn resolve_apps(apps: &[String], target: &Target) -> Vec<Entry> {
    let exempt = exempt(target);
    let mut out = Vec::with_capacity(apps.len());
    for a in apps.iter().take(8) {
        if let Some(e) = resolve_static(a, exempt) {
            out.push(e);
            continue;
        }
        let found = if matches!(target, Target::ThisDevice) { spotlight_app(a.trim()).await } else { None };
        match found {
            Some((name, bundle)) => {
                let category = categorize(Some(name.as_str()), Some(bundle.as_str()));
                out.push(Entry { name, bundle_id: bundle, category, max_tier: if exempt { Tier::Full } else { category.ceiling() } });
            }
            None if exempt => {
                let category = categorize(Some(a.as_str()), None);
                out.push(Entry { name: a.trim().to_string(), bundle_id: a.trim().to_string(), category, max_tier: Tier::Full });
            }
            None => out.push(unknown_entry(a, exempt)),
        }
    }
    out
}

/// One requested entry, checked: the resolved app and the tier asked for.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Requested {
    #[serde(flatten)]
    pub entry: Entry,
    pub tier: Tier,
}

/// Parse and check request_access entries against the category ceilings and
/// the deny lists. `Err((code, reason))`: `tier_exceeded` for a tier above an
/// app's ceiling, `safety_denied` for an app a deny list names, and
/// `invalid_input` for a malformed entry.
pub fn check_request(input: &Value, settings: &SafetySettings, exempt: bool) -> Result<Vec<Requested>, (&'static str, String)> {
    let entries = input.get("entries").and_then(Value::as_array).ok_or(("invalid_input", "entries is required".to_string()))?;
    if entries.is_empty() || entries.len() > 8 {
        return Err(("invalid_input", "entries takes 1 to 8 apps".into()));
    }
    let mut out = Vec::new();
    for e in entries {
        let bundle = e.get("bundle_id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let tier = e.get("tier").and_then(Value::as_str).and_then(Tier::parse);
        let (Some(bundle), Some(tier)) = (bundle, tier) else {
            return Err(("invalid_input", "each entry needs a bundle_id and a tier (read, click or full)".into()));
        };
        let entry = resolve_static(bundle, exempt).unwrap_or_else(|| unknown_entry(bundle, exempt));
        let ctx = Context { app: Some(entry.name.clone()), bundle_id: Some(entry.bundle_id.clone()), known: true, ..Default::default() };
        if let Some(p) = settings.deny_apps.iter().flatten().find(|p| crate::computer_safety::app_matches(p, &ctx)) {
            return Err(("safety_denied", format!("{} is on this computer's deny list ({p}), so it can't be granted.", entry.name)));
        }
        if tier > entry.max_tier {
            return Err((
                "tier_exceeded",
                format!(
                    "{} is a {} app; it can be granted at most {} access, not {}. Ask for {} or less.",
                    entry.name,
                    entry.category.as_str(),
                    entry.max_tier.as_str(),
                    tier.as_str(),
                    entry.max_tier.as_str()
                ),
            ));
        }
        out.push(Requested { entry, tier });
    }
    Ok(out)
}

/// The `access` block on a request_access `approval_required` reply: what
/// the person is asked to grant.
pub fn request_payload(input: &Value, exempt: bool) -> Value {
    let entries: Vec<Value> = input
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let bundle = e.get("bundle_id").and_then(Value::as_str)?;
            let entry = resolve_static(bundle, exempt).unwrap_or_else(|| unknown_entry(bundle, exempt));
            Some(json!({
                "name": entry.name,
                "bundle_id": entry.bundle_id,
                "category": entry.category,
                "max_tier": entry.max_tier,
                "tier": e.get("tier"),
            }))
        })
        .collect();
    json!({
        "entries": entries,
        "reason": input.get("reason").and_then(Value::as_str).unwrap_or_default(),
        "expires_in_secs": GRANT_TTL_SECS,
    })
}

/// request_access on an exempt target asks nothing: there is nothing to grant.
pub fn approval_exempt(member: &str, sandboxed_or_exempt: bool) -> bool {
    member == "request_access" && sandboxed_or_exempt
}

fn result_json(v: Value) -> ToolsetResult {
    ToolsetResult {
        is_error: false,
        content: vec![text(serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()))],
        browser_state: None,
        screen: screen_info(None),
        error: None,
    }
}

/// Run an access member (from `computer_v2::execute_v2`, after the lease,
/// policy, safety, approval and audit steps of `execute`).
pub async fn execute(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    member: &str,
    input: &Value,
    run_id: &str,
) -> Result<ToolsetResult, Fail> {
    match member {
        "resolve_access" => {
            let apps: Vec<String> =
                input.get("apps").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            if apps.is_empty() {
                return Err(Fail::coded("invalid_input", "apps takes 1 to 8 app names or bundle ids"));
            }
            let entries = resolve_apps(&apps, target).await;
            Ok(result_json(json!({ "entries": entries })))
        }
        "request_access" => {
            let ex = exempt(target);
            // Checked again after the approval: the settings may have changed
            // while the person was deciding.
            let settings = crate::computer_safety::load_settings(state, &user.user_id, &computer.id, None).await;
            let wanted = check_request(input, &settings, ex).map_err(|(code, why)| Fail::coded(code, why))?;
            if ex {
                let granted: Vec<Value> = wanted.iter().map(|r| json!({ "bundle_id": r.entry.bundle_id, "tier": "full" })).collect();
                return Ok(result_json(json!({ "granted": granted, "note": "This computer is a sandbox: every app already runs at full access." })));
            }
            let session = session_key(Some(run_id));
            let now = now_secs();
            let granted: Vec<Value> = wanted
                .iter()
                .map(|r| {
                    let g = record_grant(&user.user_id, &computer.id, &session, &r.entry.bundle_id, &r.entry.name, r.tier, now);
                    json!({ "bundle_id": g.bundle_id, "tier": g.tier })
                })
                .collect();
            crate::computer_safety::emit(
                &computer.id,
                "access_granted",
                None,
                member,
                Some(run_id),
                json!({ "granted": granted, "session": session, "expires_at": now + GRANT_TTL_SECS }),
            );
            Ok(result_json(json!({ "granted": granted })))
        }
        other => Err(Fail::from(format!("{other} isn't an access member"))),
    }
}

// ---------------------------------------------------------------------------
// Routes: GET/DELETE /computers/:id/access.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct RevokeQuery {
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Whether a computer's calls are exempt from tiers (same rule as
/// `exempt(target)`, from the computer record).
fn computer_exempt(computer: &ComputerResponse) -> bool {
    !crate::computer_routes::is_this_device(computer) || *ON_CLOUD
}

/// The live session grants on a computer, the standing grants from
/// `allow_apps`, and the category ceilings.
pub async fn get_access(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let computer = match crate::computer_toolset::resolve_computer(&state, &user, &id, &headers).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    let settings = crate::computer_safety::load_settings(&state, &user.user_id, &computer.id, None).await;
    let grants = list_grants(&user.user_id, &computer.id, now_secs());
    let standing: Vec<Value> = settings
        .allow_apps
        .iter()
        .flatten()
        .map(|p| {
            let entry = resolve_static(p, false);
            let category = entry.as_ref().map_or_else(|| categorize(Some(p.as_str()), None), |e| e.category);
            json!({ "pattern": p, "category": category, "tier": category.ceiling() })
        })
        .collect();
    Json(json!({
        "computer_id": computer.id,
        "exempt": computer_exempt(&computer),
        "grants": grants,
        "standing": standing,
        "deny_apps": settings.deny_apps.unwrap_or_default(),
        "ceilings": {
            "browser": Category::Browser.ceiling(),
            "terminal": Category::Terminal.ceiling(),
            "ide": Category::Ide.ceiling(),
            "other": Category::Other.ceiling(),
            "unknown": Category::Unknown.ceiling(),
        },
        "ttl_secs": GRANT_TTL_SECS,
    }))
    .into_response()
}

/// Revoke session grants on a computer: all, or by `bundle_id` and/or
/// `session`.
pub async fn delete_access(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<RevokeQuery>,
) -> Response {
    let computer = match crate::computer_toolset::resolve_computer(&state, &user, &id, &headers).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    let bundle = q.bundle_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let n = revoke(&user.user_id, &computer.id, bundle, q.session.as_deref());
    crate::computer_safety::emit(&computer.id, "access_revoked", None, "request_access", None, json!({ "bundle_id": bundle, "session": q.session, "revoked": n }));
    Json(json!({ "computer_id": computer.id, "revoked": n })).into_response()
}

// ---------------------------------------------------------------------------
// Action classes (the Rust side of contracts/action-classes/vectors.json).
// ---------------------------------------------------------------------------

/// The four action classes, weakest first: the stricter class wins, so
/// [`tighten`] is `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    Free,
    ConfirmUnlessNamed,
    ConfirmNow,
    HandOff,
}

impl ActionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionClass::Free => "free",
            ActionClass::ConfirmUnlessNamed => "confirm_unless_named",
            ActionClass::ConfirmNow => "confirm_now",
            ActionClass::HandOff => "hand_off",
        }
    }
}

/// Join two verdicts: can only make the class stricter.
pub fn tighten(a: ActionClass, b: ActionClass) -> ActionClass {
    a.max(b)
}

fn class_of(i: crate::computer_safety::Irreversible) -> ActionClass {
    use crate::computer_safety::Irreversible as I;
    match i {
        I::Send | I::Publish => ActionClass::ConfirmUnlessNamed,
        I::Pay | I::Purchase | I::Transfer | I::Delete | I::Credentials | I::Account => ActionClass::ConfirmNow,
    }
}

/// Label words the irreversible classifier doesn't carry: uploads, API keys.
const EXTRA_LABELS: &[(&str, ActionClass)] = &[
    ("upload", ActionClass::ConfirmUnlessNamed),
    ("secret key", ActionClass::ConfirmNow),
    ("api key", ActionClass::ConfirmNow),
    ("access token", ActionClass::ConfirmNow),
    ("show password", ActionClass::ConfirmNow),
];

fn has_phrase(toks: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split(' ').collect();
    p.len() <= toks.len() && toks.windows(p.len()).any(|w| w.iter().zip(&p).all(|(a, b)| a == b))
}

/// A human check (CAPTCHA): the person's to do.
fn captcha_label(toks: &[String]) -> bool {
    toks.iter().any(|t| matches!(t.as_str(), "captcha" | "recaptcha" | "hcaptcha"))
        || has_phrase(toks, "not a robot")
        || has_phrase(toks, "verify you are human")
        || has_phrase(toks, "i am human")
}

/// Signing a document: "Sign", "Adopt and Sign", "Sign contract" — not "Sign
/// in", "Sign up" or "Sign out".
fn signing_label(toks: &[String]) -> bool {
    toks.iter().enumerate().any(|(i, t)| {
        t == "sign" && !toks.get(i + 1).is_some_and(|n| matches!(n.as_str(), "in" | "up" | "out" | "on" | "off" | "into"))
    })
}

/// The class of a control's label (role aware: text fields and other passive
/// roles are never classified by their label, except a CAPTCHA).
pub fn classify_control(role: &str, label: &str) -> ActionClass {
    let toks = words(label);
    if captcha_label(&toks) {
        return ActionClass::HandOff;
    }
    if crate::computer_safety::passive_role(role) {
        return ActionClass::Free;
    }
    let mut c = crate::computer_safety::classify_label(label).map_or(ActionClass::Free, class_of);
    if signing_label(&toks) {
        c = tighten(c, ActionClass::ConfirmNow);
    }
    for (p, class) in EXTRA_LABELS {
        if has_phrase(&toks, p) {
            c = tighten(c, *class);
        }
    }
    c
}

static FORK_BOMB: Lazy<Regex> = Lazy::new(|| Regex::new(r"([A-Za-z_:][A-Za-z0-9_:]*)\(\)\{([A-Za-z_:][A-Za-z0-9_:]*)\|").unwrap());
static DD_DEVICE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bdd\b[^;&|]*\bof=/dev/").unwrap());
static DISK_FORMAT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(diskutil\s+(erase\w*|zerodisk|secureerase|partitiondisk|reformat)|mkfs(\.\w+)?|format\s+[a-z]:|wipefs)\b").unwrap());
static STRIPE_MONEY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bstripe\s+(payouts|charges|transfers|refunds|payment_intents|invoices|subscriptions|topups)\s+(create|send|pay|finalize|capture|confirm)\b").unwrap()
});
static CRED_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(\.aws/credentials|\.ssh/id_|\.netrc|\.npmrc|\.pypirc|\.docker/config\.json|\.kube/config|(^|[\s/])\.env(\.\w+)?(\s|$)|security\s+find-(generic|internet)-password|\bgh\s+auth\s+token\b)").unwrap()
});
static SECRET_ASSIGN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b[A-Z0-9_]*(SECRET|API_?KEY|TOKEN|PASSWORD|PASSWD|PRIVATE_KEY)[A-Z0-9_]*\s*=").unwrap());
static SECRET_VALUE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(sk_live_|sk_test_|rk_live_|\bsk-[A-Za-z0-9<]|\bAKIA[0-9A-Z]{4}|\bghp_|\bxox[abp]-)").unwrap());
static PUBLISH_CMD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(git\s+push|(npm|pnpm|yarn|bun)\s+publish|cargo\s+publish|twine\s+upload|gh\s+pr\s+merge|gh\s+release\s+create|docker\s+push|wrangler\s+(pages\s+)?(deploy|publish)|vercel(\s+deploy|\s+--prod)|netlify\s+deploy|firebase\s+deploy|kubectl\s+apply|terraform\s+apply)\b").unwrap()
});

/// The class of a shell command. Each `;`/`&&`/`||`/`|` segment is checked
/// and the strictest one wins.
pub fn classify_command(command: &str) -> ActionClass {
    let squeezed: String = command.chars().filter(|c| !c.is_whitespace()).collect();
    if squeezed.contains(":(){:|:&};:") || FORK_BOMB.captures(&squeezed).is_some_and(|c| c[1] == c[2]) {
        return ActionClass::HandOff;
    }
    if DD_DEVICE.is_match(command) || DISK_FORMAT.is_match(command) {
        return ActionClass::HandOff;
    }
    let mut c = ActionClass::Free;
    for seg in command.split(['\n', ';', '|', '&']) {
        c = tighten(c, classify_rm(seg));
    }
    if c == ActionClass::HandOff {
        return c;
    }
    if STRIPE_MONEY.is_match(command) || CRED_READ.is_match(command) || SECRET_ASSIGN.is_match(command) || SECRET_VALUE.is_match(command) {
        c = tighten(c, ActionClass::ConfirmNow);
    }
    if PUBLISH_CMD.is_match(command) {
        c = tighten(c, ActionClass::ConfirmUnlessNamed);
    }
    c
}

/// rm/shred: wiping `/` or the home folder is hand_off; deleting an absolute
/// or home path (outside the project) is confirm_now; a relative path inside
/// the project stays free here (the project rules apply).
fn classify_rm(segment: &str) -> ActionClass {
    let mut toks: Vec<&str> = segment.split_whitespace().collect();
    while toks.first().is_some_and(|t| matches!(*t, "sudo" | "doas" | "command" | "exec" | "nohup") || t.contains('=')) {
        toks.remove(0);
    }
    let Some(cmd) = toks.first().map(|t| t.rsplit('/').next().unwrap_or(*t)) else { return ActionClass::Free };
    if matches!(cmd, "shred" | "srm") {
        return ActionClass::ConfirmNow;
    }
    if cmd != "rm" {
        return ActionClass::Free;
    }
    let (flags, paths): (Vec<&str>, Vec<&str>) = toks[1..].iter().copied().partition(|t| t.starts_with('-'));
    let recursive = flags.iter().any(|f| *f == "--recursive" || (!f.starts_with("--") && f.contains(['r', 'R'])));
    let root_like = |p: &str| {
        let p = p.trim_matches(['"', '\'']);
        matches!(p, "/" | "/*" | "~" | "~/" | "~/*" | "$HOME" | "$HOME/" | "$HOME/*" | "${HOME}" | "/Users" | "/home" | "/System" | "/usr" | "/etc")
    };
    if recursive && paths.iter().any(|p| root_like(*p)) {
        return ActionClass::HandOff;
    }
    if paths.iter().any(|p| {
        let p = p.trim_matches(['"', '\'']);
        p.starts_with('/') || p.starts_with('~') || p.starts_with("$HOME") || p.starts_with("${HOME}") || p.starts_with("..")
    }) {
        return ActionClass::ConfirmNow;
    }
    ActionClass::Free
}

/// The class of an MCP tool, from the words of its name.
pub fn classify_mcp(name: &str) -> ActionClass {
    let toks = words(&name.replace('_', " "));
    let any = |list: &[&str]| toks.iter().any(|t| list.contains(&t.as_str()));
    let read_only = any(&["list", "get", "read", "search", "fetch", "find", "query", "show", "describe", "view", "lookup", "count", "check", "status"]);
    let money = any(&["invoice", "invoices", "payout", "payouts", "payment", "payments", "charge", "charges", "refund", "refunds", "transfer", "transfers", "pay", "subscription", "subscriptions", "checkout", "purchase", "buy", "order"]);
    let writes = any(&["send", "create", "pay", "finalize", "charge", "refund", "transfer", "cancel", "update", "capture", "confirm", "issue", "buy", "purchase", "place", "submit"]);
    let deletes = any(&["delete", "remove", "destroy", "purge", "erase", "wipe", "drop"]);
    let creds = any(&["password", "passwords", "secret", "secrets", "credential", "credentials", "apikey", "token", "tokens"]);
    let sends = any(&["send", "post", "publish", "reply", "forward", "upload", "deploy", "merge", "share", "tweet", "push", "invite"]);
    let mut c = ActionClass::Free;
    if (money && writes) || deletes || (creds && !read_only) {
        c = ActionClass::ConfirmNow;
    }
    if sends {
        c = tighten(c, ActionClass::ConfirmUnlessNamed);
    }
    if money && writes {
        c = tighten(c, ActionClass::ConfirmNow);
    }
    c
}

/// The class of one action, in the shape of a vectors.json row: `tool`
/// (computer, browser, pane_browser, bash, write, mcp), `action` (member, op
/// or verb) and an `args` summary.
pub fn classify_action(tool: &str, action: &str, args: &Value) -> ActionClass {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    let secure = args.get("secure").and_then(Value::as_bool).unwrap_or(false);
    match tool {
        "bash" => classify_command(s("command")),
        "mcp" => classify_mcp(s("name")),
        "write" => {
            let path = s("path");
            let base = path.rsplit('/').next().unwrap_or(path);
            let env_file = base == ".env" || base.starts_with(".env.") || base == "credentials" || base.ends_with(".pem") || base.ends_with(".key");
            if env_file || SECRET_VALUE.is_match(s("text")) || SECRET_ASSIGN.is_match(s("text")) {
                ActionClass::ConfirmNow
            } else {
                ActionClass::Free
            }
        }
        _ => {
            let op = s("op");
            let member = if action == "act" && !op.is_empty() { op } else { action };
            match member {
                "use_credential" => ActionClass::ConfirmNow,
                "file_upload" | "upload_image" => ActionClass::ConfirmUnlessNamed,
                "type" | "set_value" | "form_input" | "fill" | "select" if secure => ActionClass::ConfirmNow,
                "key" | "press" => {
                    let chord = if s("text").is_empty() { s("key") } else { s("text") };
                    let k = crate::computer_safety::classify_key(chord).map_or(ActionClass::Free, class_of);
                    if args.get("label").is_some() && matches!(chord.to_lowercase().as_str(), "return" | "enter" | "space") {
                        tighten(k, classify_control(s("role"), s("label")))
                    } else {
                        k
                    }
                }
                "menu" => {
                    let last = match args.get("path") {
                        Some(Value::Array(a)) => a.last().and_then(Value::as_str).unwrap_or("").to_string(),
                        Some(Value::String(p)) => p.rsplit('>').next().unwrap_or("").trim().to_string(),
                        _ => String::new(),
                    };
                    classify_control("menuitem", &last)
                }
                _ if args.get("label").is_some() => {
                    if matches!(member, "type" | "set_value" | "form_input" | "fill" | "select" | "scroll" | "hover" | "focus") {
                        // Typing into or scrolling a control doesn't press it.
                        let toks = words(s("label"));
                        if captcha_label(&toks) { ActionClass::HandOff } else { ActionClass::Free }
                    } else {
                        classify_control(s("role"), s("label"))
                    }
                }
                _ => ActionClass::Free,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(app: &str, bundle: &str) -> Context {
        Context { app: Some(app.into()), bundle_id: Some(bundle.into()), known: true, ..Default::default() }
    }

    fn grant(bundle: &str, tier: Tier) -> Grant {
        Grant { session: "s".into(), bundle_id: bundle.into(), name: bundle.into(), tier, granted_at: 0, expires_at: u64::MAX }
    }

    #[test]
    fn categories_and_ceilings() {
        assert_eq!(categorize(Some("Safari"), Some("com.apple.Safari")), Category::Browser);
        assert_eq!(categorize(Some("Google Chrome"), None), Category::Browser);
        assert_eq!(categorize(None, Some("company.thebrowser.Browser")), Category::Browser);
        assert_eq!(categorize(Some("Terminal"), Some("com.apple.Terminal")), Category::Terminal);
        assert_eq!(categorize(Some("iTerm2"), Some("com.googlecode.iterm2")), Category::Terminal);
        assert_eq!(categorize(Some("Visual Studio Code"), Some("com.microsoft.VSCode")), Category::Ide);
        assert_eq!(categorize(Some("Cursor"), Some("com.todesktop.230313mzl4w4u92")), Category::Ide);
        assert_eq!(categorize(Some("PyCharm"), Some("com.jetbrains.pycharm")), Category::Ide);
        assert_eq!(categorize(Some("Notes"), Some("com.apple.Notes")), Category::Other);
        assert_eq!(categorize(None, None), Category::Unknown);
        assert_eq!(categorize(Some("  "), Some("")), Category::Unknown);
        assert_eq!(Category::Browser.ceiling(), Tier::Read);
        assert_eq!(Category::Terminal.ceiling(), Tier::Click);
        assert_eq!(Category::Ide.ceiling(), Tier::Click);
        assert_eq!(Category::Other.ceiling(), Tier::Full);
        assert_eq!(Category::Unknown.ceiling(), Tier::Read);
    }

    #[test]
    fn every_contract_member_has_a_tier_rule() {
        let c = crate::computer_toolset::contract(crate::computer_toolset::Toolset::Computer);
        let expect: &[(&str, Option<Tier>)] = &[
            ("screenshot", Some(Tier::Read)),
            ("zoom", Some(Tier::Read)),
            ("left_click", Some(Tier::Click)),
            ("right_click", Some(Tier::Click)),
            ("middle_click", Some(Tier::Click)),
            ("double_click", Some(Tier::Click)),
            ("triple_click", Some(Tier::Click)),
            ("mouse_move", Some(Tier::Click)),
            ("left_click_drag", Some(Tier::Click)),
            ("left_mouse_down", Some(Tier::Click)),
            ("left_mouse_up", Some(Tier::Click)),
            ("scroll", Some(Tier::Click)),
            ("type", Some(Tier::Full)),
            ("key", Some(Tier::Full)),
            ("hold_key", Some(Tier::Full)),
            ("wait", Some(Tier::Read)),
            ("cursor_position", Some(Tier::Read)),
            ("read_ui", Some(Tier::Read)),
            ("act", Some(Tier::Full)),
            ("run_batch", Some(Tier::Full)),
            ("verify", Some(Tier::Read)),
            ("request_human", None),
            ("use_credential", Some(Tier::Full)),
            ("run_subtask", None),
            ("run_parallel", None),
            ("run_skill", None),
            ("skills", None),
            ("resolve_access", None),
            ("request_access", None),
        ];
        assert_eq!(c.members.len(), expect.len());
        for (m, t) in expect {
            assert!(c.member(m).is_some(), "{m} is in the contract");
            assert_eq!(required_tier(m, &json!({})), *t, "{m}");
        }
        assert_eq!(required_tier("act", &json!({"id": "1", "op": "click"})), Some(Tier::Click));
        assert_eq!(required_tier("act", &json!({"id": "1", "op": "menu"})), Some(Tier::Click));
        assert_eq!(required_tier("act", &json!({"id": "1", "op": "set_value"})), Some(Tier::Full));
        assert_eq!(required_tier("act", &json!({"id": "1", "op": "press"})), Some(Tier::Full));
        let batch = |steps: Value| required_tier("run_batch", &json!({ "steps": steps }));
        assert_eq!(batch(json!([{ "wait": 100 }, { "act": { "id": "1", "op": "click" } }])), Some(Tier::Click));
        assert_eq!(batch(json!([{ "act": { "id": "1", "op": "click" } }, { "act": { "id": "2", "op": "select" } }])), Some(Tier::Full));
        assert_eq!(batch(json!([{ "pixel": { "tool": "screenshot" } }])), Some(Tier::Read));
        assert_eq!(batch(json!([{ "pixel": { "tool": "type", "args": { "text": "x" } } }])), Some(Tier::Full));
        assert_eq!(required_tier("brand_new_member", &json!({})), Some(Tier::Full), "unknown members fail closed");
    }

    #[test]
    fn tier_by_member_matrix() {
        let settings = SafetySettings::default();
        let notes = ctx("Notes", "com.apple.Notes");
        let members: &[(&str, Value, Tier)] = &[
            ("screenshot", json!({}), Tier::Read),
            ("left_click", json!({}), Tier::Click),
            ("scroll", json!({}), Tier::Click),
            ("act", json!({"id": "1", "op": "click"}), Tier::Click),
            ("type", json!({"text": "hi"}), Tier::Full),
            ("key", json!({"text": "cmd+s"}), Tier::Full),
            ("act", json!({"id": "1", "op": "set_value", "value": "x"}), Tier::Full),
        ];
        for granted in [None, Some(Tier::Read), Some(Tier::Click), Some(Tier::Full)] {
            let grants: Vec<Grant> = granted.map(|t| grant("com.apple.Notes", t)).into_iter().collect();
            let held = granted.unwrap_or(Tier::Read);
            for (m, input, need) in members {
                let req = required_tier(m, input).unwrap();
                assert_eq!(req, *need, "{m}");
                let (check, out) = decide(&notes, &settings, &grants, req, false);
                assert_eq!(check.granted, held);
                assert_eq!(out.is_ok(), *need <= held, "{m} needs {need:?}, holds {held:?}");
                // Exempt targets (sandbox/cloud) always run.
                assert!(decide(&notes, &settings, &grants, req, true).1.is_ok());
            }
        }
    }

    #[test]
    fn ceilings_cap_grants() {
        let s = SafetySettings::default();
        let safari = ctx("Safari", "com.apple.Safari");
        let term = ctx("Terminal", "com.apple.Terminal");
        let full = |b: &str| vec![grant(b, Tier::Full)];
        assert_eq!(granted_tier(&safari, &s, &full("com.apple.Safari")).1, Tier::Read);
        assert!(decide(&safari, &s, &full("com.apple.Safari"), Tier::Click, false).1.is_err());
        assert_eq!(granted_tier(&term, &s, &full("com.apple.Terminal")).1, Tier::Click);
        assert!(decide(&term, &s, &full("com.apple.Terminal"), Tier::Click, false).1.is_ok());
        let err = decide(&term, &s, &full("com.apple.Terminal"), Tier::Full, false).1.unwrap_err();
        assert!(err.contains("at most at click"), "{err}");
        // A grant for another app doesn't carry over.
        assert_eq!(granted_tier(&ctx("Notes", "com.apple.Notes"), &s, &full("com.apple.TextEdit")).1, Tier::Read);
    }

    #[test]
    fn unknown_app_fails_closed() {
        let s = SafetySettings { allow_apps: Some(vec!["*".into()]), ..Default::default() };
        let unknown = Context::default();
        let (check, out) = decide(&unknown, &s, &[grant("com.apple.Notes", Tier::Full)], Tier::Click, false);
        assert_eq!(check.category, Category::Unknown);
        assert_eq!(check.granted, Tier::Read);
        assert!(out.unwrap_err().contains("couldn't be identified"));
        assert!(decide(&unknown, &s, &[], Tier::Read, false).1.is_ok(), "reading stays allowed");
    }

    #[test]
    fn deny_beats_grant_and_allow_apps_are_standing_grants() {
        let notes = ctx("Notes", "com.apple.Notes");
        let allow = SafetySettings { allow_apps: Some(vec!["com.apple.Notes".into(), "Terminal".into()]), ..Default::default() };
        assert_eq!(granted_tier(&notes, &allow, &[]).1, Tier::Full);
        assert_eq!(granted_tier(&ctx("Terminal", "com.apple.Terminal"), &allow, &[]).1, Tier::Click, "standing grant capped at ceiling");
        let deny = SafetySettings { allow_apps: Some(vec!["com.apple.Notes".into()]), deny_apps: Some(vec!["Notes".into()]), ..Default::default() };
        assert_eq!(granted_tier(&notes, &deny, &[grant("com.apple.Notes", Tier::Full)]).1, Tier::Read);
        assert!(decide(&notes, &deny, &[grant("com.apple.Notes", Tier::Full)], Tier::Click, false).1.is_err());
        // request_access for a denied app is refused.
        let req = json!({ "entries": [{ "bundle_id": "com.apple.Notes", "tier": "click" }], "reason": "x" });
        assert_eq!(check_request(&req, &deny, false).unwrap_err().0, "safety_denied");
    }

    #[test]
    fn request_checks_ceilings() {
        let s = SafetySettings::default();
        let ask = |b: &str, t: &str| check_request(&json!({ "entries": [{ "bundle_id": b, "tier": t }], "reason": "r" }), &s, false);
        assert_eq!(ask("com.apple.Safari", "click").unwrap_err().0, "tier_exceeded");
        assert!(ask("com.apple.Safari", "read").is_ok());
        assert_eq!(ask("com.apple.Terminal", "full").unwrap_err().0, "tier_exceeded");
        assert!(ask("com.apple.Terminal", "click").is_ok());
        assert!(ask("com.apple.Notes", "full").is_ok());
        assert_eq!(ask("Some App", "click").unwrap_err().0, "tier_exceeded", "unidentified apps top out at read");
        assert!(check_request(&json!({ "entries": [{ "bundle_id": "com.apple.Safari", "tier": "full" }], "reason": "r" }), &s, true).is_ok());
        let r = resolve_static("Safari", false).unwrap();
        assert_eq!((r.bundle_id.as_str(), r.category, r.max_tier), ("com.apple.Safari", Category::Browser, Tier::Read));
        assert_eq!(resolve_static("Safari", true).unwrap().max_tier, Tier::Full);
    }

    #[test]
    fn grants_are_per_session_and_expire() {
        let (owner, cid) = ("test-owner-access", "test-computer-access");
        revoke(owner, cid, None, None);
        record_grant(owner, cid, "run-a", "com.apple.Notes", "Notes", Tier::Click, 1_000);
        record_grant(owner, cid, "run-a", "com.apple.Notes", "Notes", Tier::Full, 1_000);
        record_grant(owner, cid, "run-b", "com.apple.TextEdit", "TextEdit", Tier::Click, 1_000);
        assert_eq!(session_grants(owner, cid, "run-a", 1_001).len(), 1, "a re-grant replaces the old one");
        assert_eq!(session_grants(owner, cid, "run-a", 1_001)[0].tier, Tier::Full);
        assert!(session_grants(owner, cid, "run-c", 1_001).is_empty());
        assert_eq!(list_grants(owner, cid, 1_000 + GRANT_TTL_SECS + 1).len(), 0, "12 h expiry");
        record_grant(owner, cid, "run-a", "com.apple.Notes", "Notes", Tier::Full, 2_000);
        record_grant(owner, cid, "run-b", "com.apple.Notes", "Notes", Tier::Full, 2_000);
        assert_eq!(revoke(owner, cid, Some("com.apple.notes"), Some("run-a")), 1);
        assert_eq!(revoke(owner, cid, None, None), 1);
        assert_eq!(session_key(None), "");
        assert_eq!(session_key(Some("toolset-abc")), "");
        assert_eq!(session_key(Some("run-1")), "run-1");
    }

    #[test]
    fn action_class_vectors() {
        let table: Value = serde_json::from_str(include_str!("../../../contracts/action-classes/vectors.json")).expect("vectors.json parses");
        let rows = table["vectors"].as_array().expect("vectors");
        assert!(rows.len() >= 61);
        let mut failures = Vec::new();
        for row in rows {
            let input = &row["input"];
            let got = classify_action(input["tool"].as_str().unwrap(), input["action"].as_str().unwrap(), &input["args"]);
            let want = row["expected"].as_str().unwrap();
            if got.as_str() != want {
                failures.push(format!("{}: got {}, want {} ({})", row["id"], got.as_str(), want, row["why"]));
            }
        }
        assert!(failures.is_empty(), "action-class vectors:\n{}", failures.join("\n"));
    }

    #[test]
    fn tighten_never_loosens() {
        let all = [ActionClass::Free, ActionClass::ConfirmUnlessNamed, ActionClass::ConfirmNow, ActionClass::HandOff];
        for a in all {
            for b in all {
                let t = tighten(a, b);
                assert!(t >= a && t >= b);
            }
        }
    }
}
