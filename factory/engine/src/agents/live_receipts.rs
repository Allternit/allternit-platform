//! Receipts for live-session messages: what was pushed, what the receiver
//! confirmed, what must be pushed again, and what was already seen.
//!
//! Claude Code's messaging socket has no reply, so a write is only `pushed`.
//! The receiver confirms (`ack`) when its hook sees the message id; until then
//! the message stays in `<factory home>/live-pending/` and is pushed again with
//! backoff, and again as soon as the session comes back online. The receiver
//! also keeps a seen-log keyed by hop id (7-day TTL) so a re-push never shows
//! the same message twice.
//!
//! Files (all under the factory home):
//! - `live-pending/<hop>.json`   unconfirmed push (envelope, attempts, next try)
//! - `live-seen.ndjson`          `{id, at}` per hop id the receiver has taken
//! - `live-messaging/<pid>.json` the Claude session's socket + token, written by `gizzi team-mcp`

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Receiver-side dedupe memory.
pub const SEEN_TTL_SECS: i64 = 7 * 24 * 3600;
/// Seconds to wait before push attempt n+1 (index = attempts so far - 1).
const BACKOFF_SECS: [i64; 5] = [5, 15, 45, 120, 300];
/// Give up re-pushing after this many tries; the inbox copy stays.
pub const MAX_ATTEMPTS: u32 = BACKOFF_SECS.len() as u32 + 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub hop_id: String,
    pub session: String,
    pub from: String,
    pub envelope: String,
    pub attempts: u32,
    /// Unix seconds.
    pub next_at: i64,
    pub first_at: i64,
}

fn home() -> PathBuf {
    super::registry::factory_home()
}
fn pending_dir() -> PathBuf {
    home().join("live-pending")
}
fn seen_path() -> PathBuf {
    home().join("live-seen.ndjson")
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

// ─── Pending pushes ─────────────────────────────────────────────────────────

/// Seconds until the next push after `attempts` tries; `None` when out of tries.
pub fn backoff_after(attempts: u32) -> Option<i64> {
    BACKOFF_SECS.get((attempts as usize).saturating_sub(1)).copied()
}

/// Remember a push that is not confirmed yet.
pub fn record_push(hop_id: &str, session: &str, from: &str, envelope: &str) {
    let _ = std::fs::create_dir_all(pending_dir());
    let path = pending_dir().join(format!("{}.json", safe(hop_id)));
    let prior: Option<Pending> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let attempts = prior.as_ref().map_or(1, |p| p.attempts + 1);
    let p = Pending {
        hop_id: hop_id.into(),
        session: session.into(),
        from: from.into(),
        envelope: envelope.into(),
        attempts,
        next_at: now() + backoff_after(attempts).unwrap_or(i64::MAX / 4),
        first_at: prior.map_or_else(now, |p| p.first_at),
    };
    let _ = std::fs::write(path, serde_json::to_vec(&p).unwrap_or_default());
}

/// The receiver confirmed `hop_id`: stop re-pushing. True when it was pending.
pub fn ack(hop_id: &str) -> bool {
    std::fs::remove_file(pending_dir().join(format!("{}.json", safe(hop_id)))).is_ok()
}

pub fn pending() -> Vec<Pending> {
    std::fs::read_dir(pending_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect()
}

/// Pending pushes whose time has come, or all of a session's when it just came back.
pub fn due(session_back: Option<&str>) -> Vec<Pending> {
    let t = now();
    pending()
        .into_iter()
        .filter(|p| p.attempts < MAX_ATTEMPTS && (p.next_at <= t || session_back.is_some_and(|s| s == p.session)))
        .collect()
}

/// Re-push everything due; called by the background sweeper.
pub async fn sweep() {
    for p in due(None) {
        let Some(s) = super::live_delivery::lookup().and_then(|l| l.list().into_iter().find(|s| s.name == p.session)) else { continue };
        if s.status == "offline" {
            continue;
        }
        super::live_delivery::repush(&s, &p).await;
    }
}

/// Start the once-a-5-seconds sweeper (idempotent; needs a tokio runtime).
pub fn ensure_sweeper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_ok() {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                sweep().await;
            }
        });
    }
}

// ─── Receiver-side dedupe ───────────────────────────────────────────────────

fn read_seen(at: i64) -> Vec<(String, i64)> {
    std::fs::read_to_string(seen_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v["id"].as_str()?.to_string(), v["at"].as_i64()?)))
        .filter(|(_, t)| at - t < SEEN_TTL_SECS)
        .collect()
}

/// True when `hop_id` was taken before (within the TTL). Otherwise records it and returns false.
pub fn seen_before(hop_id: &str) -> bool {
    let t = now();
    let mut all = read_seen(t);
    if all.iter().any(|(id, _)| id == hop_id) {
        return true;
    }
    all.push((hop_id.to_string(), t));
    let body: String = all.iter().map(|(id, at)| format!("{}\n", json!({ "id": id, "at": at }))).collect();
    let _ = std::fs::create_dir_all(home());
    let _ = std::fs::write(seen_path(), body);
    false
}

// ─── Claude socket + token registered by gizzi team-mcp ─────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Messaging {
    #[serde(default)]
    pub socket: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub permission_mode: String,
}

/// What `gizzi team-mcp` registered for the Claude process `pid`, or for the
/// last run of `session_id` (kept after exit so a closed session can be woken).
pub fn messaging(pid: Option<u32>, session_id: &str) -> Option<Messaging> {
    let dir = home().join("live-messaging");
    if let Some(m) = pid.and_then(|p| std::fs::read(dir.join(format!("{p}.json"))).ok()).and_then(|b| serde_json::from_slice::<Messaging>(&b).ok()) {
        return Some(m);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Messaging>(&b).ok())
        .find(|m| !session_id.is_empty() && m.session_id == session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_stops() {
        assert_eq!(backoff_after(1), Some(5));
        assert_eq!(backoff_after(5), Some(300));
        assert_eq!(backoff_after(6), None);
    }

    #[test]
    fn safe_ids_have_no_path_characters() {
        assert_eq!(safe("../hop/1"), "---hop-1");
    }
}
