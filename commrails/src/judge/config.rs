//! Judge backend configuration: `<root>/.allternit/judge/config.json`.
//!
//! ```json
//! {
//!   "backend": "command",              // or "stub"
//!   "timeout_secs": 180,               // node verdicts
//!   "tool_timeout_secs": 30,           // tool decisions
//!   "command": { "argv": ["claude", "-p", "..."], "cwd": null, "env": {} },
//!   "stub": { "node": "accomplished", "tool": "allow" },
//!   "system_one": { "url": "http://127.0.0.1:7717", "model": "jev-latest",
//!                   "confidence_band": 0.85, "timeout_ms": 3000 }
//! }
//! ```
//!
//! A missing file means the default command backend. An unreadable or
//! invalid file does **not** fall back to anything permissive: the judge is
//! built as [`FailedJudge`], so every verdict is `needs_human` and every
//! tool decision is `ask` until the file is fixed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::judge::backends::{CommandJudge, FailedJudge, Judge, StubJudge, SystemOneFirstPass};

pub const CONFIG_REL_PATH: &str = ".allternit/judge/config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    #[default]
    Command,
    Stub,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandConfig {
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Default for CommandConfig {
    /// Claude Code in print mode with a forced JSON schema, no tools, only
    /// project settings (so user-level hooks such as steering do not fire),
    /// no session persistence, a small fast model. Runs in a temp cwd.
    fn default() -> Self {
        Self {
            argv: [
                "claude",
                "-p",
                "--output-format",
                "json",
                "--model",
                "haiku",
                "--tools",
                "",
                "--setting-sources",
                "project",
                "--no-session-persistence",
                "--json-schema",
                "{json_schema}",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StubConfig {
    #[serde(default = "default_stub_node")]
    pub node: String,
    #[serde(default = "default_stub_tool")]
    pub tool: String,
}

fn default_stub_node() -> String {
    "accomplished".to_string()
}
fn default_stub_tool() -> String {
    "ask".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemOneConfig {
    #[serde(default = "default_s1_url")]
    pub url: String,
    #[serde(default = "default_s1_model")]
    pub model: String,
    #[serde(default = "default_s1_band")]
    pub confidence_band: f64,
    #[serde(default = "default_s1_timeout")]
    pub timeout_ms: u64,
    /// Bearer token; falls back to `SYSTEM_ONE_TOKEN` in the environment.
    #[serde(default)]
    pub token: Option<String>,
}

fn default_s1_url() -> String {
    "http://127.0.0.1:7717".to_string()
}
fn default_s1_model() -> String {
    "jev-latest".to_string()
}
fn default_s1_band() -> f64 {
    0.85
}
fn default_s1_timeout() -> u64 {
    3000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeConfig {
    #[serde(default)]
    pub backend: BackendKind,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: u64,
    #[serde(default)]
    pub command: CommandConfig,
    #[serde(default)]
    pub stub: Option<StubConfig>,
    #[serde(default)]
    pub system_one: Option<SystemOneConfig>,
}

fn default_timeout() -> u64 {
    180
}
fn default_tool_timeout() -> u64 {
    30
}

impl Default for JudgeConfig {
    fn default() -> Self {
        Self {
            backend: BackendKind::Command,
            timeout_secs: default_timeout(),
            tool_timeout_secs: default_tool_timeout(),
            command: CommandConfig::default(),
            stub: None,
            system_one: None,
        }
    }
}

impl JudgeConfig {
    pub fn node_timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }
    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.tool_timeout_secs.max(1))
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_REL_PATH)
}

/// Load the config. `Ok(default)` when the file is absent; `Err` when it
/// exists but is unreadable or invalid.
pub fn load_config(root: &Path) -> Result<JudgeConfig> {
    let path = config_path(root);
    if !path.exists() {
        return Ok(JudgeConfig::default());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| anyhow!("read {}: {e}", path.display()))?;
    let cfg: JudgeConfig =
        serde_json::from_str(&text).map_err(|e| anyhow!("parse {}: {e}", path.display()))?;
    if cfg.backend == BackendKind::Command && cfg.command.argv.is_empty() {
        return Err(anyhow!("{}: command.argv is empty", path.display()));
    }
    if let Some(s1) = &cfg.system_one {
        if !(0.0..=1.0).contains(&s1.confidence_band) {
            return Err(anyhow!(
                "{}: system_one.confidence_band must be in [0,1]",
                path.display()
            ));
        }
    }
    Ok(cfg)
}

/// Build the judge for `cfg`.
pub fn build_judge(cfg: &JudgeConfig) -> Arc<dyn Judge> {
    let base: Arc<dyn Judge> = match cfg.backend {
        BackendKind::Command => Arc::new(CommandJudge {
            argv: cfg.command.argv.clone(),
            cwd: cfg.command.cwd.clone(),
            env: cfg.command.env.clone().into_iter().collect(),
        }),
        BackendKind::Stub => {
            let stub = cfg.stub.clone().unwrap_or(StubConfig {
                node: default_stub_node(),
                tool: default_stub_tool(),
            });
            Arc::new(StubJudge::new(&stub.node, &stub.tool))
        }
    };
    match &cfg.system_one {
        Some(s1) => Arc::new(SystemOneFirstPass {
            url: s1.url.clone(),
            model: s1.model.clone(),
            confidence_band: s1.confidence_band,
            timeout: Duration::from_millis(s1.timeout_ms.max(1)),
            token: s1
                .token
                .clone()
                .or_else(|| std::env::var("SYSTEM_ONE_TOKEN").ok()),
            next: base,
        }),
        None => base,
    }
}

/// Load + build, failing closed: an invalid config yields a judge whose
/// every call fails. Returns the timeouts alongside.
pub fn judge_for_root(root: &Path) -> (Arc<dyn Judge>, Duration, Duration) {
    match load_config(root) {
        Ok(cfg) => (build_judge(&cfg), cfg.node_timeout(), cfg.tool_timeout()),
        Err(err) => {
            let d = JudgeConfig::default();
            (
                Arc::new(FailedJudge {
                    error: format!("judge config invalid: {err}"),
                }),
                d.node_timeout(),
                d.tool_timeout(),
            )
        }
    }
}
