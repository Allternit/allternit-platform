//! `reap`: the merged + idle + clean rule over every worktree of every known
//! repo. A dry run by default.
//!
//! * merged into origin/main, nothing uncommitted or unpushed, no process in
//!   it and no open file under it → removed (as `done` would).
//! * open branch, idle (no process, no source file touched for
//!   `idle_hours`) → build output deleted, the tree kept.
//! * dev servers running longer than `dev_server_hours` → stopped.
//! * paused, detached build trees, the shared checkout and the base → kept
//!   and listed, never removed.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use super::config::TreesConfig;
use super::land::{self, DoneRequest};
use super::lease::{canonical, Lease, LeaseStore};
use super::{git, now, ops, procs, seed, Owner, OwnerKind, TreeResult, TreeState};

#[derive(Clone, Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReapReport {
    pub applied: bool,
    pub removed: Vec<Reaped>,
    pub cleaned: Vec<Reaped>,
    pub stopped: Vec<Stopped>,
    pub kept: Vec<Reaped>,
    pub free_bytes_before: Option<u64>,
    pub free_bytes_after: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reaped {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub reason: String,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub path: PathBuf,
    pub pid: u32,
    pub command: String,
    pub age_hours: u64,
}

/// Newer than `cutoff`? Walks source only (no .git, no build output) and
/// stops at the first recent file.
fn touched_since(path: &Path, cfg: &TreesConfig, cutoff: SystemTime) -> bool {
    let mut it = walkdir::WalkDir::new(path).min_depth(1).into_iter();
    while let Some(Ok(e)) = it.next() {
        let name = e.file_name().to_string_lossy();
        if e.file_type().is_dir() && (name == ".git" || cfg.clean.build_dirs.iter().any(|b| b == name.as_ref())) {
            it.skip_current_dir();
            continue;
        }
        if e.metadata().ok().and_then(|m| m.modified().ok()).is_some_and(|t| t > cutoff) {
            return true;
        }
    }
    false
}

pub struct ReapRequest {
    pub repos: Vec<PathBuf>,
    pub apply: bool,
    pub with_size: bool,
}

pub fn reap(store: &LeaseStore, req: ReapRequest) -> TreeResult<ReapReport> {
    let leases = store.list()?;
    let mut repos: Vec<PathBuf> = leases.iter().map(|l| canonical(&l.repo_path)).collect();
    repos.extend(req.repos.iter().filter_map(|r| git::main_checkout(r).ok()).map(|p| canonical(&p)));
    repos.sort();
    repos.dedup();
    let mut report = ReapReport { applied: req.apply, ..Default::default() };
    report.free_bytes_before = repos.first().and_then(|r| ops::free_bytes(r));
    for main in &repos {
        reap_repo(store, main, &leases, &req, &mut report)?;
    }
    // Leases whose folder is gone (removed by hand) are dropped.
    if req.apply {
        for l in store.list()?.into_iter().filter(|l| !l.path.exists()) {
            store.remove(&l.id).ok();
        }
    }
    report.free_bytes_after = repos.first().and_then(|r| ops::free_bytes(r));
    Ok(report)
}

fn reap_repo(store: &LeaseStore, main: &Path, leases: &[Lease], req: &ReapRequest, report: &mut ReapReport) -> TreeResult<()> {
    let cfg = TreesConfig::load(main).unwrap_or_default();
    if req.apply {
        git::run(main, &["worktree", "prune"]).ok();
    }
    git::fetch_main(main).ok();
    let base = canonical(&seed::base_path(main));
    let idle_cutoff = SystemTime::now() - Duration::from_secs(cfg.reaper.idle_hours * 3600);
    let now_ts = chrono::Utc::now();
    let cwds = procs::cwds();
    for w in git::worktrees(main)? {
        let path = canonical(&w.path);
        if path == canonical(main) || path == base || w.prunable || !path.exists() {
            continue;
        }
        let lease = leases.iter().find(|l| canonical(&l.path) == path).cloned();
        let lease_id = lease.as_ref().map(|l| l.id.clone());
        let size = || {
            let bytes = if req.with_size { ops::size_bytes(&path) } else { None };
            if let (Some(b), Some(id)) = (bytes, lease_id.as_deref()) {
                let _ = store.modify(id, |l| {
                    l.size_bytes = Some(b);
                    l.size_at = Some(now());
                });
            }
            bytes
        };
        let entry = |reason: String, size_bytes: Option<u64>| Reaped { path: path.clone(), branch: w.branch.clone(), reason, size_bytes };

        // Dev servers past their time stop first, whatever the tree's state.
        let mut running = procs::in_tree_from(&cwds, &path, |c| cfg.is_dev_server(c));
        let stale: Vec<procs::TreeProcess> = running
            .iter()
            .filter(|p| p.dev_server && !p.protected && p.age_secs.unwrap_or(0) > cfg.reaper.dev_server_hours * 3600)
            .cloned()
            .collect();
        if !stale.is_empty() {
            if req.apply {
                procs::stop(&stale, Duration::from_secs(20));
            }
            for p in &stale {
                report.stopped.push(Stopped {
                    path: path.clone(),
                    pid: p.pid,
                    command: p.command.clone(),
                    age_hours: p.age_secs.unwrap_or(0) / 3600,
                });
            }
            let gone: Vec<u32> = stale.iter().map(|p| p.pid).collect();
            running.retain(|p| !gone.contains(&p.pid));
        }

        if lease.as_ref().is_some_and(|l| l.state == TreeState::Paused) {
            report.kept.push(entry("paused: kept for a resume".into(), size()));
            continue;
        }
        let is_once = lease.as_ref().is_some_and(|l| l.once);
        if w.detached && !is_once {
            report.kept.push(entry("detached build tree: listed, never removed".into(), size()));
            continue;
        }
        if w.locked {
            report.kept.push(entry("locked worktree".into(), size()));
            continue;
        }
        if let Some(p) = running.first() {
            report.kept.push(entry(format!("running: {} (pid {})", short(&p.command), p.pid), size()));
            continue;
        }
        let dirty = git::dirty_files(&path).map(|d| d.len()).unwrap_or(usize::MAX);
        let unpushed = git::unpushed(&path);
        let merged = land::merged(&path, w.branch.as_deref());
        let expired = lease
            .as_ref()
            .and_then(|l| l.expires_at.as_deref())
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
            .is_none_or(|e| e < now_ts);
        // A new tree has no commits of its own, so it looks "merged"; and a
        // cloned one keeps old mtimes. Its `.git` file is as old as the tree.
        let new_tree = std::fs::metadata(path.join(".git"))
            .and_then(|m| m.modified())
            .is_ok_and(|t| t > idle_cutoff);
        let idle = expired && !new_tree && !touched_since(&path, &cfg, idle_cutoff);
        if merged && dirty == 0 && unpushed == 0 && !idle {
            report.kept.push(entry("merged and clean, but in use or new (removed once idle)".into(), size()));
            continue;
        }
        if merged && dirty == 0 && unpushed == 0 {
            if procs::has_open_files(&path) {
                report.kept.push(entry("merged, but a file in it is open".into(), size()));
                continue;
            }
            let sz = size();
            if req.apply {
                let lease = lease.clone().unwrap_or_else(|| synthetic_lease(main, &path, w.branch.clone()));
                if let Err(e) = land::done(store, &lease, DoneRequest { abandon: false, skip_discipline: true }) {
                    report.kept.push(entry(format!("merged, but landing failed: {}", e.fact), sz));
                    continue;
                }
            }
            report.removed.push(entry("merged into origin/main, clean, idle".into(), sz));
            continue;
        }
        if idle {
            let dirs = land::build_dirs(&path, &cfg);
            if dirs.is_empty() {
                report.kept.push(entry(open_reason(dirty, unpushed, merged, "idle, no build output"), size()));
                continue;
            }
            let sz = size();
            let mut what: Vec<String> = dirs.iter().map(|d| d.strip_prefix(&path).unwrap_or(d).to_string_lossy().into_owned()).collect();
            if req.apply {
                what = land::remove_build_output(&path, &cfg);
            }
            report.cleaned.push(entry(
                format!("{}; build output deleted: {}", open_reason(dirty, unpushed, merged, "idle"), what.join(", ")),
                sz,
            ));
            continue;
        }
        report.kept.push(entry(open_reason(dirty, unpushed, merged, "in use"), size()));
    }
    Ok(())
}

fn open_reason(dirty: usize, unpushed: u32, merged: bool, tail: &str) -> String {
    let mut parts = Vec::new();
    if !merged {
        parts.push("not merged".to_string());
    }
    if dirty > 0 && dirty != usize::MAX {
        parts.push(format!("{dirty} uncommitted"));
    }
    if unpushed > 0 {
        parts.push(format!("{unpushed} unpushed"));
    }
    parts.push(tail.to_string());
    parts.join(", ")
}

fn short(cmd: &str) -> String {
    let s: String = cmd.chars().take(60).collect();
    if cmd.chars().count() > 60 { format!("{s}…") } else { s }
}

/// A lease for an un-leased worktree, so `done` can land it.
fn synthetic_lease(main: &Path, path: &Path, branch: Option<String>) -> Lease {
    let dir = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let t = now();
    Lease {
        id: super::tree_id(&git::repo_name(main), branch.as_deref().unwrap_or(&dir)),
        repo: git::repo_name(main),
        repo_path: main.to_path_buf(),
        path: path.to_path_buf(),
        branch,
        base: String::new(),
        owner: Owner { kind: OwnerKind::Human, id: "reaper".into(), label: "reaper".into() },
        state: TreeState::Done,
        created_at: t.clone(),
        updated_at: t,
        expires_at: None,
        seed: None,
        once: false,
        size_bytes: None,
        size_at: None,
        checkpoints: 0,
        last_checkpoint_at: None,
        last_checkpoint_tree: None,
    }
}

/// Install the scheduled reaper (launchd on macOS, a systemd user timer on
/// Linux), running `<engine> agents tree reap --apply --size` every `hours`
/// (the size pass keeps each lease's disk use current for the Tree card).
pub fn schedule(engine: &Path, root: &Path, hours: u32, install: bool) -> anyhow::Result<String> {
    let label = "com.allternit.agent-trees-reaper";
    if cfg!(target_os = "macos") {
        let plist = crate::platform::user_home_or("/tmp").join(format!("Library/LaunchAgents/{label}.plist"));
        let uid = unsafe { libc::getuid() };
        let _ = std::process::Command::new("launchctl").args(["bootout", &format!("gui/{uid}/{label}")]).status();
        if !install {
            let _ = std::fs::remove_file(&plist);
            return Ok(format!("removed {}", plist.display()));
        }
        let log = crate::agents::registry::factory_home().join("trees/reaper.log");
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key><array>
    <string>{engine}</string><string>--root</string><string>{root}</string>
    <string>agents</string><string>tree</string><string>reap</string><string>--apply</string><string>--size</string>
  </array>
  <key>StartInterval</key><integer>{secs}</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
</dict></plist>
"#,
            engine = engine.display(),
            root = root.display(),
            secs = hours * 3600,
            log = log.display(),
        );
        std::fs::create_dir_all(plist.parent().unwrap())?;
        std::fs::write(&plist, body)?;
        let st = std::process::Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}"), &plist.to_string_lossy()])
            .status()?;
        anyhow::ensure!(st.success(), "launchctl bootstrap failed for {}", plist.display());
        return Ok(format!("installed {} (every {hours} h)", plist.display()));
    }
    if cfg!(target_os = "linux") {
        let dir = crate::platform::user_home_or("/tmp").join(".config/systemd/user");
        let svc = dir.join("allternit-agent-trees-reaper.service");
        let timer = dir.join("allternit-agent-trees-reaper.timer");
        if !install {
            let _ = std::process::Command::new("systemctl").args(["--user", "disable", "--now", "allternit-agent-trees-reaper.timer"]).status();
            let _ = std::fs::remove_file(&svc);
            let _ = std::fs::remove_file(&timer);
            return Ok("removed the systemd user timer".into());
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            &svc,
            format!(
                "[Unit]\nDescription=Allternit Agent Trees reaper\n\n[Service]\nType=oneshot\nNice=10\nExecStart={} --root {} agents tree reap --apply --size\n",
                engine.display(),
                root.display()
            ),
        )?;
        std::fs::write(
            &timer,
            format!("[Unit]\nDescription=Run the Agent Trees reaper\n\n[Timer]\nOnBootSec=15min\nOnUnitActiveSec={hours}h\n\n[Install]\nWantedBy=timers.target\n"),
        )?;
        let st = std::process::Command::new("systemctl").args(["--user", "enable", "--now", "allternit-agent-trees-reaper.timer"]).status()?;
        anyhow::ensure!(st.success(), "systemctl --user enable failed");
        return Ok(format!("installed {} (every {hours} h)", timer.display()));
    }
    anyhow::bail!("scheduling the reaper is supported on macOS and Linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_read_plainly() {
        assert_eq!(open_reason(2, 1, false, "idle"), "not merged, 2 uncommitted, 1 unpushed, idle");
        assert_eq!(open_reason(0, 0, true, "in use"), "in use");
    }
}
