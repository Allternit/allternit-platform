//! `ao peer list|send` — Rails peer feed for the P5 visibility panel
//! (spec binding decision 4; memo Finding 3).
//!
//! The registry is the Rails `PeerRegistry` file at
//! `<root>/.allternit/peers/registry.json` (`rails/src/peer/mod.rs`), read
//! **read-only** through the rails crate's own `Peer`/`PeerStatus` types so
//! the ao side can never drift from the registry schema.
//!
//! Root resolution (documented in `docs/programs/ao/AO_VISIBILITY_PEERS_NOTES.md`):
//! **`--root <dir>` > `AO_PEERS_ROOT` env > the ao process's current working
//! directory.** The registry is local-only — sessions under different roots
//! are invisible to each other (`peer/mod.rs` doc comment) — so a wrong root
//! means an empty panel.
//!
//! Two deliberate deviations from `PeerRegistry::list()`, both to keep a UI
//! poll from mutating shared state:
//! - statuses are recomputed in memory (socket liveness dims Active/Idle to
//!   Dead) but **never written back** to `registry.json`;
//! - the "inbox socket missing ⇒ Dead" rule is applied for display only.
//!   HTTP-polling peers (gizzi) keep their placeholder-socket workaround
//!   (memo open question 3) without ao re-persisting anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use allternit_commrails::peer::{Peer, PeerStatus};
use allternit_commrails::peer::PeerEnvelope;

/// Env override for the registry root (flag > env > cwd).
pub const ROOT_ENV_VAR: &str = "AO_PEERS_ROOT";

/// Resolve the registry root: flag > env > process cwd.
pub fn registry_root(flag: Option<&str>) -> std::io::Result<PathBuf> {
    if let Some(dir) = flag {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var(ROOT_ENV_VAR) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    std::env::current_dir()
}

/// Registry file mirror (`peer/mod.rs` RegistryFile — private there, so the
/// shape is mirrored; `Peer` itself comes from the rails crate).
#[derive(Debug, Default, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    peers: HashMap<String, Peer>,
}

fn registry_path(root: &Path) -> PathBuf {
    root.join(".allternit").join("peers").join("registry.json")
}

/// Read all peers under `root`, recomputing socket liveness for display
/// without persisting (see module docs).
pub fn read_registry(root: &Path) -> std::io::Result<Vec<Peer>> {
    let path = registry_path(root);
    let Ok(bytes) = std::fs::read(&path) else {
        // No registry yet is not an error: the panel renders an empty feed.
        return Ok(Vec::new());
    };
    let file: RegistryFile = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let mut peers: Vec<Peer> = file.peers.into_values().collect();
    for peer in &mut peers {
        if peer.status != PeerStatus::Dead && !peer.inbox_socket.exists() {
            peer.status = PeerStatus::Dead;
        }
    }
    peers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(peers)
}

/// Resolve a peer by exact id, exact name, or unambiguous name prefix
/// (mirrors `PeerRegistry::find_key`).
pub fn resolve<'a>(peers: &'a [Peer], id_or_name: &str) -> Option<&'a Peer> {
    if let Some(p) = peers.iter().find(|p| p.peer_id == id_or_name) {
        return Some(p);
    }
    if let Some(p) = peers.iter().find(|p| p.name == id_or_name) {
        return Some(p);
    }
    let mut matches = peers.iter().filter(|p| p.name.starts_with(id_or_name));
    let first = matches.next()?;
    if matches.next().is_none() {
        Some(first)
    } else {
        None
    }
}

/// Deliver a plain-text message to a peer's UDS inbox via the rails
/// transport (`send_envelope`), with a short timeout.
#[cfg(unix)]
pub fn send_message(
    from: &str,
    peer: &Peer,
    body: &str,
    timeout: std::time::Duration,
) -> Result<allternit_commrails::peer::DeliveryReceipt, String> {
    let envelope = PeerEnvelope::new(from, &peer.name, body);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    runtime
        .block_on(allternit_commrails::peer::send_envelope(
            &peer.inbox_socket,
            &envelope,
            timeout,
        ))
        .map_err(|e| format!("send_envelope: {e}"))
}

/// CLI entry: `ao peer list [--root <dir>]` / `ao peer send <name> <message> [--root <dir>]`.
pub fn run(args: &[String]) -> std::io::Result<i32> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[1.min(args.len())..];
    let (rest, root_flag) = take_root_flag(rest);
    match sub {
        "list" => {
            let root = registry_root(root_flag.as_deref())?;
            let peers = read_registry(&root)?;
            let out = serde_json::json!({
                "root": root,
                "registry": registry_path(&root),
                "peers": peers,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
            Ok(0)
        }
        "send" => {
            let name = rest.first().cloned().unwrap_or_default();
            let body = rest.get(1).cloned().unwrap_or_default();
            if name.is_empty() || body.is_empty() {
                eprintln!("usage: ao peer send <name> <message> [--root <dir>]");
                return Ok(2);
            }
            let root = registry_root(root_flag.as_deref())?;
            let peers = read_registry(&root)?;
            let Some(peer) = resolve(&peers, &name) else {
                eprintln!("ao peer send: no peer named {name:?} under {}", root.display());
                return Ok(1);
            };
            let receipt = send_message("ao", peer, &body, std::time::Duration::from_secs(2))
                .map_err(std::io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "to": peer.name,
                    "delivered": receipt.delivered,
                    "error": receipt.error,
                }))
                .unwrap()
            );
            Ok(if receipt.delivered { 0 } else { 1 })
        }
        _ => {
            eprintln!("usage: ao peer <list|send> [--root <dir>]");
            Ok(2)
        }
    }
}

fn take_root_flag(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut rest = Vec::with_capacity(args.len());
    let mut root = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--root" {
            root = iter.next().cloned();
        } else if let Some(value) = arg.strip_prefix("--root=") {
            root = Some(value.to_string());
        } else {
            rest.push(arg.clone());
        }
    }
    (rest, root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_registry(root: &Path, entries: serde_json::Value) {
        let dir = root.join(".allternit").join("peers");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("registry.json"), serde_json::to_string(&entries).unwrap()).unwrap();
    }

    fn peer_json(name: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "peer_id": format!("peer_{name}"),
            "name": name,
            "cwd": "/tmp",
            "vendor": "test",
            "inbox_socket": "/nonexistent/peer_inbox.sock",
            "registered_at": "2026-09-10T00:00:00Z",
            "last_heartbeat_at": "2026-09-10T00:00:00Z",
            "status": status,
        })
    }

    #[test]
    fn read_registry_parses_peers_and_dims_dead() {
        let tmp = tempfile::tempdir().unwrap();
        write_registry(
            tmp.path(),
            serde_json::json!({"peers": {
                "peer_a": peer_json("alpha", "active"),
                "peer_b": peer_json("beta", "idle"),
            }}),
        );
        let peers = read_registry(tmp.path()).unwrap();
        assert_eq!(peers.len(), 2);
        // Socket paths do not exist → display status dims to Dead, but the
        // file on disk must be untouched.
        assert!(peers.iter().all(|p| p.status == PeerStatus::Dead));
        let on_disk = std::fs::read_to_string(registry_path(tmp.path())).unwrap();
        assert!(on_disk.contains("\"active\""), "read must not persist");
    }

    #[test]
    fn read_registry_missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_registry(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn resolve_by_id_name_and_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        write_registry(
            tmp.path(),
            serde_json::json!({"peers": {
                "peer_1": peer_json("long-session-name", "active"),
                "peer_2": peer_json("long-other", "active"),
            }}),
        );
        let peers = read_registry(tmp.path()).unwrap();
        assert_eq!(resolve(&peers, "peer_long-session-name").unwrap().peer_id, "peer_long-session-name");
        assert_eq!(resolve(&peers, "long-session-name").unwrap().peer_id, "peer_long-session-name");
        assert!(resolve(&peers, "long-").is_none(), "ambiguous prefix");
        assert_eq!(resolve(&peers, "long-s").unwrap().name, "long-session-name");
        assert_eq!(resolve(&peers, "long-o").unwrap().name, "long-other");
        assert!(resolve(&peers, "zzz").is_none());
    }
}
