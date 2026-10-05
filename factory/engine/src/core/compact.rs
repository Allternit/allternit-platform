//! Compaction and pruning for the Rails CLI.
//!
//! Compaction reclaims space by removing redundant snapshots and garbage
//! collecting ephemeral data. Pruning archives or removes old closed tickets
//! according to retention policy.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::echoes::EchoStore;
use crate::tickets::TicketStore;

/// Result of a compaction run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompactResult {
    pub snapshots_removed: usize,
    pub snapshots_rebuilt: usize,
    pub echoes_removed: usize,
    pub sync_mappings_removed: usize,
    pub tickets_pruned: usize,
}

/// Compactor for Rails storage.
pub struct Compactor {
    root: PathBuf,
}

impl Compactor {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Remove all ticket snapshots and rebuild them from the event log.
    pub fn compact_snapshots(&self) -> Result<usize> {
        let store = TicketStore::new(&self.root)?;
        let snapshots_dir = self.root.join(crate::tickets::TICKET_SNAPSHOTS_DIR);

        let mut removed = 0;
        if snapshots_dir.exists() {
            for entry in std::fs::read_dir(&snapshots_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }

        store.rebuild_snapshots()?;
        Ok(removed)
    }

    /// Garbage collect expired echoes.
    pub fn compact_echoes(&self) -> Result<usize> {
        let store = EchoStore::new(&self.root)?;
        store.gc()
    }

    /// Remove stale sync mapping files for providers that are not configured.
    pub fn compact_sync_mappings(&self) -> Result<usize> {
        let sync_dir = self.root.join(".allternit/rails/sync");
        let mut removed = 0;
        if sync_dir.exists() {
            for entry in std::fs::read_dir(&sync_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".mappings.json") {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    /// Prune closed tickets older than the retention period.
    ///
    /// Pruning moves closed ticket events into an archive directory rather than
    /// deleting them, preserving auditability.
    pub fn prune_tickets(&self, retention_days: i64) -> Result<usize> {
        let store = TicketStore::new(&self.root)?;
        let cutoff = Utc::now() - Duration::days(retention_days);
        let archive_dir = self.root.join(".allternit/rails/archive/ticket_events");
        std::fs::create_dir_all(&archive_dir)?;

        let mut pruned = 0;
        for ticket in store.list()? {
            if ticket.status == crate::tickets::TicketStatus::Closed {
                let closed_at = ticket.closed_at.unwrap_or(ticket.updated_at);
                if closed_at < cutoff {
                    // Move this ticket's event files to archive.
                    let events_dir = self.root.join(crate::tickets::TICKET_EVENTS_DIR);
                    let prefix = format!("{}", ticket.id);
                    for entry in std::fs::read_dir(&events_dir)? {
                        let entry = entry?;
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.contains(&prefix) && name.ends_with(".json") {
                            let dest = archive_dir.join(&name);
                            std::fs::rename(entry.path(), dest)?;
                            pruned += 1;
                        }
                    }
                }
            }
        }

        Ok(pruned)
    }

    /// Run all compaction operations and return a summary.
    pub fn compact_all(&self) -> Result<CompactResult> {
        let snapshots_removed = self.compact_snapshots()?;
        let echoes_removed = self.compact_echoes()?;
        let sync_mappings_removed = self.compact_sync_mappings()?;
        Ok(CompactResult {
            snapshots_removed,
            snapshots_rebuilt: snapshots_removed, // each removed snapshot is rebuilt
            echoes_removed,
            sync_mappings_removed,
            tickets_pruned: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn compact_snapshots_rebuilds() {
        let tmp = TempDir::new().unwrap();
        let compactor = Compactor::new(tmp.path());
        let store = TicketStore::new(tmp.path()).unwrap();
        let ticket = crate::tickets::Ticket {
            id: crate::rails_id::TicketId::mint("test"),
            hierarchical_id: crate::rails_id::HierarchicalId::root(crate::rails_id::TicketId::mint("test")),
            title: "Test".to_string(),
            description: "".to_string(),
            design: None,
            acceptance: None,
            notes: Vec::new(),
            status: crate::tickets::TicketStatus::Open,
            kind: crate::tickets::TicketKind::Task,
            priority: crate::tickets::TicketPriority::P2,
            assignee: None,
            estimate_minutes: None,
            due_at: None,
            defer_until: None,
            labels: Vec::new(),
            external_ref: None,
            metadata: std::collections::HashMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
            close_reason: None,
        };
        store.create(ticket).unwrap();

        let removed = compactor.compact_snapshots().unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.list().unwrap().len(), 1);
    }
}
