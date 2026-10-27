//! Thin `git` helpers for trees (blocking; HTTP calls run them on
//! `spawn_blocking`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

pub fn cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir);
    c
}

/// Run git in `dir`; stdout trimmed, or the error with git's stderr.
pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = cmd(dir).args(args).output().with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub fn ok(dir: &Path, args: &[&str]) -> bool {
    cmd(dir).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

/// The main checkout of the repo `dir` belongs to (parent of the common git dir).
pub fn main_checkout(dir: &Path) -> Result<PathBuf> {
    let common = run(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = PathBuf::from(common);
    match common.file_name().and_then(|n| n.to_str()) {
        Some(".git") => Ok(common.parent().map(Path::to_path_buf).unwrap_or(common)),
        _ => bail!("{} is not inside a non-bare git checkout", dir.display()),
    }
}

pub fn toplevel(dir: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(run(dir, &["rev-parse", "--show-toplevel"])?))
}

pub fn repo_name(main: &Path) -> String {
    main.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into())
}

/// `git fetch origin main`, quietly. Offline is not fatal: the caller decides.
pub fn fetch_main(dir: &Path) -> Result<()> {
    run(dir, &["fetch", "--quiet", "origin", "main"]).map(|_| ())
}

pub fn rev(dir: &Path, rev: &str) -> Result<String> {
    run(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
}

pub fn current_branch(dir: &Path) -> Option<String> {
    run(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok().filter(|b| !b.is_empty())
}

pub fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    ok(dir, &["merge-base", "--is-ancestor", a, b])
}

/// Files with uncommitted changes (tracked edits + untracked, not ignored).
pub fn dirty_files(dir: &Path) -> Result<Vec<String>> {
    let out = run(dir, &["status", "--porcelain", "--untracked-files=all"])?;
    Ok(out.lines().filter(|l| l.len() > 3).map(|l| l[3..].to_string()).collect())
}

/// `(behind, ahead)` of HEAD against `upstream`.
pub fn behind_ahead(dir: &Path, upstream: &str) -> Option<(u32, u32)> {
    let out = run(dir, &["rev-list", "--left-right", "--count", &format!("{upstream}...HEAD")]).ok()?;
    let mut it = out.split_whitespace().map(|n| n.parse::<u32>().ok());
    Some((it.next()??, it.next()??))
}

/// Commits on HEAD that no remote branch has (unpushed work).
pub fn unpushed(dir: &Path) -> u32 {
    run(dir, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// One `git worktree list --porcelain` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

pub fn worktrees(main: &Path) -> Result<Vec<Worktree>> {
    let out = run(main, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktrees(&out))
}

pub fn parse_worktrees(out: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut cur: Option<Worktree> = None;
    for line in out.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(w) = cur.take() {
                list.push(w);
            }
            continue;
        }
        if let Some(p) = line.strip_prefix("worktree ") {
            cur = Some(Worktree {
                path: PathBuf::from(p),
                head: None,
                branch: None,
                detached: false,
                locked: false,
                prunable: false,
            });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        if let Some(h) = line.strip_prefix("HEAD ") {
            w.head = Some(h.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            w.branch = Some(b.trim_start_matches("refs/heads/").to_string());
        } else if line == "detached" {
            w.detached = true;
        } else if line.starts_with("locked") {
            w.locked = true;
        } else if line.starts_with("prunable") {
            w.prunable = true;
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_worktree_list() {
        let out = "worktree /r/a\nHEAD 111\nbranch refs/heads/main\n\nworktree /r/b\nHEAD 222\ndetached\nlocked reason\n\nworktree /r/c\nHEAD 333\nbranch refs/heads/feat/x\nprunable gitdir file points to non-existent location\n";
        let w = parse_worktrees(out);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert!(w[1].detached && w[1].locked);
        assert_eq!(w[2].branch.as_deref(), Some("feat/x"));
        assert!(w[2].prunable);
    }
}
