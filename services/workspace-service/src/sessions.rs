//! In-memory session and pane store.
//!
//! Sessions map roughly to tmux sessions; panes are individual terminal splits.
//! A production deployment would persist to Redis or SQLite.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub name: String,
    pub workspace_id: Option<String>,
    pub status: SessionStatus,
    pub working_dir: Option<String>,
    pub env: HashMap<String, String>,
    pub metadata: SessionMetadata,
    pub pane_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Idle,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMetadata {
    pub dag_id: Option<String>,
    pub wih_id: Option<String>,
    pub owner: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    /// Team workspace this session belongs to
    pub team_workspace_id: Option<String>,
    /// Team name for display
    pub team_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneRecord {
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub metadata: PaneMetadata,
    pub output_buffer: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// Backing Factory pane engine terminal id (a real PTY); None until the
    /// terminal is provisioned.
    #[serde(default)]
    pub terminal_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaneMetadata {
    pub agent_id: Option<String>,
    pub wih_id: Option<String>,
    pub pane_type: Option<String>,
    /// Team workspace this pane belongs to
    pub team_workspace_id: Option<String>,
}

pub struct SessionStore {
    sessions: RwLock<HashMap<String, SessionRecord>>,
    panes: RwLock<HashMap<String, PaneRecord>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            panes: RwLock::new(HashMap::new()),
        }
    }

    pub fn create_session(&self, name: String, metadata: SessionMetadata, working_dir: Option<String>, env: HashMap<String, String>, workspace_id: Option<String>) -> SessionRecord {
        let record = SessionRecord {
            id: Uuid::new_v4().to_string(),
            name,
            workspace_id,
            status: SessionStatus::Active,
            working_dir,
            env,
            metadata,
            pane_ids: vec![],
            created_at: Utc::now(),
        };
        self.sessions.write().unwrap().insert(record.id.clone(), record.clone());
        record
    }

    pub fn get_session(&self, id: &str) -> Option<SessionRecord> {
        // Support lookup by both ID and name
        let sessions = self.sessions.read().unwrap();
        sessions.get(id).cloned().or_else(|| {
            sessions.values().find(|s| s.name == id).cloned()
        })
    }

    pub fn list_sessions(&self, workspace_id: Option<&str>) -> Vec<SessionRecord> {
        let sessions = self.sessions.read().unwrap();
        sessions.values().filter(|s| {
            match workspace_id {
                Some(wid) => s.workspace_id.as_deref() == Some(wid),
                None => true,
            }
        }).cloned().collect()
    }

    pub fn delete_session(&self, id: &str) -> Option<SessionRecord> {
        let mut sessions = self.sessions.write().unwrap();
        if let Some(session) = sessions.remove(id) {
            let mut panes = self.panes.write().unwrap();
            for pane_id in &session.pane_ids {
                panes.remove(pane_id);
            }
            Some(session)
        } else {
            None
        }
    }

    /// Record the backing pane engine terminal id on a pane record.
    pub fn set_terminal_id(&self, id: &str, terminal_id: String) {
        if let Some(pane) = self.panes.write().unwrap().get_mut(id) {
            pane.terminal_id = Some(terminal_id);
        }
    }

    pub fn create_pane(&self, session_id: &str, title: String, metadata: PaneMetadata) -> Option<PaneRecord> {
        let pane = PaneRecord {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            title,
            metadata,
            output_buffer: vec![],
            created_at: Utc::now(),
            terminal_id: None,
        };

        let mut sessions = self.sessions.write().unwrap();
        if let Some(session) = sessions.get_mut(session_id) {
            session.pane_ids.push(pane.id.clone());
            drop(sessions);
            self.panes.write().unwrap().insert(pane.id.clone(), pane.clone());
            Some(pane)
        } else {
            None
        }
    }

    pub fn get_pane(&self, id: &str) -> Option<PaneRecord> {
        self.panes.read().unwrap().get(id).cloned()
    }

    pub fn delete_pane(&self, id: &str) -> bool {
        let removed = self.panes.write().unwrap().remove(id).is_some();
        if removed {
            let mut sessions = self.sessions.write().unwrap();
            for session in sessions.values_mut() {
                session.pane_ids.retain(|pid| pid != id);
            }
        }
        removed
    }

    pub fn append_pane_output(&self, id: &str, line: String) {
        if let Some(pane) = self.panes.write().unwrap().get_mut(id) {
            // Cap buffer to last 1000 lines
            if pane.output_buffer.len() >= 1000 {
                pane.output_buffer.drain(0..100);
            }
            pane.output_buffer.push(line);
        }
    }

    pub fn capture_pane_output(&self, id: &str) -> Option<String> {
        self.panes.read().unwrap().get(id).map(|p| p.output_buffer.join("\n"))
    }

    pub fn session_count(&self) -> usize {
        self.sessions.read().unwrap().len()
    }

    pub fn pane_count(&self) -> usize {
        self.panes.read().unwrap().len()
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
