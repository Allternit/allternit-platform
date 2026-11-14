//! What the runner knows about each vendor CLI: its home, environment,
//! sign-in, status, logout and one tools-off chat turn. Every argv and every
//! matched string comes from step 0.2 of the brain-runner plan
//! (`FINDINGS-brain-runner-0.2.md`, measured 2026-11-14 on claude 2.1.325,
//! codex 0.169.0, grok 1.0.77, kimi 2.5.2). Re-measure before changing a pin.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::Config;

/// Linux refuses a single argv string over 128 KiB (`MAX_ARG_STRLEN`). Prompts
/// that travel in argv (grok, kimi, system prompts) stay under this.
pub(crate) const MAX_ARG_BYTES: usize = 120_000;

/// Used when the turn carries no system prompt, so no CLI falls back to its own
/// coding-agent prompt (which talks about tools the runner removed).
pub(crate) const DEFAULT_SYSTEM: &str = "You are a helpful assistant. Answer in plain text.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Vendor {
    Claude,
    Chatgpt,
    Grok,
    Kimi,
}

impl Vendor {
    pub const ALL: [Vendor; 4] = [Vendor::Claude, Vendor::Chatgpt, Vendor::Grok, Vendor::Kimi];

    /// The connections row id, which is also the protocol's `vendor` value.
    pub fn parse(id: &str) -> Option<Vendor> {
        match id.trim() {
            "claude" => Some(Vendor::Claude),
            "chatgpt" => Some(Vendor::Chatgpt),
            "grok" => Some(Vendor::Grok),
            "kimi" => Some(Vendor::Kimi),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Vendor::Claude => "claude",
            Vendor::Chatgpt => "chatgpt",
            Vendor::Grok => "grok",
            Vendor::Kimi => "kimi",
        }
    }

    /// Upper-case form for per-vendor env overrides (`ALLTERNIT_RUNNER_BIN_CLAUDE`).
    pub(crate) fn env_key(self) -> &'static str {
        match self {
            Vendor::Claude => "CLAUDE",
            Vendor::Chatgpt => "CHATGPT",
            Vendor::Grok => "GROK",
            Vendor::Kimi => "KIMI",
        }
    }

    /// The program name, from the shared CLI catalog (`cli-catalog.json`).
    pub(crate) fn binary_name(self) -> String {
        let catalog_id = match self {
            Vendor::Claude => "claude",
            Vendor::Chatgpt => "codex",
            Vendor::Grok => "grok",
            Vendor::Kimi => "kimi",
        };
        crate::cli_catalog::find(catalog_id)
            .map(|tool| tool.bin.clone())
            .unwrap_or_else(|| catalog_id.to_string())
    }

    pub(crate) fn default_model(self) -> &'static str {
        match self {
            Vendor::Claude => "sonnet",
            Vendor::Chatgpt => "gpt-6.1-sol",
            Vendor::Grok => "grok-4.6",
            Vendor::Kimi => "kimi-code/kimi-for-coding",
        }
    }

    /// `login_view.kind` (contract 9.1): Claude is link + paste, the rest device code.
    pub(crate) fn login_kind(self) -> &'static str {
        match self {
            Vendor::Claude => "link_paste",
            _ => "device_code",
        }
    }

    pub(crate) fn login_argv(self) -> &'static [&'static str] {
        match self {
            Vendor::Claude => &["auth", "login", "--claudeai"],
            Vendor::Chatgpt => &["login", "--device-auth"],
            Vendor::Grok => &["login", "--device-auth"],
            Vendor::Kimi => &["login", "--region", "global"],
        }
    }

    pub(crate) fn status_argv(self) -> &'static [&'static str] {
        match self {
            Vendor::Claude => &["auth", "status"],
            Vendor::Chatgpt => &["login", "status"],
            Vendor::Grok => &["models"],
            Vendor::Kimi => &["provider", "list"],
        }
    }

    /// Kimi has no logout command: its sign-in is two files in its home.
    pub(crate) fn logout_argv(self) -> Option<&'static [&'static str]> {
        match self {
            Vendor::Claude => Some(&["auth", "logout"]),
            Vendor::Chatgpt => Some(&["logout"]),
            Vendor::Grok => Some(&["logout"]),
            Vendor::Kimi => None,
        }
    }
}

/// The vendor's private home under the runner volume (`/home/runner/<vendor>`),
/// or a development override. Logout deletes it only when it is inside the
/// runner home, so an override pointing at a real home is never deleted.
pub(crate) fn home(cfg: &Config, vendor: Vendor) -> PathBuf {
    cfg.home_overrides
        .get(&vendor)
        .cloned()
        .unwrap_or_else(|| cfg.home.join(vendor.id()))
}

pub(crate) fn home_is_owned(cfg: &Config, vendor: Vendor) -> bool {
    !cfg.home_overrides.contains_key(&vendor)
}

pub(crate) fn binary(cfg: &Config, vendor: Vendor) -> Option<PathBuf> {
    if let Some(path) = cfg.bins.get(&vendor) {
        return Some(path.clone());
    }
    crate::cli_tools::resolve_binary(&vendor.binary_name())
}

/// Environment for every process the runner starts for `vendor`.
pub(crate) fn env(cfg: &Config, vendor: Vendor) -> Vec<(String, OsString)> {
    let home = home(cfg, vendor).into_os_string();
    let mut env: Vec<(String, OsString)> = vec![
        ("HOME".into(), home.clone()),
        // Sign-in pages open on the person's device, never in the box.
        ("BROWSER".into(), "true".into()),
        ("NO_COLOR".into(), "1".into()),
        ("TERM".into(), "dumb".into()),
    ];
    match vendor {
        Vendor::Claude => {
            for key in ["DISABLE_AUTOUPDATER", "DISABLE_UPDATES", "DISABLE_TELEMETRY", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"] {
                env.push((key.into(), "1".into()));
            }
        }
        Vendor::Chatgpt => env.push(("CODEX_HOME".into(), home)),
        Vendor::Grok => {}
        Vendor::Kimi => {
            env.push(("KIMI_CODE_HOME".into(), home));
            env.push(("KIMI_CODE_NO_AUTO_UPDATE".into(), "1".into()));
        }
    }
    env
}

// ─── Status ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignIn {
    In { plan: Option<String> },
    Out,
}

/// Reads a status command's result. `text` is stdout and stderr, ANSI stripped.
pub(crate) fn parse_status(vendor: Vendor, exit_ok: bool, text: &str) -> SignIn {
    let lower = text.to_lowercase();
    match vendor {
        // `claude auth status` prints JSON; exit 0 + loggedIn:true, else exit 1.
        Vendor::Claude => {
            let parsed = text
                .find('{')
                .and_then(|start| serde_json::from_str::<Value>(text[start..].trim()).ok());
            match parsed {
                Some(v) if v.get("loggedIn").and_then(Value::as_bool) == Some(true) => SignIn::In {
                    plan: v.get("subscriptionType").and_then(Value::as_str).map(str::to_string),
                },
                _ => SignIn::Out,
            }
        }
        // `codex login status`: exit 0 "Logged in using ChatGPT" / exit 1 "Not logged in".
        Vendor::Chatgpt => {
            if exit_ok && lower.contains("logged in") && !lower.contains("not logged in") {
                SignIn::In { plan: None }
            } else {
                SignIn::Out
            }
        }
        // `grok models` exits 0 either way; signed out prints "You are not authenticated."
        Vendor::Grok => {
            if exit_ok && !lower.contains("not authenticated") && !lower.contains("not signed in") {
                SignIn::In { plan: None }
            } else {
                SignIn::Out
            }
        }
        // `kimi provider list` exits 0 always: `managed:kimi-code … source=oauth`
        // when signed in, "No providers configured." when not.
        Vendor::Kimi => {
            if lower.contains("source=oauth") {
                SignIn::In { plan: None }
            } else {
                SignIn::Out
            }
        }
    }
}

// ─── Failures ────────────────────────────────────────────────────────────────

/// Maps a CLI's error text to the protocol code: `vendor_limited` (plan limit,
/// lapsed subscription) or `login_expired` (signed out). `None` = something
/// else went wrong (`cli_failed`). Limits are checked first: Kimi's lapsed
/// subscription arrives as `provider.auth_error`, which is not a sign-out.
pub(crate) fn classify_failure(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    const LIMITED: &[&str] = &[
        "you've hit your usage limit",
        "hit your limit",
        "usage limit reached",
        "does not have access to kimi code",
        "upgrade your plan",
    ];
    const SIGNED_OUT: &[&str] = &[
        "not logged in",
        "please run /login",
        "authentication_failed",
        "missing bearer",
        "not signed in",
        "you are not authenticated",
        "no model configured",
        "no providers configured",
    ];
    if LIMITED.iter().any(|needle| lower.contains(needle)) {
        Some("vendor_limited")
    } else if SIGNED_OUT.iter().any(|needle| lower.contains(needle)) {
        Some("login_expired")
    } else {
        None
    }
}

// ─── Turns ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Msg {
    pub user: bool,
    pub content: String,
}

/// How to run one turn: argv after the program, stdin, and files to write first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TurnPlan {
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub files: Vec<(PathBuf, String)>,
}

/// `model` reaches argv, so it is a plain id, never a flag.
pub(crate) fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 100
        && !model.starts_with('-')
        && model.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':'))
}

/// A TOML basic string. JSON string escaping is a subset TOML accepts.
fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

pub(crate) fn codex_catalog_path(cfg: &Config) -> PathBuf {
    home(cfg, Vendor::Chatgpt).join("allternit-tools-off-catalog.json")
}

pub(crate) fn kimi_agent_file(cfg: &Config) -> PathBuf {
    home(cfg, Vendor::Kimi).join("allternit-chat-only.md")
}

/// The earlier turns as plain text, then the latest user message (for CLIs
/// that take one prompt). Oldest turns are dropped until it fits `max` bytes.
pub(crate) fn transcript(history: &[Msg], max: usize) -> Result<String, String> {
    let Some((last, earlier)) = history.split_last() else {
        return Err("no messages".into());
    };
    if last.content.len() > max {
        return Err("the message is too long".into());
    }
    let render = |earlier: &[Msg]| {
        if earlier.is_empty() {
            return last.content.clone();
        }
        let mut text = String::from("Here is the conversation so far:\n\n");
        for m in earlier {
            text.push_str(if m.user { "[User]\n" } else { "[Assistant]\n" });
            text.push_str(&m.content);
            text.push_str("\n\n");
        }
        text.push_str("Reply to the user's latest message:\n\n");
        text.push_str(&last.content);
        text
    };
    let mut start = 0;
    loop {
        let text = render(&earlier[start..]);
        if text.len() <= max || start >= earlier.len() {
            return Ok(text);
        }
        start += 1;
    }
}

/// Claude's history replay: one stream-json line per turn, the new user line last.
pub(crate) fn claude_stdin(history: &[Msg]) -> String {
    let mut out = String::new();
    for m in history {
        let line = if m.user {
            json!({"type": "user", "message": {"role": "user", "content": m.content}})
        } else {
            json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": m.content}]}})
        };
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// The tools-off one-turn command for `vendor`. `history` ends with a user message.
pub(crate) fn turn_plan(
    cfg: &Config,
    vendor: Vendor,
    model: &str,
    system: &str,
    history: &[Msg],
) -> Result<TurnPlan, String> {
    if !valid_model(model) {
        return Err("unknown model".into());
    }
    if system.len() > MAX_ARG_BYTES {
        return Err("the system prompt is too long".into());
    }
    let s = |v: &str| v.to_string();
    Ok(match vendor {
        Vendor::Claude => TurnPlan {
            args: vec![
                s("-p"), s("--tools"), s(""), s("--strict-mcp-config"), s("--mcp-config"), s(r#"{"mcpServers":{}}"#),
                s("--setting-sources"), s(""), s("--disable-slash-commands"), s("--no-session-persistence"),
                s("--system-prompt"), s(system), s("--input-format"), s("stream-json"),
                s("--output-format"), s("stream-json"), s("--include-partial-messages"), s("--verbose"),
                s("--model"), s(model),
            ],
            stdin: Some(claude_stdin(history)),
            files: vec![],
        },
        Vendor::Chatgpt => {
            let catalog = codex_catalog_path(cfg);
            TurnPlan {
                args: vec![
                    s("exec"), s("--ignore-user-config"), s("--ignore-rules"), s("--ephemeral"), s("--skip-git-repo-check"),
                    s("-s"), s("read-only"), s("--json"),
                    s("-c"), format!("model_catalog_json={}", toml_string(&catalog.to_string_lossy())),
                    s("-c"), format!("model={}", toml_string(model)),
                    s("-c"), s("check_for_update_on_startup=false"),
                    s("-c"), format!("developer_instructions={}", toml_string(system)),
                    s("--disable"), s("multi_agent"), s("--disable"), s("multi_agent_v2"), s("--disable"), s("view_image"),
                    s("-m"), s(model), s("-"),
                ],
                // stdin has no argv limit, but keep the same shape everywhere.
                stdin: Some(transcript(history, usize::MAX)?),
                files: vec![],
            }
        }
        Vendor::Grok => TurnPlan {
            args: vec![
                s("-p"), transcript(history, MAX_ARG_BYTES)?, s("--tools"), s(""), s("--no-subagents"),
                s("--disable-web-search"), s("--no-plan"), s("--permission-mode"), s("dontAsk"),
                s("--system-prompt-override"), s(system),
                s("--output-format"), s("streaming-messages-json"), s("--include-partial-messages"),
                s("-m"), s(model),
            ],
            stdin: None,
            files: vec![],
        },
        Vendor::Kimi => {
            let agent_file = kimi_agent_file(cfg);
            let body = format!("---\nname: chat-only\ndescription: Plain chat, no tools\ntools: []\n---\n{system}\n");
            TurnPlan {
                args: vec![
                    s("--agent-file"), agent_file.to_string_lossy().into_owned(), s("-m"), s(model),
                    s("-p"), transcript(history, MAX_ARG_BYTES)?, s("--output-format"), s("stream-json"),
                ],
                stdin: None,
                files: vec![(agent_file, body)],
            }
        }
    })
}

/// Codex keeps its tool list in the model catalog and no flag removes it, so
/// the runner runs codex against a copy of `codex debug models` with the
/// tool fields cleared (step 0.2's list).
pub(crate) fn codex_tools_off(mut catalog: Value) -> Result<Value, String> {
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .filter(|models| !models.is_empty())
        .ok_or("codex debug models printed no models")?;
    for model in models.iter_mut() {
        let Some(fields) = model.as_object_mut() else { continue };
        fields.insert("shell_type".into(), json!("disabled"));
        fields.insert("apply_patch_tool_type".into(), Value::Null);
        fields.insert("tool_mode".into(), json!("default"));
        fields.insert("input_modalities".into(), json!(["text"]));
        fields.insert("multi_agent_version".into(), Value::Null);
        fields.insert("experimental_supported_tools".into(), json!([]));
        fields.insert("supports_search_tool".into(), json!(false));
    }
    Ok(catalog)
}

/// Login link → `login_view` fields. Grok and Kimi print a link with the code
/// in it (`?user_code=`): that is `url_with_code`, and `url` is the bare page.
pub(crate) fn login_fields(vendor: Vendor, url: Option<&str>, code: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let mut code = code.map(str::to_string);
    let Some(url) = url else { return (None, code, None) };
    if vendor == Vendor::Claude {
        return (Some(url.to_string()), None, None);
    }
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some((_, value)) = parsed.query_pairs().find(|(k, _)| k == "user_code") {
            if code.is_none() {
                code = Some(value.into_owned());
            }
            let mut bare = parsed.clone();
            bare.set_query(None);
            return (Some(bare.to_string()), code, Some(url.to_string()));
        }
    }
    (Some(url.to_string()), code, None)
}

pub(crate) fn path_is_inside(path: &Path, root: &Path) -> bool {
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(path), Ok(root)) => path.starts_with(&root) && path != root,
        _ => false,
    }
}
