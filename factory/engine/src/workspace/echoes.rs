//! Ephemeral work units for the Rails CLI.
//!
//! Echoes are short-lived tickets used for heartbeats, pings, patrols,
//! error reports, escalations, and other transient agent signals. They
//! auto-expire and can be garbage collected.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};
use crate::rails_id::TicketId;

/// Default directory for echoes, relative to workspace root.
pub const ECHO_DIR: &str = ".allternit/rails/echoes";

/// Default TTL for an echo if not specified.
pub const DEFAULT_ECHO_TTL_SECONDS: i64 = 86400; // 24 hours

/// Kind of ephemeral signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoKind {
    Heartbeat,
    Ping,
    Patrol,
    GcReport,
    Recovery,
    Error,
    Escalation,
}

impl std::fmt::Display for EchoKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EchoKind::Heartbeat => write!(f, "heartbeat"),
            EchoKind::Ping => write!(f, "ping"),
            EchoKind::Patrol => write!(f, "patrol"),
            EchoKind::GcReport => write!(f, "gc_report"),
            EchoKind::Recovery => write!(f, "recovery"),
            EchoKind::Error => write!(f, "error"),
            EchoKind::Escalation => write!(f, "escalation"),
        }
    }
}

impl std::str::FromStr for EchoKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "heartbeat" => Ok(EchoKind::Heartbeat),
            "ping" => Ok(EchoKind::Ping),
            "patrol" => Ok(EchoKind::Patrol),
            "gc_report" => Ok(EchoKind::GcReport),
            "recovery" => Ok(EchoKind::Recovery),
            "error" => Ok(EchoKind::Error),
            "escalation" => Ok(EchoKind::Escalation),
            _ => anyhow::bail!("unknown echo kind: {s}"),
        }
    }
}

/// An ephemeral work unit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Echo {
    pub id: TicketId,
    pub kind: EchoKind,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub promoted_to: Option<TicketId>,
}

impl Echo {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }
}

/// Store for ephemeral echoes.
pub struct EchoStore {
    echoes_dir: PathBuf,
}

impl EchoStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let echoes_dir = root.join(ECHO_DIR);
        ensure_dir(&echoes_dir)?;
        Ok(Self { echoes_dir })
    }

    /// Create a new echo.
    pub fn create(
        &self,
        kind: EchoKind,
        content: impl Into<String>,
        ttl_seconds: i64,
    ) -> Result<Echo> {
        let content = content.into();
        let id = TicketId::mint(content.as_bytes());
        let created_at = Utc::now();
        let expires_at = created_at + Duration::seconds(ttl_seconds);
        let echo = Echo {
            id,
            kind,
            content,
            created_at,
            expires_at,
            promoted_to: None,
        };
        self.write(&echo)?;
        Ok(echo)
    }

    /// Get a single echo if it exists.
    pub fn get(&self, id: &TicketId) -> Result<Option<Echo>> {
        read_json(&self.path(id))
            .with_context(|| format!("failed to read echo {id}"))
    }

    /// List all echoes, optionally including expired ones.
    pub fn list(&self, include_expired: bool) -> Result<Vec<Echo>> {
        let now = Utc::now();
        let mut echoes = Vec::new();
        for entry in std::fs::read_dir(&self.echoes_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(echo) = read_json::<Echo>(&entry.path())? {
                    if include_expired || !echo.is_expired(now) {
                        echoes.push(echo);
                    }
                }
            }
        }
        echoes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(echoes)
    }

    /// Promote an echo to a permanent ticket.
    pub fn promote(&self, id: &TicketId, ticket_id: TicketId) -> Result<Echo> {
        let mut echo = self
            .get(id)?
            .with_context(|| format!("echo {id} not found"))?;
        echo.promoted_to = Some(ticket_id);
        self.write(&echo)?;
        Ok(echo)
    }

    /// Delete expired echoes and return the count removed.
    pub fn gc(&self) -> Result<usize> {
        let now = Utc::now();
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.echoes_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(echo) = read_json::<Echo>(&entry.path())? {
                    if echo.is_expired(now) && echo.promoted_to.is_none() {
                        std::fs::remove_file(entry.path())?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    fn path(&self, id: &TicketId) -> PathBuf {
        self.echoes_dir.join(format!("{}.json", id))
    }

    fn write(&self, echo: &Echo) -> Result<()> {
        let path = self.path(&echo.id);
        write_json_atomic(&path, echo)
            .with_context(|| format!("failed to write echo {path:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn create_and_list() {
        let tmp = TempDir::new().unwrap();
        let store = EchoStore::new(tmp.path()).unwrap();
        let echo = store
            .create(EchoKind::Heartbeat, "alive", 3600)
            .unwrap();
        assert_eq!(echo.kind, EchoKind::Heartbeat);

        let echoes = store.list(false).unwrap();
        assert_eq!(echoes.len(), 1);
    }

    #[test]
    fn expired_echoes_filtered() {
        let tmp = TempDir::new().unwrap();
        let store = EchoStore::new(tmp.path()).unwrap();
        store.create(EchoKind::Ping, "old", -1).unwrap();

        let active = store.list(false).unwrap();
        assert!(active.is_empty());

        let all = store.list(true).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn gc_removes_expired() {
        let tmp = TempDir::new().unwrap();
        let store = EchoStore::new(tmp.path()).unwrap();
        store.create(EchoKind::Error, "stale", -1).unwrap();
        store.create(EchoKind::Ping, "fresh", 3600).unwrap();

        let removed = store.gc().unwrap();
        assert_eq!(removed, 1);

        let active = store.list(false).unwrap();
        assert_eq!(active.len(), 1);
    }
}
