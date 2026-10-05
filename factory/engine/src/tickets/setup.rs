//! Agent/editor setup recipes for the Rails CLI.
//!
//! `rails setup` writes configuration files that teach Claude, Codex, Cursor,
//! and other agents how to use the Rails CLI in this workspace.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Supported agent/editor targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentTarget {
    Claude,
    Codex,
    Cursor,
    Windsurf,
    Aider,
}

impl std::fmt::Display for AgentTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentTarget::Claude => write!(f, "claude"),
            AgentTarget::Codex => write!(f, "codex"),
            AgentTarget::Cursor => write!(f, "cursor"),
            AgentTarget::Windsurf => write!(f, "windsurf"),
            AgentTarget::Aider => write!(f, "aider"),
        }
    }
}

/// A setup recipe that writes agent-specific configuration files.
pub struct SetupRecipe;

impl SetupRecipe {
    /// Apply the setup recipe for a target agent.
    pub fn apply(target: AgentTarget, workspace_root: &Path) -> Result<Vec<PathBuf>> {
        let files = match target {
            AgentTarget::Claude => Self::write_claude(workspace_root),
            AgentTarget::Codex => Self::write_codex(workspace_root),
            AgentTarget::Cursor => Self::write_cursor(workspace_root),
            AgentTarget::Windsurf => Self::write_windsurf(workspace_root),
            AgentTarget::Aider => Self::write_aider(workspace_root),
        }?;
        Ok(files)
    }

    fn write_claude(root: &Path) -> Result<Vec<PathBuf>> {
        let dir = root.join(".claude");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("AGENTS.md");
        let content = r#"# Rails Agent Instructions

This workspace uses the `rails` CLI for ticket and DAG workflow management.

## Commands you may use

- `rails init` — initialize Rails state in the current workspace.
- `rails ticket new <title> --description "..." --priority p1` — create a ticket.
- `rails ticket list` — list open tickets.
- `rails ticket show <id>` — show ticket details.
- `rails dag block <ticket> <blocker>` — add a blocking dependency.
- `rails ready --explain` — show what work is unblocked.
- `rails doctor` — verify workspace integrity.
- `rails memory learn <content> --tags <tag>` — remember project context.
- `rails brief --tags <tag>` — recall relevant context.
- `rails echo new <content> --kind heartbeat` — emit an ephemeral signal.
- `rails template new <name> --steps <steps.json>` — create a workflow template.
- `rails template instantiate <id>` — create tickets from a template.
- `rails batch exec <batch.json>` — execute atomic operations.
- `rails gate add <ticket> manual --description "..."` — add a wait-gate.
- `rails lock acquire <domain> --owner <agent>` — acquire a merge lock.

## Rules

1. Prefer `rails` over manual `.allternit/rails/` file edits.
2. Ticket IDs are hash-based (`T-` prefix, 32 hex chars).
3. Do not create blocking dependency cycles.
4. Run `rails doctor` before and after non-trivial changes.
"#;
        std::fs::write(&path, content)?;
        Ok(vec![path])
    }

    fn write_codex(root: &Path) -> Result<Vec<PathBuf>> {
        let dir = root.join(".codex");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("instructions.md");
        let content = r#"# Codex Instructions

Use the `rails` CLI for all ticket/dependency work in this repository.

- Create tickets: `rails ticket new "title" --description "..."`
- Link work: `rails dag block <ticket> <blocker>`
- Check status: `rails ready --explain`
- Remember context: `rails memory learn "..." --tags api`
- Recall context: `rails brief --tags api`

Always run `rails doctor` after modifying tickets or dependencies.
"#;
        std::fs::write(&path, content)?;
        Ok(vec![path])
    }

    fn write_cursor(root: &Path) -> Result<Vec<PathBuf>> {
        let path = root.join(".cursorrules");
        let content = r#"# Cursor Rules for Rails

- Use `rails` CLI for ticket/dependency operations.
- Ticket IDs are hash-based (`T-...`).
- Run `rails ready --explain` before starting work.
- Run `rails doctor` after changes.
- Learn important context with `rails memory learn`.
"#;
        std::fs::write(&path, content)?;
        Ok(vec![path])
    }

    fn write_windsurf(root: &Path) -> Result<Vec<PathBuf>> {
        let dir = root.join(".windsurf");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("rules.md");
        let content = r#"# Windsurf Rules

Use the `rails` CLI for ticket and DAG workflows in this workspace.
"#;
        std::fs::write(&path, content)?;
        Ok(vec![path])
    }

    fn write_aider(root: &Path) -> Result<Vec<PathBuf>> {
        let path = root.join(".aider.conf.yml");
        let content = r#"# Aider configuration for Rails
# This project uses the `rails` CLI for ticket/dependency tracking.
# Run `rails ready --explain` before starting a task.
"#;
        std::fs::write(&path, content)?;
        Ok(vec![path])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn claude_setup_writes_file() {
        let tmp = TempDir::new().unwrap();
        let files = SetupRecipe::apply(AgentTarget::Claude, tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].exists());
    }

    #[test]
    fn cursor_setup_writes_file() {
        let tmp = TempDir::new().unwrap();
        let files = SetupRecipe::apply(AgentTarget::Cursor, tmp.path()).unwrap();
        assert!(files[0].exists());
    }
}
