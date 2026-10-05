//! Health-check and integrity diagnostics for the Rails CLI.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::dependencies::load_graph;
use crate::rails_id::TicketId;
use crate::tickets::{TicketStatus, TicketStore};

/// Default age in days after which an open ticket is considered stale.
pub const DEFAULT_STALE_DAYS: i64 = 30;

/// A diagnostic issue found by the doctor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub ticket_id: Option<String>,
}

/// Result of a doctor run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub ticket_count: usize,
    pub edge_count: usize,
    pub stale_count: usize,
    pub orphan_count: usize,
    pub duplicate_count: usize,
    pub ledger_valid: bool,
    pub issues: Vec<DoctorIssue>,
}

/// Run all diagnostics on a workspace.
pub fn diagnose(
    root: &Path,
    stale_days: i64,
) -> Result<DoctorReport> {
    let store = TicketStore::new(root)?;
    let graph = load_graph(root)?;
    let tickets = store.list()?;

    let mut issues = Vec::new();

    // Dependency cycle check.
    if graph.has_cycle() {
        issues.push(DoctorIssue {
            severity: "error".to_string(),
            code: "cycle".to_string(),
            message: "blocking dependency cycle detected".to_string(),
            ticket_id: None,
        });
    }

    // Ledger integrity.
    let ledger_check = store.verify_chain()?;
    if !ledger_check.valid {
        for issue in &ledger_check.issues {
            issues.push(DoctorIssue {
                severity: "error".to_string(),
                code: "ledger".to_string(),
                message: issue.clone(),
                ticket_id: None,
            });
        }
    }

    // Closed tickets missing closed_at.
    for ticket in &tickets {
        if ticket.status == TicketStatus::Closed && ticket.closed_at.is_none() {
            issues.push(DoctorIssue {
                severity: "error".to_string(),
                code: "missing_closed_at".to_string(),
                message: format!("{} is closed but missing closed_at", ticket.id),
                ticket_id: Some(ticket.id.to_string()),
            });
        }
    }

    // Stale tickets.
    let stale_threshold = Utc::now() - Duration::days(stale_days);
    let mut stale_count = 0;
    for ticket in &tickets {
        if ticket.status != TicketStatus::Closed && ticket.updated_at < stale_threshold {
            stale_count += 1;
            issues.push(DoctorIssue {
                severity: "warning".to_string(),
                code: "stale".to_string(),
                message: format!(
                    "{} has not been updated since {}",
                    ticket.id, ticket.updated_at
                ),
                ticket_id: Some(ticket.id.to_string()),
            });
        }
    }

    // Orphan tickets (open, no deps in either direction).
    let mut orphan_count = 0;
    let connected: HashSet<TicketId> = graph
        .edges()
        .flat_map(|e| [e.from.clone(), e.to.clone()])
        .collect();
    for ticket in &tickets {
        if ticket.status != TicketStatus::Closed && !connected.contains(&ticket.id) {
            orphan_count += 1;
            issues.push(DoctorIssue {
                severity: "info".to_string(),
                code: "orphan".to_string(),
                message: format!("{} has no dependencies", ticket.id),
                ticket_id: Some(ticket.id.to_string()),
            });
        }
    }

    // Duplicate detection by normalized title.
    let mut by_title: HashMap<String, Vec<TicketId>> = HashMap::new();
    for ticket in &tickets {
        let key = ticket.title.trim().to_lowercase();
        by_title.entry(key).or_default().push(ticket.id.clone());
    }
    let mut duplicate_count = 0;
    for (title, ids) in &by_title {
        if ids.len() > 1 {
            duplicate_count += 1;
            issues.push(DoctorIssue {
                severity: "warning".to_string(),
                code: "duplicate".to_string(),
                message: format!(
                    "duplicate title {:?} on tickets: {}",
                    title,
                    ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ")
                ),
                ticket_id: None,
            });
        }
    }

    let ok = issues.iter().all(|i| i.severity != "error");

    Ok(DoctorReport {
        ok,
        ticket_count: tickets.len(),
        edge_count: graph.edges().count(),
        stale_count,
        orphan_count,
        duplicate_count,
        ledger_valid: ledger_check.valid,
        issues,
    })
}
