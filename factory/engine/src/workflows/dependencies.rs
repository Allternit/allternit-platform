//! Typed dependency graph for Rails tickets.
//!
//! Rails supports several edge types beyond the hard `blocks` relationship:
//!
//! - `blocks` / `blocked_by`: hard blocker used for ready-list derivation
//! - `tracks`: tracks progress of another ticket
//! - `related`: non-blocking context link
//! - `duplicate_of`: marks a ticket as a duplicate of another
//! - `supersedes`: replaces another ticket
//! - `derived_from`: provenance link
//!
//! The graph is stored as a set of directed edges and validated to be
//! acyclic for hard-blocking edges.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::rails_id::TicketId;
use crate::tickets::{TicketEvent, TicketStore};

/// A directed edge between two tickets.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DependencyEdge {
    pub from: TicketId,
    pub to: TicketId,
    pub kind: DependencyKind,
}

impl DependencyEdge {
    pub fn new(from: TicketId, to: TicketId, kind: DependencyKind) -> Self {
        Self { from, to, kind }
    }
}

/// The semantic kind of a dependency edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    /// `from` blocks `to`. Used for ready-list computation.
    Blocks,
    /// `from` tracks progress of `to`.
    Tracks,
    /// `from` is related to `to`.
    Related,
    /// `from` is a duplicate of `to`.
    DuplicateOf,
    /// `from` supersedes `to`.
    Supersedes,
    /// `from` was derived from `to`.
    DerivedFrom,
}

impl DependencyKind {
    /// Return true if this edge type participates in hard-block cycle detection.
    pub fn is_blocking(&self) -> bool {
        matches!(self, DependencyKind::Blocks)
    }

    /// Return the natural inverse kind.
    pub fn inverse(&self) -> Self {
        match self {
            DependencyKind::Blocks => DependencyKind::Blocks,
            DependencyKind::Tracks => DependencyKind::Tracks,
            DependencyKind::Related => DependencyKind::Related,
            DependencyKind::DuplicateOf => DependencyKind::DuplicateOf,
            DependencyKind::Supersedes => DependencyKind::Supersedes,
            DependencyKind::DerivedFrom => DependencyKind::DerivedFrom,
        }
    }
}

impl std::fmt::Display for DependencyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencyKind::Blocks => write!(f, "blocks"),
            DependencyKind::Tracks => write!(f, "tracks"),
            DependencyKind::Related => write!(f, "related"),
            DependencyKind::DuplicateOf => write!(f, "duplicate_of"),
            DependencyKind::Supersedes => write!(f, "supersedes"),
            DependencyKind::DerivedFrom => write!(f, "derived_from"),
        }
    }
}

impl std::str::FromStr for DependencyKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "blocks" | "blocked_by" => Ok(DependencyKind::Blocks),
            "tracks" => Ok(DependencyKind::Tracks),
            "related" | "relates_to" => Ok(DependencyKind::Related),
            "duplicate_of" | "duplicates" => Ok(DependencyKind::DuplicateOf),
            "supersedes" => Ok(DependencyKind::Supersedes),
            "derived_from" | "discovered_from" => Ok(DependencyKind::DerivedFrom),
            _ => anyhow::bail!("unknown dependency kind: {s}"),
        }
    }
}

/// In-memory dependency graph.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DependencyGraph {
    edges: HashSet<DependencyEdge>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an edge. Returns false if the exact edge already existed.
    pub fn add(&mut self, edge: DependencyEdge) -> bool {
        self.edges.insert(edge)
    }

    /// Remove an exact edge. Returns true if it existed.
    pub fn remove(&mut self, from: &TicketId, to: &TicketId, kind: DependencyKind) -> bool {
        self.edges.remove(&DependencyEdge {
            from: from.clone(),
            to: to.clone(),
            kind,
        })
    }

    /// Remove all edges involving a ticket.
    pub fn remove_ticket(&mut self, id: &TicketId) {
        self.edges.retain(|e| e.from != *id && e.to != *id);
    }

    /// Return all edges.
    pub fn edges(&self) -> impl Iterator<Item = &DependencyEdge> {
        self.edges.iter()
    }

    /// Return edges of a specific kind.
    pub fn edges_of_kind(&self, kind: DependencyKind) -> Vec<&DependencyEdge> {
        self.edges.iter().filter(|e| e.kind == kind).collect()
    }

    /// Return edges where `id` is the source.
    pub fn outgoing(&self, id: &TicketId) -> Vec<&DependencyEdge> {
        self.edges.iter().filter(|e| e.from == *id).collect()
    }

    /// Return edges where `id` is the target.
    pub fn incoming(&self, id: &TicketId) -> Vec<&DependencyEdge> {
        self.edges.iter().filter(|e| e.to == *id).collect()
    }

    /// Return the set of tickets that `id` blocks (outgoing blocks edges).
    pub fn blocked_by(&self, id: &TicketId) -> Vec<&TicketId> {
        self.edges
            .iter()
            .filter(|e| e.from == *id && e.kind == DependencyKind::Blocks)
            .map(|e| &e.to)
            .collect()
    }

    /// Return the set of tickets that block `id` (incoming blocks edges).
    pub fn blocks(&self, id: &TicketId) -> Vec<&TicketId> {
        self.edges
            .iter()
            .filter(|e| e.to == *id && e.kind == DependencyKind::Blocks)
            .map(|e| &e.from)
            .collect()
    }

    /// Detect cycles in the blocking subgraph.
    pub fn has_cycle(&self) -> bool {
        let mut adj: HashMap<&TicketId, Vec<&TicketId>> = HashMap::new();
        for edge in self.edges.iter().filter(|e| e.kind.is_blocking()) {
            adj.entry(&edge.from).or_default().push(&edge.to);
        }

        let mut visited: HashSet<&TicketId> = HashSet::new();
        let mut stack: HashSet<&TicketId> = HashSet::new();

        fn dfs<'a>(
            node: &'a TicketId,
            adj: &HashMap<&'a TicketId, Vec<&'a TicketId>>,
            visited: &mut HashSet<&'a TicketId>,
            stack: &mut HashSet<&'a TicketId>,
        ) -> bool {
            if stack.contains(node) {
                return true;
            }
            if visited.contains(node) {
                return false;
            }
            visited.insert(node);
            stack.insert(node);
            for next in adj.get(node).into_iter().flatten() {
                if dfs(next, adj, visited, stack) {
                    return true;
                }
            }
            stack.remove(node);
            false
        }

        for node in adj.keys().copied().collect::<Vec<_>>() {
            if dfs(node, &adj, &mut visited, &mut stack) {
                return true;
            }
        }
        false
    }

    /// Return a concrete cycle if one exists in the blocking subgraph.
    pub fn find_cycle(&self) -> Option<Vec<TicketId>> {
        let mut adj: HashMap<&TicketId, Vec<&TicketId>> = HashMap::new();
        for edge in self.edges.iter().filter(|e| e.kind.is_blocking()) {
            adj.entry(&edge.from).or_default().push(&edge.to);
        }

        let mut visited: HashSet<&TicketId> = HashSet::new();
        let mut stack: Vec<&TicketId> = Vec::new();
        let mut on_stack: HashSet<&TicketId> = HashSet::new();

        fn dfs<'a>(
            node: &'a TicketId,
            adj: &HashMap<&'a TicketId, Vec<&'a TicketId>>,
            visited: &mut HashSet<&'a TicketId>,
            stack: &mut Vec<&'a TicketId>,
            on_stack: &mut HashSet<&'a TicketId>,
        ) -> Option<Vec<TicketId>> {
            if on_stack.contains(node) {
                let start = stack.iter().position(|&n| n == node).unwrap_or(0);
                let cycle = stack[start..]
                    .iter()
                    .map(|&n| n.clone())
                    .chain(std::iter::once(node.clone()))
                    .collect();
                return Some(cycle);
            }
            if visited.contains(node) {
                return None;
            }
            visited.insert(node);
            stack.push(node);
            on_stack.insert(node);
            for next in adj.get(node).into_iter().flatten() {
                if let Some(cycle) = dfs(next, adj, visited, stack, on_stack) {
                    return Some(cycle);
                }
            }
            stack.pop();
            on_stack.remove(node);
            None
        }

        for node in adj.keys().copied().collect::<Vec<_>>() {
            if let Some(cycle) = dfs(node, &adj, &mut visited, &mut stack, &mut on_stack) {
                return Some(cycle);
            }
        }
        None
    }

    /// Check whether adding `edge` would introduce a blocking cycle.
    pub fn would_cycle(&self, edge: &DependencyEdge) -> bool {
        if !edge.kind.is_blocking() {
            return false;
        }
        let mut g = self.clone();
        g.add(edge.clone());
        g.has_cycle()
    }
}

/// Path of the dependency graph snapshot, relative to workspace root.
pub const GRAPH_PATH: &str = ".allternit/rails/dependencies/graph.json";

/// Load the dependency graph for a workspace.
///
/// `graph.json` is a rebuildable snapshot: when it is missing the graph is
/// derived by replaying `DependencyAdded`/`DependencyRemoved` events from the
/// full ticket event log.
pub fn load_graph(root: &Path) -> Result<DependencyGraph> {
    let path = root.join(GRAPH_PATH);
    if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read dependency graph {path:?}"))?;
        let graph: DependencyGraph = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse dependency graph {path:?}"))?;
        return Ok(graph);
    }
    let store = TicketStore::new(root)?;
    rebuild_graph_from_events(&store)
}

/// Persist the dependency graph snapshot.
pub fn save_graph(root: &Path, graph: &DependencyGraph) -> Result<()> {
    let path = root.join(GRAPH_PATH);
    crate::core::io::write_json_atomic(&path, graph)
        .with_context(|| format!("failed to write dependency graph {path:?}"))
}

/// Rebuild the dependency graph by replaying dependency events from the full
/// ticket event log. Dependency events span two tickets but are filed under a
/// single id, so this must scan the whole log, not `events_for(id)`.
pub fn rebuild_graph_from_events(store: &TicketStore) -> Result<DependencyGraph> {
    let mut graph = DependencyGraph::new();
    for event in store.all_events()? {
        match event {
            TicketEvent::DependencyAdded { from, to, kind, .. } => {
                graph.add(DependencyEdge::new(from, to, kind));
            }
            TicketEvent::DependencyRemoved { from, to, kind, .. } => {
                graph.remove(&from, &to, kind);
            }
            _ => {}
        }
    }
    Ok(graph)
}

/// Add a dependency edge: append a hash-chained `DependencyAdded` event and
/// re-derive the graph snapshot from the event log.
///
/// Validates against blocking cycles before applying; on cycle the error
/// carries the cycle path and nothing is persisted.
pub fn add_edge(root: &Path, store: &TicketStore, edge: DependencyEdge) -> Result<()> {
    let graph = load_graph(root)?;
    if edge.kind.is_blocking() && graph.would_cycle(&edge) {
        let mut g = graph.clone();
        g.add(edge.clone());
        let cycle = g
            .find_cycle()
            .map(|ids| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(" -> "))
            .unwrap_or_else(|| format!("{} -> {}", edge.from, edge.to));
        anyhow::bail!("dependency would create a blocking cycle: {cycle}");
    }

    store.append(&TicketEvent::DependencyAdded {
        from: edge.from,
        to: edge.to,
        kind: edge.kind,
        ts: Utc::now(),
    })?;
    let graph = rebuild_graph_from_events(store)?;
    save_graph(root, &graph)?;
    Ok(())
}

/// Remove a dependency edge: append a hash-chained `DependencyRemoved` event
/// and re-derive the graph snapshot from the event log.
///
/// Returns false (and appends nothing) when the edge does not exist.
pub fn remove_edge(
    root: &Path,
    store: &TicketStore,
    from: &TicketId,
    to: &TicketId,
    kind: DependencyKind,
) -> Result<bool> {
    let graph = load_graph(root)?;
    if !graph.edges().any(|e| e.from == *from && e.to == *to && e.kind == kind) {
        return Ok(false);
    }

    store.append(&TicketEvent::DependencyRemoved {
        from: from.clone(),
        to: to.clone(),
        kind,
        ts: Utc::now(),
    })?;
    let graph = rebuild_graph_from_events(store)?;
    save_graph(root, &graph)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> TicketId {
        TicketId::mint(name)
    }

    #[test]
    fn blocks_relationship() {
        let mut g = DependencyGraph::new();
        let a = id("a");
        let b = id("b");
        g.add(DependencyEdge::new(a.clone(), b.clone(), DependencyKind::Blocks));

        assert_eq!(g.blocked_by(&a), vec![&b]);
        assert_eq!(g.blocks(&b), vec![&a]);
    }

    #[test]
    fn detects_cycle() {
        let mut g = DependencyGraph::new();
        let a = id("a");
        let b = id("b");
        let c = id("c");
        g.add(DependencyEdge::new(a.clone(), b.clone(), DependencyKind::Blocks));
        g.add(DependencyEdge::new(b.clone(), c.clone(), DependencyKind::Blocks));
        g.add(DependencyEdge::new(c.clone(), a.clone(), DependencyKind::Blocks));

        assert!(g.has_cycle());
        let cycle = g.find_cycle().unwrap();
        assert!(cycle.len() >= 3);
    }

    #[test]
    fn non_blocking_edges_do_not_cycle() {
        let mut g = DependencyGraph::new();
        let a = id("a");
        let b = id("b");
        g.add(DependencyEdge::new(a.clone(), b.clone(), DependencyKind::Related));
        g.add(DependencyEdge::new(b.clone(), a.clone(), DependencyKind::Related));

        assert!(!g.has_cycle());
    }

    #[test]
    fn would_cycle_predicts() {
        let mut g = DependencyGraph::new();
        let a = id("a");
        let b = id("b");
        g.add(DependencyEdge::new(a.clone(), b.clone(), DependencyKind::Blocks));

        let closing = DependencyEdge::new(b.clone(), a.clone(), DependencyKind::Blocks);
        assert!(g.would_cycle(&closing));
    }

    mod event_sourced {
        use super::*;
        use crate::rails_id::HierarchicalId;
        use crate::tickets::{Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore};
        use std::collections::HashMap;
        use tempfile::TempDir;

        fn make_ticket(store: &TicketStore, title: &str) -> Ticket {
            let id = TicketId::mint(title);
            let ticket = Ticket {
                id: id.clone(),
                hierarchical_id: HierarchicalId::root(id),
                title: title.to_string(),
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
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                closed_at: None,
                close_reason: None,
            };
            store.create(ticket.clone()).unwrap()
        }

        #[test]
        fn add_edge_emits_event_and_graph_rebuilds_from_full_log() {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            let store = TicketStore::new(root).unwrap();
            let a = make_ticket(&store, "a");
            let b = make_ticket(&store, "b");

            add_edge(
                root,
                &store,
                DependencyEdge::new(a.id.clone(), b.id.clone(), DependencyKind::Blocks),
            )
            .unwrap();

            // Event round-trip: the edge is derivable by replaying the log.
            let replayed = rebuild_graph_from_events(&store).unwrap();
            assert_eq!(replayed.blocked_by(&a.id), vec![&b.id]);

            // Delete the snapshot; load_graph must rebuild from the full log
            // (the event is filed only under `from`, so events_for(b) would miss it).
            std::fs::remove_file(root.join(GRAPH_PATH)).unwrap();
            let graph = load_graph(root).unwrap();
            assert_eq!(graph.blocks(&b.id), vec![&a.id]);

            // The blocked ticket is not ready.
            let gates = crate::wait_gates::WaitGateStore::new(root).unwrap();
            let all = store.list().unwrap();
            let ready =
                crate::tickets::ready(&all, &graph, &gates, chrono::Utc::now()).unwrap();
            assert!(ready.iter().any(|t| t.id == a.id));
            assert!(!ready.iter().any(|t| t.id == b.id));
        }

        #[test]
        fn remove_edge_emits_event_and_rebuild_drops_edge() {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            let store = TicketStore::new(root).unwrap();
            let a = make_ticket(&store, "a");
            let b = make_ticket(&store, "b");

            add_edge(
                root,
                &store,
                DependencyEdge::new(a.id.clone(), b.id.clone(), DependencyKind::Blocks),
            )
            .unwrap();
            let removed =
                remove_edge(root, &store, &a.id, &b.id, DependencyKind::Blocks).unwrap();
            assert!(removed);

            let graph = rebuild_graph_from_events(&store).unwrap();
            assert!(graph.blocks(&b.id).is_empty());
            assert_eq!(load_graph(root).unwrap().edges().count(), 0);

            // Removing a missing edge is a no-op.
            let removed =
                remove_edge(root, &store, &a.id, &b.id, DependencyKind::Blocks).unwrap();
            assert!(!removed);
        }

        #[test]
        fn add_edge_rejects_cycle_without_persisting() {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            let store = TicketStore::new(root).unwrap();
            let a = make_ticket(&store, "a");
            let b = make_ticket(&store, "b");

            add_edge(
                root,
                &store,
                DependencyEdge::new(a.id.clone(), b.id.clone(), DependencyKind::Blocks),
            )
            .unwrap();
            let event_count_before = store.all_events().unwrap().len();

            let err = add_edge(
                root,
                &store,
                DependencyEdge::new(b.id.clone(), a.id.clone(), DependencyKind::Blocks),
            )
            .unwrap_err();
            assert!(err.to_string().contains("cycle"));

            // No partial mutation: no new event, prior edge intact.
            assert_eq!(store.all_events().unwrap().len(), event_count_before);
            let graph = load_graph(root).unwrap();
            assert_eq!(graph.edges().count(), 1);
            assert_eq!(graph.blocked_by(&a.id), vec![&b.id]);
        }
    }
}
