//! Shared test fixtures for the graph module (diamond, chain, tickets).
//!
//! Test-only: compiled under `#[cfg(test)]` via `mod fixtures;` in `mod.rs`.

use std::collections::HashMap;

use chrono::Utc;

use crate::dependencies::{DependencyEdge, DependencyGraph, DependencyKind};
use crate::rails_id::{HierarchicalId, TicketId};
use crate::tickets::{Ticket, TicketKind, TicketPriority, TicketStatus};

pub fn id(name: &str) -> TicketId {
    TicketId::new(format!("T-{name}"))
}

pub fn blocks(graph: &mut DependencyGraph, from: &str, to: &str) {
    graph.add(DependencyEdge::new(id(from), id(to), DependencyKind::Blocks));
}

/// A blocks B,C; B,C block D.
pub fn diamond() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    blocks(&mut g, "a", "b");
    blocks(&mut g, "a", "c");
    blocks(&mut g, "b", "d");
    blocks(&mut g, "c", "d");
    g
}

pub fn chain(names: &[&str]) -> DependencyGraph {
    let mut g = DependencyGraph::new();
    for w in names.windows(2) {
        blocks(&mut g, w[0], w[1]);
    }
    g
}

/// A minimal open task ticket for view-model tests.
pub fn ticket(name: &str) -> Ticket {
    let id = id(name);
    let now = Utc::now();
    Ticket {
        hierarchical_id: HierarchicalId::root(id.clone()),
        id,
        title: format!("Ticket {name}"),
        description: String::new(),
        design: None,
        acceptance: None,
        notes: Vec::new(),
        status: TicketStatus::Open,
        kind: TicketKind::Task,
        priority: TicketPriority::P2,
        assignee: None,
        estimate_minutes: None,
        due_at: None,
        defer_until: None,
        labels: Vec::new(),
        external_ref: None,
        metadata: HashMap::new(),
        created_at: now,
        updated_at: now,
        closed_at: None,
        close_reason: None,
    }
}
