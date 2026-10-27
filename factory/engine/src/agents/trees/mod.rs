//! Agent Trees: one leased git worktree per agent (Factory part `agents`).
//!
//! A tree is a linked worktree with a lease (owner, state, expiry) kept in
//! `<factory home>/trees/leases.json`. `new` seeds it from the repo's warm
//! base (an APFS clone or Linux reflink of source + build output with mtimes
//! kept, so cargo sees unchanged crates as fresh), `done` lands it (merge
//! check, stop its processes, delete build output, branch and checkpoints,
//! remove the worktree), and `reap` applies the merged + idle + clean rule to
//! every worktree on the machine. Checkpoints are commits under
//! `refs/checkpoints/<tree id>/<n>` taken after each edit by the CLI hooks.
//!
//! The rules here are the AGENTS.md worktree rules written as code: branch
//! from fresh origin/main, the free-disk gate, ownership, forward-only sync,
//! paused trees are never reaped, kill what you started, teardown on landing.

pub mod checkpoint;
pub mod config;
pub mod git;
pub mod http;
pub mod land;
pub mod lease;
pub mod ops;
pub mod overlap;
pub mod procs;
pub mod reaper;
pub mod seed;

use serde::{Deserialize, Serialize};

pub use config::TreesConfig;
pub use lease::{Lease, LeaseStore};

/// Who a tree belongs to. Only the owner (or a person passing `--as-owner`)
/// may pause, rewind or land it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub kind: OwnerKind,
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerKind {
    Session,
    Bot,
    Human,
}

impl Owner {
    /// `kind:id` (`session:abc123`, `bot:writer@docs`, `human:joe`).
    pub fn parse(spec: &str) -> Option<Owner> {
        let (kind, id) = spec.split_once(':')?;
        let kind = match kind {
            "session" => OwnerKind::Session,
            "bot" => OwnerKind::Bot,
            "human" => OwnerKind::Human,
            _ => return None,
        };
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        Some(Owner { kind, id: id.to_string(), label: id.to_string() })
    }

    /// The caller: `--owner`, else `ALLTERNIT_TREE_OWNER`, else the Factory
    /// bot this terminal is, else the person at the keyboard.
    pub fn resolve(explicit: Option<&str>) -> Owner {
        if let Some(o) = explicit.and_then(Owner::parse) {
            return o;
        }
        if let Some(o) = std::env::var("ALLTERNIT_TREE_OWNER").ok().as_deref().and_then(Owner::parse) {
            return o;
        }
        if let Ok(bot) = std::env::var(crate::agents::whoami::ENV_BOT) {
            if !bot.is_empty() {
                let team = std::env::var(crate::agents::whoami::ENV_TEAM).unwrap_or_default();
                let id = if team.is_empty() { bot.clone() } else { format!("{bot}@{team}") };
                return Owner { kind: OwnerKind::Bot, label: id.clone(), id };
            }
        }
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "person".into());
        Owner { kind: OwnerKind::Human, label: user.clone(), id: user }
    }

    pub fn spec(&self) -> String {
        let kind = match self.kind {
            OwnerKind::Session => "session",
            OwnerKind::Bot => "bot",
            OwnerKind::Human => "human",
        };
        format!("{kind}:{}", self.id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeState {
    Active,
    Paused,
    Done,
}

/// How a tree was seeded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedInfo {
    pub strategy: SeedStrategy,
    pub seconds: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeedStrategy {
    /// Source + build output cloned from the warm base (APFS clone / reflink).
    Clone,
    /// A plain checkout; dependency steps still ran unless `--source-only`.
    Plain,
}

/// A tree's id: `<repo>~<branch slug>` (or `<repo>~<dir name>` when detached).
pub fn tree_id(repo: &str, branch_or_dir: &str) -> String {
    let slug: String = branch_or_dir
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    format!("{repo}~{}", slug.trim_matches('-'))
}

/// Errors carry the API.md code so the CLI and HTTP map them the same way.
#[derive(Debug)]
pub struct TreeError {
    pub code: &'static str,
    pub fact: String,
    pub action: String,
}

impl TreeError {
    pub fn new(code: &'static str, fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self { code, fact: fact.into(), action: action.into() }
    }
    pub fn usage(fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self::new("usage", fact, action)
    }
    pub fn refused(fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self::new("refused", fact, action)
    }
    pub fn not_found(fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self::new("not_found", fact, action)
    }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::new("internal", format!("{e:#}"), "This is an engine failure; keep the output and report it.")
    }
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.fact)
    }
}

impl From<anyhow::Error> for TreeError {
    fn from(e: anyhow::Error) -> Self {
        TreeError::internal(e)
    }
}

pub type TreeResult<T> = Result<T, TreeError>;

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_parse_and_spec_round_trip() {
        let o = Owner::parse("session:abc123").unwrap();
        assert_eq!(o.kind, OwnerKind::Session);
        assert_eq!(o.spec(), "session:abc123");
        assert!(Owner::parse("robot:x").is_none());
        assert!(Owner::parse("bot:").is_none());
    }

    #[test]
    fn tree_ids_are_url_safe() {
        assert_eq!(tree_id("allternit", "session/agent-trees"), "allternit~session-agent-trees");
        assert_eq!(tree_id("allternit-ai", "feat/x y"), "allternit-ai~feat-x-y");
    }
}
