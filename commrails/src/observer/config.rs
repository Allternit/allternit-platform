//! Observer configuration: `.allternit/rails/observer.json` + env overrides.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const OBSERVER_CONFIG_PATH: &str = ".allternit/rails/observer.json";

fn default_true() -> bool {
    true
}
fn default_threshold() -> u32 {
    2
}
fn default_timeout() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserverConfig {
    /// Consult command: `claude`, `kimi`, `codex` (optionally `--model X`),
    /// or any command with `read_only_attested: true`.
    /// Env `ALLTERNIT_OBSERVER_CMD` overrides.
    #[serde(default)]
    pub consult_cmd: Option<String>,
    /// Operator attests that exactly `consult_cmd` has no write tools (it
    /// never covers a `--consult-cmd` override or `STEER_CONSULT_CMD`). Env
    /// `ALLTERNIT_OBSERVER_READ_ONLY_ATTESTED=1` overrides.
    #[serde(default)]
    pub read_only_attested: bool,
    /// Observe after `plan new` (opt-in).
    #[serde(default)]
    pub observe_on_plan: bool,
    /// Observe when a node fails with the same signature `repeat_failure_threshold` times.
    #[serde(default = "default_true")]
    pub observe_on_repeat_failure: bool,
    #[serde(default = "default_threshold")]
    pub repeat_failure_threshold: u32,
    /// Policy: observe before every `wih close` (opt-in).
    #[serde(default)]
    pub observe_before_close: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            consult_cmd: None,
            read_only_attested: false,
            observe_on_plan: false,
            observe_on_repeat_failure: true,
            repeat_failure_threshold: 2,
            observe_before_close: false,
            timeout_secs: 300,
        }
    }
}

impl ObserverConfig {
    /// Load `<root>/.allternit/rails/observer.json` (defaults when absent),
    /// then apply env overrides.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(OBSERVER_CONFIG_PATH);
        let mut cfg: ObserverConfig = if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            ObserverConfig::default()
        };
        if let Ok(cmd) = std::env::var("ALLTERNIT_OBSERVER_CMD") {
            if !cmd.trim().is_empty() {
                cfg.consult_cmd = Some(cmd);
            }
        }
        if std::env::var("ALLTERNIT_OBSERVER_READ_ONLY_ATTESTED").as_deref() == Ok("1") {
            cfg.read_only_attested = true;
        }
        Ok(cfg)
    }

    /// The command for an explicit `observe` run: the observer's own, else
    /// the steering consult command (`STEER_CONSULT_CMD`), still forced
    /// through a read-only profile.
    pub fn explicit_consult_cmd(&self) -> Option<String> {
        self.consult_cmd.clone().or_else(|| {
            std::env::var("STEER_CONSULT_CMD")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
    }

    /// The attestation binds to the configured command string only.
    pub fn attested_for(&self, cmd: &str) -> bool {
        self.read_only_attested && self.consult_cmd.as_deref().map(str::trim) == Some(cmd.trim())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }
}
