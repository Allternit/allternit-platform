//! Processes working inside a tree: found by their current directory, so
//! nothing has to register. `stop` never touches the caller's own process
//! chain or an agent CLI (the agent landing its tree runs from inside it).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TreeProcess {
    pub pid: u32,
    pub command: String,
    pub dev_server: bool,
    /// Seconds since it started, when `ps` could tell.
    pub age_secs: Option<u64>,
    /// An agent CLI (claude, codex, kimi, gizzi) or the caller itself: listed, never stopped.
    pub protected: bool,
}

/// Agent CLIs that may sit in a tree as its owner; never signalled.
const HARNESSES: [&str; 6] = ["claude", "codex", "kimi", "gizzi", "gizzi-code", "agy"];

/// pid → cwd for every process we can see.
pub fn cwds() -> BTreeMap<u32, PathBuf> {
    let mut map = BTreeMap::new();
    #[cfg(target_os = "linux")]
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for e in dir.flatten() {
            if let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() {
                if let Ok(cwd) = std::fs::read_link(e.path().join("cwd")) {
                    map.insert(pid, cwd);
                }
            }
        }
        return map;
    }
    #[cfg(unix)]
    if let Ok(out) = Command::new("lsof").args(["-n", "-P", "-a", "-d", "cwd", "-F", "pn"]).output() {
        let mut pid = None;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(p) = line.strip_prefix('p') {
                pid = p.parse::<u32>().ok();
            } else if let (Some(n), Some(p)) = (line.strip_prefix('n'), pid) {
                map.insert(p, PathBuf::from(n));
            }
        }
    }
    map
}

/// `(age seconds, command)` for each pid.
fn describe(pids: &[u32]) -> BTreeMap<u32, (Option<u64>, String)> {
    let mut map = BTreeMap::new();
    if pids.is_empty() {
        return map;
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    if let Ok(out) = Command::new("ps").args(["-o", "pid=,etime=,command=", "-p", &list]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            let (Some(pid), Some(etime)) = (it.next().and_then(|p| p.parse::<u32>().ok()), it.next()) else { continue };
            map.insert(pid, (parse_etime(etime), it.collect::<Vec<_>>().join(" ")));
        }
    }
    map
}

/// `ps` etime: `[[dd-]hh:]mm:ss`.
pub fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

/// This process and every ancestor (the shell and agent that called us).
fn own_chain() -> BTreeSet<u32> {
    let mut chain = BTreeSet::new();
    let mut pid = std::process::id();
    for _ in 0..64 {
        chain.insert(pid);
        let ppid = Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok());
        match ppid {
            Some(p) if p > 1 && !chain.contains(&p) => pid = p,
            _ => break,
        }
    }
    chain
}

fn is_harness(command: &str) -> bool {
    let exe = command.split_whitespace().next().unwrap_or("");
    let name = exe.rsplit('/').next().unwrap_or(exe);
    HARNESSES.contains(&name)
        || command.contains("/@anthropic-ai/claude-code/")
        || command.contains("gizzi-code serve")
}

/// Processes whose working directory is inside `tree`.
pub fn in_tree(tree: &Path, is_dev_server: impl Fn(&str) -> bool) -> Vec<TreeProcess> {
    in_tree_from(&cwds(), tree, is_dev_server)
}

pub fn in_tree_from(cwds: &BTreeMap<u32, PathBuf>, tree: &Path, is_dev_server: impl Fn(&str) -> bool) -> Vec<TreeProcess> {
    let tree = super::lease::canonical(tree);
    let pids: Vec<u32> = cwds.iter().filter(|(_, cwd)| cwd.starts_with(&tree)).map(|(p, _)| *p).collect();
    let info = describe(&pids);
    let chain = own_chain();
    pids.into_iter()
        .filter_map(|pid| {
            let (age, command) = info.get(&pid).cloned()?;
            Some(TreeProcess {
                pid,
                dev_server: is_dev_server(&command),
                protected: chain.contains(&pid) || is_harness(&command),
                command,
                age_secs: age,
            })
        })
        .collect()
}

/// Whether any process holds a file open under `tree` (`lsof +D`; slow on a
/// big tree, so the reaper only asks for trees that are otherwise removable).
pub fn has_open_files(tree: &Path) -> bool {
    #[cfg(unix)]
    {
        let out = Command::new("lsof").args(["-n", "-P", "-t", "+D"]).arg(tree).output();
        match out {
            Ok(o) => {
                let chain = own_chain();
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.trim().parse::<u32>().ok())
                    .any(|p| !chain.contains(&p))
            }
            // Can't tell: treat as busy, never remove on a guess.
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tree;
        true
    }
}

/// TERM, wait up to `grace`, then KILL. Protected processes are skipped.
/// Returns the pids that were signalled.
pub fn stop(procs: &[TreeProcess], grace: Duration) -> Vec<u32> {
    let targets: Vec<u32> = procs.iter().filter(|p| !p.protected).map(|p| p.pid).collect();
    #[cfg(unix)]
    {
        for pid in &targets {
            // SAFETY: plain signal to a pid we just listed.
            unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
        }
        let start = Instant::now();
        while start.elapsed() < grace && targets.iter().any(|p| alive(*p)) {
            std::thread::sleep(Duration::from_millis(250));
        }
        for pid in targets.iter().filter(|p| alive(**p)) {
            // SAFETY: as above.
            unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
        }
    }
    #[cfg(not(unix))]
    let _ = grace;
    targets
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks the pid exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etime_formats() {
        assert_eq!(parse_etime("05:33"), Some(333));
        assert_eq!(parse_etime("01:00:00"), Some(3600));
        assert_eq!(parse_etime("3-02:00:01"), Some(3 * 86_400 + 7201));
        assert_eq!(parse_etime("x"), None);
    }

    #[test]
    fn harness_detection() {
        assert!(is_harness("/Users/joe/.local/bin/claude --resume"));
        assert!(is_harness("gizzi-code serve --port 1"));
        assert!(!is_harness("node /x/node_modules/.bin/vite"));
    }

    #[test]
    fn finds_this_process_as_protected() {
        let here = std::env::current_dir().unwrap();
        let mut map = BTreeMap::new();
        map.insert(std::process::id(), here.clone());
        let procs = in_tree_from(&map, &here, |_| false);
        assert_eq!(procs.len(), 1);
        assert!(procs[0].protected, "the caller is never stopped");
        assert!(stop(&procs, Duration::from_millis(10)).is_empty());
    }
}
