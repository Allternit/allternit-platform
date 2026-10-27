//! Checkpoints: the tree's full working state (tracked edits + untracked,
//! not ignored) saved as a commit under `refs/checkpoints/<tree id>/<n>`
//! without touching the real index, HEAD or files. Taken by the CLI hooks
//! after each edit; an unchanged tree is not saved twice.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use serde::Serialize;

use super::git;
use super::lease::{Lease, LeaseStore};

pub const REF_ROOT: &str = "refs/checkpoints";

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub n: u32,
    pub sha: String,
    pub at: String,
    pub tool: Option<String>,
    pub files: u32,
    pub summary: String,
}

/// `refs/checkpoints/<repo>/<slug>`: a tree id's `~` is not allowed in a ref name.
pub fn ref_dir(tree_id: &str) -> String {
    format!("{REF_ROOT}/{}", tree_id.replacen('~', "/", 1))
}

pub fn ref_name(tree_id: &str, n: u32) -> String {
    format!("{}/{n}", ref_dir(tree_id))
}

/// Write the working tree to a tree object through a throwaway index.
fn snapshot_tree(dir: &Path) -> Result<String> {
    let tmp = tempfile::NamedTempFile::new()?;
    let index = tmp.path().to_path_buf();
    let env = |c: &mut Command| {
        c.env("GIT_INDEX_FILE", &index);
    };
    let mut read = git::cmd(dir);
    env(&mut read);
    // Start from HEAD's index so unchanged files cost nothing to stat.
    let st = read.args(["read-tree", "HEAD"]).output()?;
    if !st.status.success() {
        bail!("git read-tree HEAD: {}", String::from_utf8_lossy(&st.stderr).trim());
    }
    let mut add = git::cmd(dir);
    env(&mut add);
    let st = add.args(["add", "-A", "."]).output()?;
    if !st.status.success() {
        bail!("git add -A: {}", String::from_utf8_lossy(&st.stderr).trim());
    }
    let mut write = git::cmd(dir);
    env(&mut write);
    let out = write.args(["write-tree"]).output()?;
    if !out.status.success() {
        bail!("git write-tree: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn changed_count(dir: &Path, from: &str, to: &str) -> u32 {
    git::run(dir, &["diff", "--name-only", from, to]).map(|o| o.lines().count() as u32).unwrap_or(0)
}

/// Save a checkpoint if the tree changed since the last one. Returns it.
pub fn save(store: &LeaseStore, lease: &Lease, tool: Option<&str>) -> Result<Option<Checkpoint>> {
    let tree = snapshot_tree(&lease.path)?;
    if lease.last_checkpoint_tree.as_deref() == Some(tree.as_str()) {
        return Ok(None);
    }
    let head_tree = git::run(&lease.path, &["rev-parse", "HEAD^{tree}"])?;
    if lease.last_checkpoint_tree.is_none() && head_tree == tree {
        // Nothing to save yet: the tree matches its commit.
        return Ok(None);
    }
    let files = changed_count(&lease.path, "HEAD", &tree);
    let n = lease.checkpoints + 1;
    let at = super::now();
    let summary = summarize(&lease.path, &tree);
    let message = format!(
        "checkpoint {n}{}\n\nfiles: {files}\nat: {at}\n{summary}",
        tool.map(|t| format!(" after {t}")).unwrap_or_default()
    );
    let sha = git::run(&lease.path, &["commit-tree", &tree, "-p", "HEAD", "-m", &message])?;
    git::run(&lease.path, &["update-ref", &ref_name(&lease.id, n), &sha])?;
    store.modify(&lease.id, |l| {
        l.checkpoints = n;
        l.last_checkpoint_at = Some(at.clone());
        l.last_checkpoint_tree = Some(tree.clone());
    })?;
    Ok(Some(Checkpoint { n, sha, at, tool: tool.map(String::from), files, summary }))
}

/// The first few changed paths, for the list.
fn summarize(dir: &Path, tree: &str) -> String {
    let names = git::run(dir, &["diff", "--name-only", "HEAD", tree]).unwrap_or_default();
    let mut list: Vec<&str> = names.lines().take(3).collect();
    let more = names.lines().count().saturating_sub(list.len());
    if more > 0 {
        list.push("…");
    }
    let s = list.join(", ");
    if more > 0 { format!("{s} (+{more})") } else { s }
}

/// Every checkpoint of a tree, newest first.
pub fn list(lease: &Lease) -> Result<Vec<Checkpoint>> {
    let prefix = format!("{}/", ref_dir(&lease.id));
    let out = git::run(
        &lease.path,
        &["for-each-ref", "--format=%(refname)%09%(objectname)%09%(contents)", "--", &prefix],
    )
    .unwrap_or_default();
    let mut cps: Vec<Checkpoint> = Vec::new();
    // %(contents) spans lines; refs start each record with the prefix.
    let mut cur: Option<(u32, String, String)> = None;
    let flush = |cur: Option<(u32, String, String)>, cps: &mut Vec<Checkpoint>| {
        if let Some((n, sha, body)) = cur {
            cps.push(parse_message(n, sha, &body));
        }
    };
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix(&prefix) {
            let mut parts = rest.splitn(3, '\t');
            if let (Some(n), Some(sha)) = (parts.next().and_then(|n| n.parse::<u32>().ok()), parts.next()) {
                flush(cur.take(), &mut cps);
                cur = Some((n, sha.to_string(), parts.next().unwrap_or("").to_string()));
                continue;
            }
        }
        if let Some((_, _, body)) = cur.as_mut() {
            body.push('\n');
            body.push_str(line);
        }
    }
    flush(cur, &mut cps);
    cps.sort_by(|a, b| b.n.cmp(&a.n));
    Ok(cps)
}

fn parse_message(n: u32, sha: String, body: &str) -> Checkpoint {
    let mut lines = body.lines();
    let title = lines.next().unwrap_or("");
    let tool = title.split_once(" after ").map(|(_, t)| t.to_string());
    let mut files = 0;
    let mut at = String::new();
    let mut summary = String::new();
    for l in lines {
        if let Some(v) = l.strip_prefix("files: ") {
            files = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("at: ") {
            at = v.trim().to_string();
        } else if !l.trim().is_empty() && summary.is_empty() {
            summary = l.trim().to_string();
        }
    }
    Checkpoint { n, sha, at, tool, files, summary }
}

/// Put the tree's files back to checkpoint `n`, saving the current state
/// first so the rewind can itself be undone. Returns that new checkpoint's n.
pub fn rewind(store: &LeaseStore, lease: &Lease, n: u32) -> super::TreeResult<u32> {
    let target = git::rev(&lease.path, &ref_name(&lease.id, n)).map_err(|_| {
        super::TreeError::not_found(format!("{} has no checkpoint {n}", lease.id), "List them with `gizzi agents tree checkpoints`.")
    })?;
    let saved = match save(store, lease, Some("rewind"))? {
        Some(cp) => cp.n,
        None => lease.checkpoints,
    };
    // Files present now but not in the checkpoint (tracked or untracked, not ignored) go.
    let now_files = git::run(&lease.path, &["ls-files", "--cached", "--others", "--exclude-standard"])?;
    let want = git::run(&lease.path, &["ls-tree", "-r", "--name-only", &target])?;
    let want: std::collections::BTreeSet<&str> = want.lines().collect();
    for f in now_files.lines().filter(|f| !want.contains(f)) {
        let _ = std::fs::remove_file(lease.path.join(f));
    }
    git::run(&lease.path, &["checkout", &target, "--", "."])?;
    // Back to HEAD's index: the restored state shows as uncommitted changes.
    git::run(&lease.path, &["reset", "--quiet"])?;
    // The next save compares against the restored state.
    let tree = git::run(&lease.path, &["rev-parse", &format!("{target}^{{tree}}")])?;
    store.modify(&lease.id, |l| l.last_checkpoint_tree = Some(tree))?;
    Ok(saved)
}

/// Push every checkpoint ref of a tree (so another machine can resume).
pub fn push(lease: &Lease) -> Result<()> {
    let spec = format!("+{0}/*:{0}/*", ref_dir(&lease.id));
    git::run(&lease.path, &["push", "--quiet", "origin", &spec]).map(|_| ())
}

/// Delete a tree's checkpoint refs here and on origin.
pub fn delete_all(main: &Path, tree_id: &str, remote: bool) -> Result<()> {
    let prefix = format!("{}/", ref_dir(tree_id));
    let refs = git::run(main, &["for-each-ref", "--format=%(refname)", "--", &prefix]).unwrap_or_default();
    for r in refs.lines() {
        git::run(main, &["update-ref", "-d", r]).ok();
    }
    if remote {
        let remote_refs = git::run(main, &["ls-remote", "origin", &format!("{prefix}*")]).unwrap_or_default();
        let dels: Vec<String> = remote_refs
            .lines()
            .filter_map(|l| l.split('\t').nth(1))
            .map(|r| format!(":{r}"))
            .collect();
        if !dels.is_empty() {
            let mut args = vec!["push", "--quiet", "origin"];
            args.extend(dels.iter().map(String::as_str));
            git::run(main, &args).ok();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_names_are_valid_git_refs() {
        assert_eq!(ref_name("allternit~session-x", 3), "refs/checkpoints/allternit/session-x/3");
    }

    #[test]
    fn parses_checkpoint_message() {
        let cp = parse_message(3, "abc".into(), "checkpoint 3 after Edit\n\nfiles: 2\nat: 2026-10-27T00:00:00Z\nsrc/a.rs, src/b.rs");
        assert_eq!(cp.tool.as_deref(), Some("Edit"));
        assert_eq!(cp.files, 2);
        assert_eq!(cp.summary, "src/a.rs, src/b.rs");
        assert_eq!(cp.at, "2026-10-27T00:00:00Z");
    }
}
