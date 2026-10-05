//! Operator config for campaigns, wakes and the attention gate:
//! `.allternit/rails/automation.yaml` (optional; every field has a safe default).
//!
//! ```yaml
//! wake:
//!   # Executor kinds a sweep may run itself. Default: none.
//!   # Only `command` is runnable today; bot/ao always raise a needs-you item
//!   # (spawn gating lands with the drive runner).
//!   enabled_executors: [command]
//!   # Exact command strings a `command` executor may run.
//!   command_allowlist:
//!     - /bin/bash "/path/to/script.sh"
//!   command_timeout_secs: 900
//! campaign:
//!   check_ceiling_secs: 604800   # 7 days
//! attention:
//!   timezone: America/Chicago
//!   quiet_hours: { start: "22:30", end: "06:00" }
//!   dedupe_window_secs: 86400
//!   per_hour_cap: 6
//!   recipient: joe
//! ```

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::attention::AttentionConfig;
use crate::campaign::DEFAULT_CHECK_CEILING_SECS;

pub const AUTOMATION_CONFIG_PATH: &str = ".allternit/rails/automation.yaml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationConfig {
    pub wake: WakeConfig,
    pub campaign: CampaignConfig,
    pub attention: AttentionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WakeConfig {
    pub enabled_executors: Vec<String>,
    pub command_allowlist: Vec<String>,
    pub command_timeout_secs: u64,
}

impl Default for WakeConfig {
    fn default() -> Self {
        Self {
            enabled_executors: Vec::new(),
            command_allowlist: Vec::new(),
            command_timeout_secs: 900,
        }
    }
}

impl WakeConfig {
    /// Whether a sweep may run this command itself: `command` is enabled and
    /// the exact string is allowlisted.
    pub fn command_runnable(&self, cmd: &str) -> bool {
        self.enabled_executors.iter().any(|e| e == "command")
            && self.command_allowlist.iter().any(|c| c == cmd)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CampaignConfig {
    pub check_ceiling_secs: i64,
}

impl Default for CampaignConfig {
    fn default() -> Self {
        Self {
            check_ceiling_secs: DEFAULT_CHECK_CEILING_SECS,
        }
    }
}

impl AutomationConfig {
    /// Load `<root>/.allternit/rails/automation.yaml`, or defaults when absent.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(AUTOMATION_CONFIG_PATH);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Self =
            serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.attention.policy()?;
        Ok(cfg)
    }
}
