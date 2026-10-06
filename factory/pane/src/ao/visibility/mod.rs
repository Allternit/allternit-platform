//! P5 "who needs you" panel model — merges the three feeds and owns the
//! persistent waiting-on-you list (spec binding decisions 1–6).
//!
//! - **Feed 1 (engine agents):** `AgentInfo` rows from `agent.list` (the
//!   panel polls; `events.subscribe pane.agent_status_changed` exists
//!   server-side but has no client machinery — poll-diff transitions are used
//!   instead, documented in `docs/programs/ao/AO_VISIBILITY_PEERS_NOTES.md`).
//! - **Feed 2 (native sessions):** [`crate::ao::native`] catalog rows.
//! - **Feed 3 (Rails peers):** [`crate::ao::peers`] registry rows.
//!
//! Ordering is blocked-first: `status_priority` ports `client/shell.rs`
//! (Blocked 4 > Done 3 > Working 2 > Idle 1 > Unknown 0), then
//! `state_change_seq` descending as the recency tiebreak — the same composite
//! the sidebar aggregate uses.
//!
//! The waiting-on-you list is **persistent** (survives overlay open/close and
//! view switches; it is client-session state, not a transient toast): an
//! entry enters when a poll diff observes a transition to `Blocked` and
//! clears when the agent transitions out or disappears.
//! `state_change_seq` is the ordering token (newest blocked first).

use std::collections::HashMap;

use serde::Serialize;

use super::native::{resume_hint, HarnessListing, NativeSession};
use super::peers::Peer;
use crate::api::schema::{AgentInfo, AgentStatus};

/// Blocked-first status ranking — port of `client/shell.rs::status_priority`.
pub fn status_priority(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Done => 3,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

/// One raw sample of the three feeds, taken by the poller (or the
/// `ao visibility` CLI).
#[derive(Clone)]
pub struct FeedSample {
    /// `Err` when no engine server answered (panel still renders the other
    /// feeds; the error is shown in the snapshot).
    pub engine: Result<Vec<AgentInfo>, String>,
    pub native: Vec<NativeSession>,
    pub peers: Vec<Peer>,
}

/// Engine agent row in the merged panel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelEngineAgent {
    pub pane_id: String,
    pub workspace_id: String,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub status: AgentStatus,
    pub state_change_seq: u64,
    pub cwd: Option<String>,
    /// `Some(join_key)` when this agent's `agent_session` matched a native
    /// catalog row (spec binding decision 6).
    pub joined_native: Option<String>,
    /// Last user prompt text from the joined native catalog row (or the
    /// peer-backed cwd join), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    /// Epoch ms when that prompt arrived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<u64>,
    pub agent_session: Option<AgentSessionJson>,
}

/// Serializable view of `AgentSessionInfo` (the join key payload).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionJson {
    pub source: String,
    pub agent: String,
    pub kind: String,
    pub value: String,
}

impl From<&crate::api::schema::AgentSessionInfo> for AgentSessionJson {
    fn from(info: &crate::api::schema::AgentSessionInfo) -> Self {
        Self {
            source: info.source.clone(),
            agent: info.agent.clone(),
            kind: serde_json::to_string(&info.kind)
                .unwrap_or_else(|_| "\"unknown\"".to_string())
                .trim_matches('"')
                .to_string(),
            value: info.value.clone(),
        }
    }
}

/// Native catalog row plus its correlation result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelNativeRow {
    #[serde(flatten)]
    pub session: NativeSession,
    pub resume_hint: String,
    /// Live engine state when the row joined an `ao` pane agent.
    pub live: Option<PanelLiveRef>,
}

/// Live engine reference for a joined native row.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelLiveRef {
    pub pane_id: String,
    pub status: AgentStatus,
}

/// Peer row (rails crate type is already serializable; wrapped for panel
/// shape stability).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelPeerRow {
    #[serde(flatten)]
    pub peer: Peer,
}

/// One waiting-on-you entry. `blocked_seq` is the agent's
/// `state_change_seq` at the observed blocked transition — the ordering
/// token for the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingEntry {
    pub pane_id: String,
    pub workspace_id: String,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub blocked_seq: u64,
    pub observed_at_ms: u64,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub tokens: HashMap<String, String>,
}

/// Persistent waiting-on-you list (see module docs).
#[derive(Debug, Default)]
pub struct WaitingList {
    entries: HashMap<String, WaitingEntry>,
}

impl WaitingList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diff a fresh `agent.list` sample against the list: enter on
    /// transition to `Blocked`, clear on transition out or disappearance.
    pub fn observe(&mut self, agents: &[AgentInfo], now_ms: u64) {
        let mut blocked_now: HashMap<&str, &AgentInfo> = HashMap::new();
        for agent in agents {
            if agent.agent_status == AgentStatus::Blocked {
                blocked_now.insert(&agent.pane_id, agent);
            }
        }
        // Clear entries whose agent left the blocked set (or vanished).
        self.entries
            .retain(|pane_id, _| blocked_now.contains_key(pane_id.as_str()));
        // Enter new blocked agents.
        for (pane_id, agent) in blocked_now {
            self.entries.entry(pane_id.to_string()).or_insert_with(|| WaitingEntry {
                pane_id: pane_id.to_string(),
                workspace_id: agent.workspace_id.clone(),
                name: agent.name.clone(),
                agent: agent.agent.clone(),
                title: agent.title.clone(),
                blocked_seq: agent.state_change_seq,
                observed_at_ms: now_ms,
                tokens: agent.tokens.clone(),
            });
        }
    }

    /// Rebuild the list from scratch (headless `ao visibility`: no history).
    pub fn rebuild(&mut self, agents: &[AgentInfo], now_ms: u64) {
        self.entries.clear();
        self.observe(agents, now_ms);
    }

    /// Entries ordered by `blocked_seq` descending (newest block on top).
    pub fn ordered(&self) -> Vec<WaitingEntry> {
        let mut entries: Vec<WaitingEntry> = self.entries.values().cloned().collect();
        entries.sort_by(|a, b| b.blocked_seq.cmp(&a.blocked_seq));
        entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// True when a native catalog row and a pushed `agent_session` refer to the
/// same CLI session (spec binding decision 6).
///
/// Match rules:
/// - harness label: `info.agent` equals the catalog harness id (a
///   `herdr:<harness>` source prefix is stripped first);
/// - `kind = "id"`: exact `sessionId` match, or either side ends with the
///   other (gizzi `getNativeSession` tolerance), or a kimi-style
///   `session_<id>` prefix on the pushed value;
/// - `kind = "path"`: the pushed path equals the catalog row path, or either
///   ends with the other (claude hooks push the transcript file path, which
///   is exactly the catalog row's `path`).
pub fn agent_session_matches(session: &NativeSession, info: &crate::api::schema::AgentSessionInfo) -> bool {
    let agent = info.agent.strip_prefix("herdr:").unwrap_or(&info.agent);
    if agent != session.harness {
        return false;
    }
    let value = info.value.trim();
    if value.is_empty() {
        return false;
    }
    match info.kind {
        crate::agent_resume::AgentSessionRefKind::Id => {
            let id = session.session_id.trim();
            value == id
                || (!id.is_empty() && value.ends_with(id))
                || value.strip_prefix("session_") == Some(id)
                || (!id.is_empty() && id.ends_with(value) && value.len() >= 8)
                || session.path.contains(value)
        }
        crate::agent_resume::AgentSessionRefKind::Path => {
            session.path == value
                || session.path.ends_with(value)
                || (value.len() >= 8 && value.ends_with(&session.path))
        }
    }
}

/// Find the native catalog row an engine agent joins to, if any.
pub fn joined_native_row<'a>(
    agent: &AgentInfo,
    native: &'a [NativeSession],
) -> Option<&'a NativeSession> {
    let info = agent.agent_session.as_ref()?;
    native.iter().find(|s| agent_session_matches(s, info))
}

/// Peer-backed fallback join: a peer registered from the same cwd as the
/// agent's pane identifies the harness (`vendor`) and project (`cwd`); pick
/// the freshest catalog row sharing that cwd (several sessions can live in
/// one project). Used only when the `agent_session` join found nothing.
fn joined_native_via_peer<'a>(
    agent: &AgentInfo,
    peers: &[Peer],
    native: &'a [NativeSession],
) -> Option<&'a NativeSession> {
    let cwd = agent.cwd.as_deref()?;
    let peer = peers
        .iter()
        .find(|p| p.cwd.to_string_lossy() == cwd)?;
    let vendor = peer.vendor.strip_prefix("herdr:").unwrap_or(&peer.vendor);
    native
        .iter()
        .filter(|s| s.harness == vendor && s.cwd.as_deref() == Some(cwd))
        .max_by_key(|s| s.updated_at)
}

/// The full merged panel — what the overlay renders and `ao visibility`
/// prints.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelSnapshot {
    pub generated_at_ms: u64,
    pub engine: PanelEngineSection,
    pub waiting_on_you: Vec<WaitingEntry>,
    pub native: Vec<PanelNativeRow>,
    pub peers: Vec<PanelPeerRow>,
    pub harnesses: Vec<HarnessListing>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelEngineSection {
    pub agents: Vec<PanelEngineAgent>,
    /// `Some` when the engine server did not answer (panel shows the other
    /// feeds and this note).
    pub error: Option<String>,
}

/// Merge one feed sample into the renderable snapshot. `waiting` is the
/// persistent client-side list, already observed against this sample.
pub fn build_panel(
    sample: FeedSample,
    waiting: &WaitingList,
    harnesses: Vec<HarnessListing>,
    now_ms: u64,
) -> PanelSnapshot {
    let (engine_agents, engine_error) = match sample.engine {
        Ok(agents) => (agents, None),
        Err(e) => (Vec::new(), Some(e)),
    };

    let mut panel_agents: Vec<PanelEngineAgent> = engine_agents
        .iter()
        .map(|agent| {
            let joined = joined_native_row(agent, &sample.native)
                .or_else(|| joined_native_via_peer(agent, &sample.peers, &sample.native));
            PanelEngineAgent {
                pane_id: agent.pane_id.clone(),
                workspace_id: agent.workspace_id.clone(),
                name: agent.name.clone(),
                agent: agent.agent.clone(),
                title: agent.title.clone(),
                status: agent.agent_status,
                state_change_seq: agent.state_change_seq,
                cwd: agent.cwd.clone(),
                joined_native: joined.map(|s| s.join_key()),
                last_message: joined.and_then(|s| s.last_prompt.clone()),
                last_message_at: joined.and_then(|s| s.last_prompt_at),
                agent_session: agent.agent_session.as_ref().map(Into::into),
            }
        })
        .collect();
    panel_agents.sort_by(|a, b| {
        status_priority(b.status)
            .cmp(&status_priority(a.status))
            .then(b.state_change_seq.cmp(&a.state_change_seq))
    });

    // Native rows: joined rows carry their live engine state; unjoined rows
    // render as external catalog entries (spec binding decision 3 fallback).
    let native: Vec<PanelNativeRow> = sample
        .native
        .iter()
        .map(|session| {
            let live = engine_agents
                .iter()
                .find(|agent| {
                    agent
                        .agent_session
                        .as_ref()
                        .is_some_and(|info| agent_session_matches(session, info))
                })
                .map(|agent| PanelLiveRef {
                    pane_id: agent.pane_id.clone(),
                    status: agent.agent_status,
                });
            PanelNativeRow {
                session: session.clone(),
                resume_hint: resume_hint(&session.harness).unwrap_or("").to_string(),
                live,
            }
        })
        .collect();

    PanelSnapshot {
        generated_at_ms: now_ms,
        engine: PanelEngineSection {
            agents: panel_agents,
            error: engine_error,
        },
        waiting_on_you: waiting.ordered(),
        native,
        peers: sample
            .peers
            .into_iter()
            .map(|peer| PanelPeerRow { peer })
            .collect(),
        harnesses,
    }
}

/// Epoch milliseconds (panel timestamps).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Extract the agents vector from an `agent.list` JSON response.
pub fn parse_agent_list(value: &serde_json::Value) -> Result<Vec<AgentInfo>, String> {
    let agents = value
        .pointer("/result/agents")
        .cloned()
        .ok_or_else(|| "agent.list response missing result.agents".to_string())?;
    serde_json::from_value(agents).map_err(|err| format!("agent.list parse failed: {err}"))
}

/// Poll the engine feed through a connected API client. `Err(reason)` when
/// the server did not answer — the panel degrades to an empty engine section
/// with the error noted.
pub fn engine_feed(client: &crate::api::client::ApiClient) -> Result<Vec<AgentInfo>, String> {
    use crate::api::schema::{EmptyParams, Method, Request};
    let value = client
        .request_value(&Request {
            id: "visibility-feed".into(),
            method: Method::AgentList(EmptyParams::default()),
        })
        .map_err(|err| format!("engine agent.list failed: {err}"))?;
    parse_agent_list(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::AgentSessionRefKind;
    use crate::api::schema::AgentSessionInfo;

    fn agent_info(pane_id: &str, status: AgentStatus, seq: u64) -> AgentInfo {
        AgentInfo {
            terminal_id: format!("term-{pane_id}"),
            name: Some(format!("agent-{pane_id}")),
            agent: Some("kimi".to_string()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: status,
            screen_detection_skipped: false,
            state_labels: Default::default(),
            tokens: Default::default(),
            agent_session: None,
            workspace_id: "ws1".to_string(),
            tab_id: "tab1".to_string(),
            pane_id: pane_id.to_string(),
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: seq,
            cwd: Some("/tmp/demo".to_string()),
            foreground_cwd: None,
            revision: 1,
        }
    }

    fn kimi_session(id: &str) -> NativeSession {
        NativeSession {
            harness: "kimi".to_string(),
            session_id: id.to_string(),
            path: format!("/home/.kimi-code/sessions/wd_a/session_{id}"),
            cwd: Some("/tmp/demo".to_string()),
            title: None,
            updated_at: 1,
            created_at: None,
            fingerprint: "f".repeat(64),
            last_event_id: Some(id.to_string()),
            last_prompt: None,
            last_prompt_at: None,
            installed: true,
            reader: crate::ao::native::ReaderKind::Directory,
            projectable: true,
        }
    }

    fn session_info(agent: &str, kind: AgentSessionRefKind, value: &str) -> AgentSessionInfo {
        AgentSessionInfo {
            source: format!("herdr:{agent}"),
            agent: agent.to_string(),
            kind,
            value: value.to_string(),
        }
    }

    #[test]
    fn waiting_list_enters_on_blocked_and_clears_on_exit() {
        let mut list = WaitingList::new();
        let working = vec![agent_info("p1", AgentStatus::Working, 1)];
        list.observe(&working, 100);
        assert!(list.is_empty());

        // Transition to Blocked → enters.
        let blocked = vec![
            agent_info("p1", AgentStatus::Blocked, 2),
            agent_info("p2", AgentStatus::Blocked, 5),
        ];
        list.observe(&blocked, 200);
        assert_eq!(list.len(), 2);
        // state_change_seq ordering: newest block first.
        let ordered = list.ordered();
        assert_eq!(ordered[0].pane_id, "p2");
        assert_eq!(ordered[1].pane_id, "p1");
        assert_eq!(ordered[0].blocked_seq, 5);

        // Transition out of Blocked → clears (p2 stays blocked in the same
        // agent.list sample, like a real poll).
        let unblocked = vec![
            agent_info("p1", AgentStatus::Working, 3),
            agent_info("p2", AgentStatus::Blocked, 5),
        ];
        list.observe(&unblocked, 300);
        assert_eq!(list.len(), 1);
        assert_eq!(list.ordered()[0].pane_id, "p2");

        // Agent disappears entirely → clears.
        list.observe(&[], 400);
        assert!(list.is_empty());
    }

    #[test]
    fn waiting_list_entry_survives_repeated_polls_while_blocked() {
        let mut list = WaitingList::new();
        list.observe(&[agent_info("p1", AgentStatus::Blocked, 2)], 100);
        // Still blocked at a later seq (labels churn): entry persists with
        // the original blocked_seq.
        list.observe(&[agent_info("p1", AgentStatus::Blocked, 9)], 200);
        assert_eq!(list.len(), 1);
        assert_eq!(list.ordered()[0].blocked_seq, 2);
    }

    #[test]
    fn waiting_list_entry_carries_pane_tokens() {
        let mut list = WaitingList::new();
        let mut blocked = agent_info("p1", AgentStatus::Blocked, 2);
        blocked
            .tokens
            .insert("needsYou".to_string(), "approve plan".to_string());
        blocked
            .tokens
            .insert("dagWih".to_string(), "wih_123".to_string());
        list.observe(&[blocked], 100);

        let entry = &list.ordered()[0];
        assert_eq!(entry.tokens.get("needsYou").map(String::as_str), Some("approve plan"));
        assert_eq!(entry.tokens.get("dagWih").map(String::as_str), Some("wih_123"));

        // Carried through the panel snapshot and serialized as `tokens`.
        let panel = build_panel(
            FeedSample {
                engine: Ok(vec![]),
                native: vec![],
                peers: vec![],
            },
            &list,
            vec![],
            100,
        );
        let value = serde_json::to_value(&panel.waiting_on_you[0]).unwrap();
        assert_eq!(
            value.pointer("/tokens/needsYou").and_then(|v| v.as_str()),
            Some("approve plan")
        );

        // Empty tokens are skipped in JSON entirely.
        let mut empty = WaitingList::new();
        empty.observe(&[agent_info("p2", AgentStatus::Blocked, 3)], 100);
        assert!(empty.ordered()[0].tokens.is_empty());
        let value = serde_json::to_value(&empty.ordered()[0]).unwrap();
        assert!(value.get("tokens").is_none());
    }

    #[test]
    fn blocked_first_ordering_ports_status_priority() {
        assert_eq!(status_priority(AgentStatus::Blocked), 4);
        assert_eq!(status_priority(AgentStatus::Done), 3);
        assert_eq!(status_priority(AgentStatus::Working), 2);
        assert_eq!(status_priority(AgentStatus::Idle), 1);
        assert_eq!(status_priority(AgentStatus::Unknown), 0);

        let sample = FeedSample {
            engine: Ok(vec![
                agent_info("idle", AgentStatus::Idle, 1),
                agent_info("blocked", AgentStatus::Blocked, 2),
                agent_info("working", AgentStatus::Working, 3),
            ]),
            native: vec![],
            peers: vec![],
        };
        let mut waiting = WaitingList::new();
        let agents = sample.engine.as_ref().unwrap().clone();
        waiting.observe(&agents, 0);
        let panel = build_panel(sample, &waiting, vec![], 0);
        let order: Vec<&str> = panel.engine.agents.iter().map(|a| a.pane_id.as_str()).collect();
        assert_eq!(order, vec!["blocked", "working", "idle"]);
    }

    #[test]
    fn join_key_matches_kimi_session_id() {
        let session = kimi_session("aaa-bbb");
        assert!(agent_session_matches(
            &session,
            &session_info("kimi", AgentSessionRefKind::Id, "aaa-bbb")
        ));
        // kimi hooks may push the raw dir id with the session_ prefix.
        assert!(agent_session_matches(
            &session,
            &session_info("kimi", AgentSessionRefKind::Id, "session_aaa-bbb")
        ));
        // herdr: source-prefixed agent label still matches.
        assert!(agent_session_matches(
            &session,
            &session_info("herdr:kimi", AgentSessionRefKind::Id, "aaa-bbb")
        ));
        // Wrong harness must not join.
        assert!(!agent_session_matches(
            &session,
            &session_info("claude", AgentSessionRefKind::Id, "aaa-bbb")
        ));
    }

    #[test]
    fn join_key_matches_claude_transcript_path() {
        let session = NativeSession {
            harness: "claude".to_string(),
            session_id: "uuid-1".to_string(),
            path: "/home/.claude/projects/-tmp-demo/uuid-1.jsonl".to_string(),
            ..kimi_session("unused")
        };
        assert!(agent_session_matches(
            &session,
            &session_info("claude", AgentSessionRefKind::Path, "/home/.claude/projects/-tmp-demo/uuid-1.jsonl")
        ));
        assert!(agent_session_matches(
            &session,
            &session_info("claude", AgentSessionRefKind::Path, "-tmp-demo/uuid-1.jsonl")
        ));
    }

    #[test]
    fn panel_marks_joined_and_external_native_rows() {
        let mut agent = agent_info("p1", AgentStatus::Working, 1);
        agent.agent_session = Some(session_info("kimi", AgentSessionRefKind::Id, "aaa-bbb"));
        let sample = FeedSample {
            engine: Ok(vec![agent]),
            native: vec![kimi_session("aaa-bbb"), kimi_session("external-1")],
            peers: vec![],
        };
        let mut waiting = WaitingList::new();
        let agents = sample.engine.as_ref().unwrap().clone();
        waiting.observe(&agents, 0);
        let panel = build_panel(sample, &waiting, vec![], 0);
        let joined = panel
            .native
            .iter()
            .find(|r| r.session.session_id == "aaa-bbb")
            .unwrap();
        assert_eq!(joined.live.as_ref().unwrap().pane_id, "p1");
        let external = panel
            .native
            .iter()
            .find(|r| r.session.session_id == "external-1")
            .unwrap();
        assert!(external.live.is_none(), "no join → external catalog row");
        // Engine row carries the join key back-reference.
        assert_eq!(
            panel.engine.agents[0].joined_native.as_deref(),
            Some("kimi+aaa-bbb")
        );
    }

    #[test]
    fn joined_native_row_flows_last_prompt_to_engine_agent() {
        let mut agent = agent_info("p1", AgentStatus::Working, 1);
        agent.agent_session = Some(session_info("kimi", AgentSessionRefKind::Id, "aaa-bbb"));
        let mut session = kimi_session("aaa-bbb");
        session.last_prompt = Some("fix the login bug".to_string());
        session.last_prompt_at = Some(1725974400000);
        let sample = FeedSample {
            engine: Ok(vec![agent]),
            native: vec![session],
            peers: vec![],
        };
        let mut waiting = WaitingList::new();
        let agents = sample.engine.as_ref().unwrap().clone();
        waiting.observe(&agents, 0);
        let panel = build_panel(sample, &waiting, vec![], 0);
        let row = &panel.engine.agents[0];
        assert_eq!(row.last_message.as_deref(), Some("fix the login bug"));
        assert_eq!(row.last_message_at, Some(1725974400000));
    }

    #[test]
    fn engine_error_degrades_gracefully() {
        let sample = FeedSample {
            engine: Err("connection refused".to_string()),
            native: vec![kimi_session("x")],
            peers: vec![],
        };
        let waiting = WaitingList::new();
        let panel = build_panel(sample, &waiting, vec![], 0);
        assert_eq!(panel.engine.error.as_deref(), Some("connection refused"));
        assert!(panel.engine.agents.is_empty());
        assert_eq!(panel.native.len(), 1, "other feeds still render");
    }
}
