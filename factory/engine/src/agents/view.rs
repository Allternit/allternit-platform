//! One view of every agent: the session registry, the live panes and the peer
//! registry merged into API.md's `Agent` (camelCase JSON).
//!
//! The registry is reconciled against the live panes first (see
//! [`super::registry::Registry::reconcile`]), so `state` is a fact about the
//! pane, never a stale claim. Peers that are also registry sessions fold into
//! that session's row; other peers (local agents that registered themselves)
//! get their own row with no pane.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::LivePane;
use super::peer::{Peer, PeerStatus};
use super::registry::{slug_of, Change, Entry, Registry, RegistryFile};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub team: Option<String>,
    pub address: String,
    pub avatar: Option<Value>,
    pub role: Option<String>,
    pub binding: Binding,
    pub state: String,
    pub machine: Option<Machine>,
    pub pane: Option<PaneRef>,
    pub current_node: Option<Value>,
    pub proof: Option<Value>,
    pub context: ContextUse,
    pub reach: Vec<String>,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guarantee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directing_bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneRef {
    pub id: String,
    pub attachable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUse {
    pub used_pct: Option<f64>,
    pub tokens: Option<u64>,
}

/// The per-field honesty labels (SPEC §9). Nothing is delivered into a pane
/// by the engine yet (persona/memory/skills delivery is stream F6e's), so
/// every field says `unavailable` until that lands.
fn fields_unavailable() -> BTreeMap<String, String> {
    ["persona", "memory", "skills", "tools", "model", "permissions"]
        .into_iter()
        .map(|f| (f.to_string(), "unavailable".to_string()))
        .collect()
}

/// This machine, as an Agent's `machine`.
pub fn local_machine() -> Machine {
    Machine { id: "local".to_string(), name: hostname().unwrap_or_else(|| "this computer".to_string()) }
}

fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer outlives the call and its length is passed.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Agent state for a registry entry, given its live pane (if any).
pub fn entry_state(entry: &Entry, pane: Option<&LivePane>) -> &'static str {
    match pane {
        None if entry.lifecycle.as_deref() == Some("finished") => "done",
        None => "offline",
        Some(p) => match p.agent_status.as_deref() {
            Some("idle") => "idle",
            // The pane engine's `blocked` is an agent stopped at a prompt
            // only a person can answer (permission, question): needs you.
            Some("blocked") => "needs_you",
            Some("done") => "done",
            // A live pane whose agent the pane engine can't classify (a
            // headless run, a shell) is running a process: working.
            _ => "working",
        },
    }
}

/// The merged view. `registry` must already be reconciled against `live`.
pub fn agents(registry: &RegistryFile, live: &[LivePane], peers: &[Peer]) -> Vec<Agent> {
    let machine = local_machine();
    let live_by_session: BTreeMap<&str, &LivePane> = live.iter().map(|p| (p.session.as_str(), p)).collect();
    let mut out = Vec::new();
    for (session, entry) in &registry.sessions {
        let pane = live_by_session.get(session.as_str()).copied();
        out.push(entry_agent(session, entry, pane, &machine));
    }
    let sessions: BTreeSet<&str> = registry.sessions.keys().map(String::as_str).collect();
    for peer in peers {
        if sessions.contains(peer.name.as_str()) {
            continue;
        }
        out.push(peer_agent(peer, &machine));
    }
    out
}

/// One registry session as an Agent.
pub fn entry_agent(session: &str, entry: &Entry, pane: Option<&LivePane>, machine: &Machine) -> Agent {
    let slug = slug_of(session).to_string();
    let bot = entry.bot.clone().unwrap_or_else(|| super::registry::BotRef::placeholder(&slug));
    let team = bot.team.clone();
    let address = match &team {
        Some(team) => format!("{slug}@{team}"),
        None => slug.clone(),
    };
    Agent {
        id: bot.id.clone(),
        name: bot.name.clone().unwrap_or_else(|| slug.clone()),
        slug,
        team,
        address,
        avatar: None,
        role: bot.role.clone(),
        binding: Binding { kind: "terminal".into(), harness: entry.harness.clone(), ..Default::default() },
        state: entry_state(entry, pane).to_string(),
        machine: Some(machine.clone()),
        pane: pane.map(|p| PaneRef { id: p.pane_id.clone(), attachable: true }),
        current_node: None,
        proof: None,
        context: ContextUse::default(),
        reach: Vec::new(),
        fields: fields_unavailable(),
    }
}

fn peer_agent(peer: &Peer, machine: &Machine) -> Agent {
    let state = match peer.status {
        PeerStatus::Dead => "offline",
        PeerStatus::Active | PeerStatus::Idle => "idle",
    };
    Agent {
        id: format!("peer:{}", peer.peer_id),
        slug: peer.name.clone(),
        name: peer.name.clone(),
        team: None,
        address: peer.name.clone(),
        avatar: None,
        role: None,
        binding: Binding {
            kind: "terminal".into(),
            harness: Some(peer.vendor.clone()).filter(|v| !v.is_empty()),
            ..Default::default()
        },
        state: state.to_string(),
        machine: Some(machine.clone()),
        pane: None,
        current_node: None,
        proof: None,
        context: ContextUse::default(),
        reach: Vec::new(),
        fields: fields_unavailable(),
    }
}

/// The reconciled view at one moment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub agents: Vec<Agent>,
    /// What reconciling the registry against the live panes changed.
    pub reconciled: Vec<Change>,
    /// False when no pane engine could be asked (every session then reads
    /// `offline`: its pane can't be seen, so it can't be called running).
    pub panes_checked: bool,
    /// The pane engine's own state (down is reported, never hidden).
    pub engine: super::backend::EngineStatus,
}

/// Reconcile the registry against the live panes, then merge in the peers
/// of `root`. `cwd` keeps only sessions working under that directory (and
/// drops peers outside it).
pub async fn snapshot(root: &std::path::Path, registry: &Registry, cwd: Option<&str>) -> anyhow::Result<Snapshot> {
    let (live, engine) = match super::backend::backend() {
        // A pane engine that can't be read leaves the registry as it is (a
        // misread must not mark live sessions dead) and says why.
        Ok(pane) => match super::backend::blocking(move || Ok((pane.list(), pane.status()))).await? {
            (Ok(live), status) => (Some(live), status),
            (Err(e), _) => (None, super::backend::EngineStatus { running: false, error: Some(format!("{e:#}")) }),
        },
        Err(e) => (None, super::backend::EngineStatus { running: false, error: Some(format!("{e:#}")) }),
    };
    let reconciled = match &live {
        Some(live) => registry.reconcile(live)?,
        None => Vec::new(),
    };
    let mut file = registry.load()?;
    let mut peers = super::peer::PeerRegistry::new(root).map(|p| p.list()).unwrap_or_default();
    if let Some(cwd) = cwd {
        file.sessions.retain(|_, e| e.cwd.starts_with(cwd));
        peers.retain(|p| p.cwd.to_string_lossy().starts_with(cwd));
    }
    let panes_checked = live.is_some();
    let live = live.unwrap_or_default();
    // Without a pane engine nothing is live, so nothing reads running.
    Ok(Snapshot { agents: agents(&file, &live, &peers), reconciled, panes_checked, engine })
}

/// Find an agent by id, address, slug or session label.
pub fn find<'a>(agents: &'a [Agent], key: &str) -> Option<&'a Agent> {
    let key_slug = slug_of(key);
    agents
        .iter()
        .find(|a| a.id == key || a.address == key)
        .or_else(|| agents.iter().find(|a| a.slug == key_slug))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::{reconcile_file, BotRef};

    #[test]
    fn dead_pane_reads_offline_and_live_pane_reads_its_status() {
        let mut reg = RegistryFile::default();
        reg.sessions.insert("ao-gone".into(), Entry { lifecycle: Some("running".into()), ..Default::default() });
        reg.sessions.insert(
            "ao-live".into(),
            Entry {
                harness: Some("claude".into()),
                bot: Some(BotRef { id: "bot_1".into(), team: Some("build".into()), ..Default::default() }),
                ..Default::default()
            },
        );
        let live = vec![LivePane {
            session: "ao-live".into(),
            pane_id: "p1".into(),
            cwd: None,
            agent_status: Some("idle".into()),
        }];
        reconcile_file(&mut reg, &live, "t");
        let all = agents(&reg, &live, &[]);
        let gone = find(&all, "gone").unwrap();
        assert_eq!(gone.state, "offline");
        assert!(gone.pane.is_none());
        assert_eq!(gone.id, "local:gone");
        let l = find(&all, "live@build").unwrap();
        assert_eq!(l.id, "bot_1");
        assert_eq!(l.state, "idle");
        assert_eq!(l.pane.as_ref().unwrap().id, "p1");
        assert_eq!(l.binding.harness.as_deref(), Some("claude"));
        let v = serde_json::to_value(l).unwrap();
        assert_eq!(v["binding"]["type"], "terminal");
        assert!(v.get("currentNode").is_some() && v["context"].get("usedPct").is_some());
    }
}
