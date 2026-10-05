//! Local peer registry and UDS inbox transport for cross-session messaging.
//!
//! Each agent session on this machine registers itself as a `Peer`.  Other
//! sessions can discover it through `PeerRegistry` and deliver plain-text
//! messages directly to its Unix-domain inbox socket.  The registry is
//! intentionally local-only: two sessions can only see each other when they
//! share the same `.allternit` workspace root.

pub mod socket;

pub use crate::peer::socket::{DeliveryReceipt, PeerEnvelope, send_envelope};
#[cfg(unix)]
pub use crate::peer::socket::PeerSocket;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::core::ids::create_peer_id;
use crate::core::io::{ensure_dir, write_json_atomic};

/// Lifecycle status of a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerStatus {
    /// Actively heartbeating and listening.
    Active,
    /// Registered but has not heartbeated recently.
    Idle,
    /// Known to be gone (e.g. session killed, socket removed).
    Dead,
}

impl Default for PeerStatus {
    fn default() -> Self {
        PeerStatus::Active
    }
}

/// A registered local agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub peer_id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub vendor: String,
    pub inbox_socket: PathBuf,
    pub registered_at: String,
    pub last_heartbeat_at: String,
    #[serde(default)]
    pub status: PeerStatus,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    peers: HashMap<String, Peer>,
}

/// Thread-safe in-memory peer registry backed by `.allternit/peers/registry.json`.
pub struct PeerRegistry {
    root_dir: PathBuf,
    registry_path: PathBuf,
    inbox_dir: PathBuf,
    state: Mutex<RegistryFile>,
}

impl PeerRegistry {
    /// Open or create a registry under `<root_dir>/.allternit/peers`.
    pub fn new(root_dir: impl AsRef<Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref().to_path_buf();
        let peers_dir = root_dir.join(".allternit").join("peers");
        let inbox_dir = peers_dir.join("inbox");
        let registry_path = peers_dir.join("registry.json");
        ensure_dir(&peers_dir)?;
        ensure_dir(&inbox_dir)?;

        let file: RegistryFile = crate::core::io::read_json_file(&registry_path, RegistryFile::default())?;
        Ok(Self {
            root_dir,
            registry_path,
            inbox_dir,
            state: Mutex::new(file),
        })
    }

    /// Register a new peer.  If `name` is already taken, the old peer is
    /// unregistered first so names stay singletons (matching the Claude Code
    /// `/rename` semantics).
    pub fn register(&self, name: &str, cwd: PathBuf, vendor: &str) -> Result<Peer> {
        if name.trim().is_empty() {
            bail!("peer name must not be empty");
        }
        let safe_name = sanitize_name(name);
        if safe_name.is_empty() {
            bail!("peer name has no usable characters");
        }

        let mut state = self.state.lock().unwrap();

        // Evict any existing peer with the same name.
        let mut to_evict: Option<String> = None;
        for (id, p) in &state.peers {
            if p.name == safe_name {
                to_evict = Some(id.clone());
                break;
            }
        }
        if let Some(id) = to_evict {
            self.remove_peer_files(&state.peers[&id]);
            state.peers.remove(&id);
        }

        let peer_id = create_peer_id();
        let inbox_socket = self.inbox_path(&peer_id);
        let now = Utc::now().to_rfc3339();
        let peer = Peer {
            peer_id: peer_id.clone(),
            name: safe_name,
            cwd,
            vendor: vendor.to_string(),
            inbox_socket,
            registered_at: now.clone(),
            last_heartbeat_at: now,
            status: PeerStatus::Active,
        };

        state.peers.insert(peer_id.clone(), peer.clone());
        self.save_locked(&state)?;
        Ok(peer)
    }

    /// Remove a peer by id or name.
    pub fn unregister(&self, id_or_name: &str) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        let key = match Self::find_key(&state, id_or_name) {
            Some(k) => k,
            None => return Ok(false),
        };
        let peer = state.peers.remove(&key).expect("key resolved to peer");
        self.remove_peer_files(&peer);
        self.save_locked(&state)?;
        Ok(true)
    }

    /// Refresh a peer's heartbeat timestamp.
    pub fn heartbeat(&self, id_or_name: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let key = match Self::find_key(&state, id_or_name) {
            Some(k) => k,
            None => bail!("peer not found: {}", id_or_name),
        };
        let now = Utc::now().to_rfc3339();
        if let Some(peer) = state.peers.get_mut(&key) {
            peer.last_heartbeat_at = now;
            peer.status = PeerStatus::Active;
        }
        self.save_locked(&state)?;
        Ok(())
    }

    /// Mark a peer as dead without removing it from the registry.
    pub fn mark_dead(&self, id_or_name: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let key = match Self::find_key(&state, id_or_name) {
            Some(k) => k,
            None => bail!("peer not found: {}", id_or_name),
        };
        if let Some(peer) = state.peers.get_mut(&key) {
            peer.status = PeerStatus::Dead;
        }
        self.save_locked(&state)?;
        Ok(())
    }

    /// Resolve a peer by exact id, exact name, or (if unambiguous) name prefix.
    pub fn resolve(&self, id_or_name: &str) -> Option<Peer> {
        let state = self.state.lock().unwrap();
        Self::find_key(&state, id_or_name).and_then(|k| state.peers.get(&k).cloned())
    }

    /// List all registered peers, re-checking socket liveness cheaply.
    pub fn list(&self) -> Vec<Peer> {
        let mut state = self.state.lock().unwrap();
        for peer in state.peers.values_mut() {
            if peer.status != PeerStatus::Dead && !peer.inbox_socket.exists() {
                peer.status = PeerStatus::Dead;
            }
        }
        // Best-effort save of updated statuses.
        let _ = self.save_locked(&state);
        state.peers.values().cloned().collect()
    }

    /// Root directory this registry is bound to.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// Directory where inbox sockets are created.
    pub fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }

    fn inbox_path(&self, peer_id: &str) -> PathBuf {
        self.inbox_dir.join(format!("{}.sock", peer_id))
    }

    fn remove_peer_files(&self, peer: &Peer) {
        let _ = std::fs::remove_file(&peer.inbox_socket);
    }

    fn find_key(state: &RegistryFile, id_or_name: &str) -> Option<String> {
        // Exact id.
        if state.peers.contains_key(id_or_name) {
            return Some(id_or_name.to_string());
        }
        // Exact name.
        for (id, p) in &state.peers {
            if p.name == id_or_name {
                return Some(id.clone());
            }
        }
        // Unambiguous prefix of a name.
        let mut prefix_matches: Vec<String> = Vec::new();
        for (id, p) in &state.peers {
            if p.name.starts_with(id_or_name) {
                prefix_matches.push(id.clone());
            }
        }
        if prefix_matches.len() == 1 {
            return Some(prefix_matches.into_iter().next().unwrap());
        }
        None
    }

    fn save_locked(&self, state: &RegistryFile) -> Result<()> {
        write_json_atomic(&self.registry_path, state)
            .map_err(|e| anyhow::anyhow!("failed to write peer registry: {}", e))
    }
}

fn sanitize_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            ' ' => '-',
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_registry() -> (TempDir, PeerRegistry) {
        let tmp = TempDir::new().unwrap();
        let reg = PeerRegistry::new(tmp.path()).unwrap();
        (tmp, reg)
    }

    #[test]
    fn register_and_list() {
        let (_tmp, reg) = tmp_registry();
        let p = reg.register("my-session", PathBuf::from("/tmp/wt"), "gizzi").unwrap();
        assert_eq!(p.name, "my-session");
        assert_eq!(p.vendor, "gizzi");
        assert!(p.inbox_socket.to_string_lossy().contains("peer_"));

        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].peer_id, p.peer_id);
    }

    #[test]
    fn name_collision_replaces_old_peer() {
        let (_tmp, reg) = tmp_registry();
        let p1 = reg.register("foo", PathBuf::from("/tmp/a"), "kimi").unwrap();
        let p2 = reg.register("foo", PathBuf::from("/tmp/b"), "claude").unwrap();
        assert_ne!(p1.peer_id, p2.peer_id);

        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].vendor, "claude");
    }

    #[test]
    fn resolve_by_id_name_and_prefix() {
        let (_tmp, reg) = tmp_registry();
        let p = reg.register("long-session-name", PathBuf::from("/tmp"), "agy").unwrap();
        assert_eq!(reg.resolve(&p.peer_id).unwrap().peer_id, p.peer_id);
        assert_eq!(reg.resolve("long-session-name").unwrap().peer_id, p.peer_id);
        assert_eq!(reg.resolve("long-").unwrap().peer_id, p.peer_id);
        assert!(reg.resolve("zzz").is_none());
    }

    #[test]
    fn unregister_removes_peer() {
        let (_tmp, reg) = tmp_registry();
        let p = reg.register("to-remove", PathBuf::from("/tmp"), "codex").unwrap();
        assert!(reg.unregister("to-remove").unwrap());
        assert!(reg.resolve(&p.peer_id).is_none());
        assert!(!reg.unregister("to-remove").unwrap());
    }

    #[test]
    fn heartbeat_refreshes_status() {
        let (_tmp, reg) = tmp_registry();
        reg.register("h", PathBuf::from("/tmp"), "gizzi").unwrap();
        reg.heartbeat("h").unwrap();
        let p = reg.resolve("h").unwrap();
        assert_eq!(p.status, PeerStatus::Active);
    }
}
