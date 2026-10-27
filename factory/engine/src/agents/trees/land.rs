//! `done`: land a tree. Merge check → stop its processes → delete build
//! output → remove the worktree → delete the branch here and on origin and
//! its checkpoints → drop the lease → run the repo's
//! `scripts/git-discipline-check.sh`. The removal is never `--force` for a
//! landed tree. Work that is not on origin/main is never deleted:
//! `--abandon` first pushes all of it to a `salvage/` branch, and only then
//! may force the removal.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use super::config::TreesConfig;
use super::lease::{canonical, Lease, LeaseStore};
use super::{checkpoint, git, procs, seed, TreeError, TreeResult};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoneReport {
    pub id: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub merged: bool,
    /// `salvage/...` branch the work was pushed to (`--abandon`).
    pub salvaged_to: Option<String>,
    pub stopped: Vec<u32>,
    pub build_dirs_removed: Vec<String>,
    pub branch_deleted: bool,
    pub remote_branch_deleted: bool,
    /// Output of `scripts/git-discipline-check.sh`, when the repo has one.
    pub discipline: Option<Discipline>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discipline {
    pub passed: bool,
    pub output: String,
}

/// Build-output dirs under `path` (by name, any depth; not descending into them).
pub fn build_dirs(path: &Path, cfg: &TreesConfig) -> Vec<PathBuf> {
    let names = &cfg.clean.build_dirs;
    let mut found = Vec::new();
    let mut it = walkdir::WalkDir::new(path).min_depth(1).into_iter();
    while let Some(Ok(e)) = it.next() {
        if !e.file_type().is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy();
        if name == ".git" || name == ".allternit-tree" {
            it.skip_current_dir();
            continue;
        }
        if names.iter().any(|n| n == name.as_ref()) {
            // Only output git ignores: a tracked `release/` dir is source.
            let rel = e.path().strip_prefix(path).unwrap_or(e.path()).to_string_lossy().into_owned();
            if git::ok(path, &["check-ignore", "-q", &rel]) {
                found.push(e.path().to_path_buf());
            }
            it.skip_current_dir();
        }
    }
    found
}

/// Delete the build output of a tree. Returns what went.
pub fn remove_build_output(path: &Path, cfg: &TreesConfig) -> Vec<String> {
    build_dirs(path, cfg)
        .into_iter()
        .filter(|d| std::fs::remove_dir_all(d).is_ok())
        .map(|d| d.strip_prefix(path).unwrap_or(&d).to_string_lossy().into_owned())
        .collect()
}

/// Is the tree's work on origin/main? Merge commits make the branch tip an
/// ancestor; a squash merge is found through `gh` when it is installed.
pub fn merged(path: &Path, branch: Option<&str>) -> bool {
    if git::is_ancestor(path, "HEAD", "origin/main") {
        return true;
    }
    let Some(branch) = branch else { return false };
    Command::new("gh")
        .args(["pr", "list", "--head", branch, "--state", "merged", "--json", "number", "--jq", "length"])
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
        .is_some_and(|n| n > 0)
}

/// Push the tree's full state (commits + uncommitted, as one commit) to
/// `salvage/<branch or dir>-<date>`.
fn salvage(lease: &Lease, store: &LeaseStore) -> TreeResult<String> {
    let name = lease.branch.clone().unwrap_or_else(|| lease.id.replace('~', "-"));
    let target = format!("salvage/{}-{}", name.replace('/', "-"), chrono::Utc::now().format("%Y%m%d-%H%M"));
    let sha = match checkpoint::save(store, lease, Some("salvage"))? {
        Some(cp) => cp.sha,
        None => git::rev(&lease.path, "HEAD")?,
    };
    git::run(&lease.path, &["push", "--quiet", "origin", &format!("{sha}:refs/heads/{target}")]).map_err(|e| {
        TreeError::new("transport", format!("could not push the salvage branch: {e:#}"), "Fix the network and run it again; nothing was deleted.")
    })?;
    Ok(target)
}

pub struct DoneRequest {
    pub abandon: bool,
    /// Skip the discipline check (the reaper, many trees at once).
    pub skip_discipline: bool,
}

pub fn done(store: &LeaseStore, lease: &Lease, req: DoneRequest) -> TreeResult<DoneReport> {
    let main = lease.repo_path.clone();
    if canonical(&lease.path) == canonical(&main) || canonical(&lease.path) == canonical(&seed::base_path(&main)) {
        return Err(TreeError::refused("that is the shared checkout or the trees base", "Only agent trees are landed."));
    }
    let cfg = TreesConfig::load(&main).unwrap_or_default();
    let present = lease.path.exists();
    if present {
        git::fetch_main(&lease.path).ok();
    }
    let is_merged = !present || merged(&lease.path, lease.branch.as_deref());
    let dirty = if present { git::dirty_files(&lease.path)? } else { Vec::new() };
    let mut salvaged_to = None;
    if !is_merged || !dirty.is_empty() {
        if !req.abandon {
            let fact = if !is_merged {
                format!("{} is not on origin/main yet", lease.branch.as_deref().unwrap_or(&lease.id))
            } else {
                format!("{} has {} uncommitted file(s): {}", lease.id, dirty.len(), dirty.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
            };
            return Err(TreeError::refused(
                fact,
                "Merge its PR (and commit everything) first. To keep it for later: `gizzi agents tree pause`. To drop it: `--abandon` (pushes it to a salvage/ branch first).",
            ));
        }
        salvaged_to = Some(salvage(lease, store)?);
    }

    let mut stopped = Vec::new();
    let mut build_dirs_removed = Vec::new();
    if present {
        let running = procs::in_tree(&lease.path, |c| cfg.is_dev_server(c));
        stopped = procs::stop(&running, Duration::from_secs(20));
        build_dirs_removed = remove_build_output(&lease.path, &cfg);
        let path = lease.path.to_string_lossy().into_owned();
        // Our own scratch (env + data dir) is not work.
        let _ = std::fs::remove_dir_all(lease.path.join(".allternit-tree"));
        if let Err(e) = git::run(&main, &["worktree", "remove", &path]) {
            if !req.abandon {
                return Err(TreeError::refused(
                    format!("git would not remove {path}: {e:#}"),
                    "Something in it is still uncommitted or locked; look with `git status` there. Nothing was forced.",
                ));
            }
            git::run(&main, &["worktree", "remove", "--force", &path])?;
        }
    }
    git::run(&main, &["worktree", "prune"]).ok();

    let mut branch_deleted = false;
    let mut remote_branch_deleted = false;
    if let Some(b) = &lease.branch {
        let flag = if is_merged { "-d" } else { "-D" };
        // -d can refuse a squash-merged branch; it is on main (gh said so) or salvaged.
        branch_deleted = git::ok(&main, &["branch", flag, b]) || git::ok(&main, &["branch", "-D", b]);
        if git::run(&main, &["ls-remote", "--exit-code", "--heads", "origin", b]).is_ok() {
            remote_branch_deleted = git::ok(&main, &["push", "--quiet", "origin", "--delete", b]);
        }
    }
    checkpoint::delete_all(&main, &lease.id, true).ok();
    store.remove(&lease.id)?;

    let discipline = if req.skip_discipline { None } else { run_discipline(&main) };
    Ok(DoneReport {
        id: lease.id.clone(),
        path: lease.path.clone(),
        branch: lease.branch.clone(),
        merged: is_merged,
        salvaged_to,
        stopped,
        build_dirs_removed,
        branch_deleted,
        remote_branch_deleted,
        discipline,
    })
}

fn run_discipline(main: &Path) -> Option<Discipline> {
    let script = main.join("scripts/git-discipline-check.sh");
    if !script.exists() {
        return None;
    }
    let out = Command::new("bash").arg(&script).current_dir(main).output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(Discipline { passed: out.status.success(), output: text.trim_end().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_dirs_only_ignored_output() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert!(Command::new("git").args(["init", "-q"]).current_dir(p).status().unwrap().success());
        std::fs::write(p.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        for d in ["target/debug", "pkg/node_modules/x", "release", "src"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
        let found: Vec<String> = build_dirs(p, &TreesConfig::default())
            .into_iter()
            .map(|d| d.strip_prefix(p).unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(found.contains(&"target".to_string()));
        assert!(found.contains(&"pkg/node_modules".to_string()));
        assert!(!found.contains(&"release".to_string()), "a not-ignored release/ is source");
    }
}
