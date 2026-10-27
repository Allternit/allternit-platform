//! Per-repo tree settings: `agent-trees.toml` at the repo root (optional).
//!
//! ```toml
//! [seed]
//! exclude = [".git", ".steering/state"]     # never cloned from the base
//! [[seed.deps]]                               # run in a new tree, in order
//! when = "pnpm-lock.yaml"                     # only if this file exists
//! run = "CI=true pnpm install --frozen-lockfile --offline --ignore-scripts"
//! [base]
//! build = ["cargo check --workspace"]         # warms the base after a refresh
//! [clean]
//! build_dirs = ["target", "node_modules", "dist"]
//! [reaper]
//! idle_hours = 24
//! ```

use std::path::Path;

use serde::Deserialize;

pub const CONFIG_FILE: &str = "agent-trees.toml";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TreesConfig {
    pub seed: SeedConfig,
    pub base: BaseConfig,
    pub clean: CleanConfig,
    pub reaper: ReaperConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SeedConfig {
    pub exclude: Vec<String>,
    pub deps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Step {
    pub when: Option<String>,
    pub run: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct BaseConfig {
    pub build: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct CleanConfig {
    pub build_dirs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ReaperConfig {
    /// A tree with no process and no file touched for this long is idle.
    pub idle_hours: u64,
    /// Dev servers running longer than this are stopped by the reaper.
    pub dev_server_hours: u64,
    /// Command-line fragments that mark a process as a dev server.
    pub dev_server_patterns: Vec<String>,
    /// `tree new` refuses below this much free disk.
    pub min_free_gb: u64,
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self {
            exclude: vec![".git".into(), ".steering/state".into(), ".session-worktree".into()],
            deps: vec![Step {
                when: Some("pnpm-lock.yaml".into()),
                // Offline first: hardlinks from the shared pnpm store, no network.
                run: "CI=true pnpm install --frozen-lockfile --offline --ignore-scripts \
                      || CI=true pnpm install --frozen-lockfile --ignore-scripts"
                    .into(),
            }],
        }
    }
}

impl Default for CleanConfig {
    fn default() -> Self {
        Self {
            build_dirs: ["target", "node_modules", "dist", ".next", "release", ".turbo", ".vite"]
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }
}

impl Default for ReaperConfig {
    fn default() -> Self {
        Self {
            idle_hours: 24,
            dev_server_hours: 24,
            dev_server_patterns: [
                "vite",
                "next dev",
                "wrangler dev",
                "wrangler pages dev",
                "wrangler pages deployment tail",
                "webpack serve",
                "pnpm dev",
                "pnpm run dev",
                "npm run dev",
                "storybook dev",
                "astro dev",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            min_free_gb: 50,
        }
    }
}

impl TreesConfig {
    /// `agent-trees.toml` from `repo_root`, or the defaults. A file that does
    /// not parse is an error (silently ignoring it would seed the wrong way).
    pub fn load(repo_root: &Path) -> anyhow::Result<TreesConfig> {
        let path = repo_root.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TreesConfig::default()),
            Err(e) => Err(anyhow::anyhow!("reading {}: {e}", path.display())),
        }
    }

    /// Matched on the program and its first arguments (basenames), not the
    /// whole command line: a shell whose script mentions `vite` is not one.
    pub fn is_dev_server(&self, command: &str) -> bool {
        let head: Vec<&str> = command
            .split_whitespace()
            .take(4)
            .map(|t| t.rsplit('/').next().unwrap_or(t))
            .collect();
        let head = head.join(" ");
        self.reaper.dev_server_patterns.iter().any(|p| {
            head.split_whitespace().collect::<Vec<_>>().windows(p.split_whitespace().count().max(1)).any(|w| w.join(" ") == *p)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let c = TreesConfig::load(dir.path()).unwrap();
        assert_eq!(c.reaper.min_free_gb, 50);
        assert!(c.clean.build_dirs.contains(&"target".to_string()));
        std::fs::write(dir.path().join(CONFIG_FILE), "[reaper]\nidle_hours = 6\n").unwrap();
        let c = TreesConfig::load(dir.path()).unwrap();
        assert_eq!(c.reaper.idle_hours, 6);
        assert_eq!(c.reaper.min_free_gb, 50, "unset keys keep defaults");
        assert!(c.is_dev_server("node /x/node_modules/.bin/vite --port 5173"));
        assert!(c.is_dev_server("npx wrangler pages dev ./dist"));
        assert!(c.is_dev_server("pnpm run dev"));
        assert!(!c.is_dev_server("/bin/zsh -c source snap && vite build"));
        assert!(!c.is_dev_server("node /x/.bin/vitest run"));
        std::fs::write(dir.path().join(CONFIG_FILE), "[reaper\n").unwrap();
        assert!(TreesConfig::load(dir.path()).is_err());
    }
}
