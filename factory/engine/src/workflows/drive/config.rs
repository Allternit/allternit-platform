//! `drive` configuration: `.allternit/drive/config.json` (all fields optional).
//!
//! The global caps and the capacity thresholds live in this file, not on the
//! command line, so every `drive` process on the machine reads the same
//! limits. Per-DAG caps default from here and can be lowered per run with
//! `--max-concurrent` / `--max-spawns-per-hour`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Default per-DAG and global concurrent-session cap (audit S10).
pub const DEFAULT_MAX_CONCURRENT: usize = 4;
/// Default per-DAG and global spawns-per-rolling-hour cap (audit S10).
pub const DEFAULT_MAX_SPAWNS_PER_HOUR: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveConfig {
    /// Per-DAG concurrent sessions (a run's `--max-concurrent` can only lower it).
    pub max_concurrent: usize,
    /// Per-DAG spawns per rolling hour.
    pub max_spawns_per_hour: usize,
    /// Concurrent drive sessions across every DAG and every drive process.
    pub global_max_concurrent: usize,
    /// Spawns per rolling hour across every DAG and every drive process.
    pub global_max_spawns_per_hour: usize,
    /// Capacity admission: minimum available memory (MiB).
    pub min_free_mem_mb: u64,
    /// Capacity admission: maximum 1-minute load average per CPU.
    pub max_load_per_cpu: f64,
    /// Per-attempt wall clock before the session is killed and the node FAILED.
    pub timeout_seconds: u64,
    /// Poll interval of the foreground loop.
    pub poll_interval_ms: u64,
    /// `ao:<harness>` → argv template. Placeholders: `{prompt}` (full text),
    /// `{prompt_file}`, `{wih_id}`, `{dag_id}`, `{node_id}`. The spawn gate
    /// then rewrites the argv (hook settings for claude, sandbox for codex).
    pub harnesses: BTreeMap<String, HarnessSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSpec {
    pub argv: Vec<String>,
}

impl Default for DriveConfig {
    fn default() -> Self {
        let mut harnesses = BTreeMap::new();
        harnesses.insert(
            "claude".to_string(),
            HarnessSpec {
                argv: vec!["claude".into(), "-p".into(), "{prompt}".into()],
            },
        );
        harnesses.insert(
            "codex".to_string(),
            HarnessSpec {
                argv: vec!["codex".into(), "exec".into(), "{prompt}".into()],
            },
        );
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            max_spawns_per_hour: DEFAULT_MAX_SPAWNS_PER_HOUR,
            global_max_concurrent: DEFAULT_MAX_CONCURRENT,
            global_max_spawns_per_hour: DEFAULT_MAX_SPAWNS_PER_HOUR,
            min_free_mem_mb: 512,
            max_load_per_cpu: 2.0,
            timeout_seconds: 3600,
            poll_interval_ms: 2000,
            harnesses,
        }
    }
}

pub fn drive_dir(root: &Path) -> PathBuf {
    root.join(".allternit/drive")
}

pub fn config_path(root: &Path) -> PathBuf {
    drive_dir(root).join("config.json")
}

impl DriveConfig {
    /// Load `.allternit/drive/config.json` over the defaults. Harness entries
    /// in the file replace the default entry of the same name; the others
    /// stay. Env overrides for the capacity thresholds:
    /// `ALLTERNIT_DRIVE_MIN_FREE_MEM_MB`, `ALLTERNIT_DRIVE_MAX_LOAD_PER_CPU`.
    pub fn load(root: &Path) -> Result<Self> {
        let path = config_path(root);
        let mut cfg = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let file: DriveConfigFile = serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
            file.apply(Self::default())
        } else {
            Self::default()
        };
        if let Ok(v) = std::env::var("ALLTERNIT_DRIVE_MIN_FREE_MEM_MB") {
            cfg.min_free_mem_mb = v.parse().context("ALLTERNIT_DRIVE_MIN_FREE_MEM_MB")?;
        }
        if let Ok(v) = std::env::var("ALLTERNIT_DRIVE_MAX_LOAD_PER_CPU") {
            cfg.max_load_per_cpu = v.parse().context("ALLTERNIT_DRIVE_MAX_LOAD_PER_CPU")?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("max_concurrent", self.max_concurrent),
            ("max_spawns_per_hour", self.max_spawns_per_hour),
            ("global_max_concurrent", self.global_max_concurrent),
            ("global_max_spawns_per_hour", self.global_max_spawns_per_hour),
        ] {
            if value == 0 {
                bail!("drive config: {name} must be at least 1");
            }
        }
        for (name, spec) in &self.harnesses {
            if spec.argv.is_empty() {
                bail!("drive config: harness {name} has an empty argv");
            }
        }
        Ok(())
    }

    /// Render the argv for `ao:<harness>`; `None` when the harness has no entry.
    pub fn harness_argv(&self, harness: &str, vars: &ArgvVars<'_>) -> Option<Vec<String>> {
        let spec = self.harnesses.get(harness)?;
        Some(spec.argv.iter().map(|w| vars.render(w)).collect())
    }
}

/// Values substituted into a harness argv template.
pub struct ArgvVars<'a> {
    pub prompt: &'a str,
    pub prompt_file: &'a str,
    pub wih_id: &'a str,
    pub dag_id: &'a str,
    pub node_id: &'a str,
}

impl ArgvVars<'_> {
    fn render(&self, word: &str) -> String {
        word.replace("{prompt_file}", self.prompt_file)
            .replace("{wih_id}", self.wih_id)
            .replace("{dag_id}", self.dag_id)
            .replace("{node_id}", self.node_id)
            .replace("{prompt}", self.prompt)
    }
}

/// On-disk shape: every field optional so a file can override one value.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DriveConfigFile {
    max_concurrent: Option<usize>,
    max_spawns_per_hour: Option<usize>,
    global_max_concurrent: Option<usize>,
    global_max_spawns_per_hour: Option<usize>,
    min_free_mem_mb: Option<u64>,
    max_load_per_cpu: Option<f64>,
    timeout_seconds: Option<u64>,
    poll_interval_ms: Option<u64>,
    #[serde(default)]
    harnesses: BTreeMap<String, HarnessSpec>,
}

impl DriveConfigFile {
    fn apply(self, mut cfg: DriveConfig) -> DriveConfig {
        macro_rules! set {
            ($($f:ident),*) => { $( if let Some(v) = self.$f { cfg.$f = v; } )* };
        }
        set!(
            max_concurrent,
            max_spawns_per_hour,
            global_max_concurrent,
            global_max_spawns_per_hour,
            min_free_mem_mb,
            max_load_per_cpu,
            timeout_seconds,
            poll_interval_ms
        );
        cfg.harnesses.extend(self.harnesses);
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_overrides_merge_over_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(drive_dir(tmp.path())).unwrap();
        std::fs::write(
            config_path(tmp.path()),
            r#"{"global_max_concurrent": 1, "harnesses": {"claude": {"argv": ["/stub/claude", "{prompt_file}"]}}}"#,
        )
        .unwrap();
        let cfg = DriveConfig::load(tmp.path()).unwrap();
        assert_eq!(cfg.global_max_concurrent, 1);
        assert_eq!(cfg.max_concurrent, DEFAULT_MAX_CONCURRENT);
        assert!(cfg.harnesses.contains_key("codex"));
        let vars = ArgvVars { prompt: "p", prompt_file: "/f", wih_id: "w", dag_id: "d", node_id: "n" };
        assert_eq!(cfg.harness_argv("claude", &vars).unwrap(), vec!["/stub/claude", "/f"]);
        assert!(cfg.harness_argv("kimi", &vars).is_none());
    }

    #[test]
    fn zero_caps_and_unknown_fields_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(drive_dir(tmp.path())).unwrap();
        std::fs::write(config_path(tmp.path()), r#"{"max_concurrent": 0}"#).unwrap();
        assert!(DriveConfig::load(tmp.path()).is_err());
        std::fs::write(config_path(tmp.path()), r#"{"max_concurent": 2}"#).unwrap();
        assert!(DriveConfig::load(tmp.path()).is_err());
    }
}
