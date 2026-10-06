//! Lease-holder heartbeats (S9) for the drive runner.
//!
//! A holder beats every ~60s: `<root>/.allternit/leases/heartbeats/<wih_id>.json`
//! = `{wih_id, agent_id, pid, host, beat_at}`. Beats are a derived file, not
//! ledger events (one per minute per holder would swamp the ledger); the
//! ledger gets `LeaseHolderHeartbeat` only when the holder (pid/host/agent)
//! changes, and `LeaseReclaimed` / `WIHReclaimed` when a sweep reclaims.
//!
//! A holder is stale when its last beat is older than `stale_after`, or its
//! pid is on this host and no longer alive. Leases whose WIH never beat are
//! left alone unless the sweep opts in (`include_unbeaten`), so flows that
//! predate heartbeats are not reclaimed by surprise.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, write_json_atomic};

pub const HEARTBEAT_DIR: &str = ".allternit/leases/heartbeats";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    pub wih_id: String,
    pub agent_id: Option<String>,
    pub pid: Option<u32>,
    pub host: Option<String>,
    pub beat_at: String,
}

impl Heartbeat {
    pub fn same_holder(&self, other: &Heartbeat) -> bool {
        self.agent_id == other.agent_id && self.pid == other.pid && self.host == other.host
    }
}

pub fn heartbeat_path(root: &Path, wih_id: &str) -> PathBuf {
    root.join(HEARTBEAT_DIR).join(format!("{wih_id}.json"))
}

pub fn read_heartbeat(root: &Path, wih_id: &str) -> Option<Heartbeat> {
    let text = std::fs::read_to_string(heartbeat_path(root, wih_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write a beat; returns the previous one (if any).
pub fn write_heartbeat(root: &Path, beat: &Heartbeat) -> Result<Option<Heartbeat>> {
    let prev = read_heartbeat(root, &beat.wih_id);
    let path = heartbeat_path(root, &beat.wih_id);
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    write_json_atomic(&path, beat)?;
    Ok(prev)
}

pub fn remove_heartbeat(root: &Path, wih_id: &str) {
    let _ = std::fs::remove_file(heartbeat_path(root, wih_id));
}

/// WIH ids with a heartbeat file.
pub fn beating_wihs(root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root.join(HEARTBEAT_DIR)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(|s| s.to_string())
        })
        .collect();
    out.sort();
    out
}

/// This machine's host name (best effort).
pub fn this_host() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// `Some(false)` when `pid` is known dead, `Some(true)` alive, `None` unknown.
pub fn pid_alive(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        Some(status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// Why a holder is stale, or `None` when it is live.
pub fn staleness(
    beat: &Heartbeat,
    now: DateTime<Utc>,
    stale_after: Duration,
    host: &str,
) -> Option<String> {
    if let (Some(pid), Some(h)) = (beat.pid, beat.host.as_deref()) {
        if h == host && pid_alive(pid) == Some(false) {
            return Some(format!("pid {pid} on {h} is not running"));
        }
    }
    match beat.beat_at.parse::<DateTime<Utc>>() {
        Ok(at) if now - at > stale_after => Some(format!(
            "last heartbeat {} is older than {}s",
            beat.beat_at,
            stale_after.num_seconds()
        )),
        Ok(_) => None,
        Err(_) => Some(format!("unparseable heartbeat time {:?}", beat.beat_at)),
    }
}

/// `90`, `90s`, `5m`, `1h`, `2d` → duration.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, "s"),
    };
    let n: i64 = num.parse().map_err(|_| anyhow!("invalid duration {s:?}"))?;
    let secs = match unit {
        "s" | "sec" | "secs" => n,
        "m" | "min" | "mins" => n * 60,
        "h" | "hr" | "hrs" => n * 3600,
        "d" => n * 86400,
        _ => return Err(anyhow!("invalid duration unit in {s:?} (use s, m, h, d)")),
    };
    if secs <= 0 {
        return Err(anyhow!("duration must be positive: {s:?}"));
    }
    Ok(Duration::seconds(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90").unwrap().num_seconds(), 90);
        assert_eq!(parse_duration("5m").unwrap().num_seconds(), 300);
        assert_eq!(parse_duration("1h").unwrap().num_seconds(), 3600);
        assert!(parse_duration("5y").is_err());
        assert!(parse_duration("0").is_err());
    }

    #[test]
    fn stale_by_age() {
        let now = Utc::now();
        let beat = Heartbeat {
            wih_id: "w".into(),
            agent_id: None,
            pid: None,
            host: None,
            beat_at: (now - Duration::seconds(400)).to_rfc3339(),
        };
        assert!(staleness(&beat, now, Duration::seconds(300), "h").is_some());
        assert!(staleness(&beat, now, Duration::seconds(600), "h").is_none());
    }
}
