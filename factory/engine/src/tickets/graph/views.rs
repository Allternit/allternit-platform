//! Shared view-models for the B2 robot surface (spec B2-R1 + B2-R2).
//!
//! These builders turn [`GraphInsights`] (B1 analytics) plus the ready list
//! into the fixed-shape JSON served by both the HTTP handlers
//! (`cmd/allternit-api/src/rails/mod.rs`) and the `rails graph` CLI
//! subcommands. One implementation, two surfaces: both serialize these
//! structs, so CLI/HTTP parity holds by construction.
//!
//! Everything here is pure: builders take `&GraphInsights`, the ready
//! `&[Ticket]` list, and a [`GraphView`] projection — no stores, no I/O —
//! so they are unit-testable over fixtures. Like the rest of `graph/`, this
//! module operates on the ticket `blocks` subgraph, never the unrelated
//! `work/graph.rs` DAG.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::rails_id::TicketId;
use crate::tickets::{Ticket, TicketPriority};

use super::{GraphInsights, GraphView, MetricStatus};

/// Cap on keystones in the insights view.
pub const MAX_KEYSTONES: usize = 10;
/// Cap on bottlenecks in the insights view.
pub const MAX_BOTTLENECKS: usize = 10;
/// Cap on quick wins in the insights view.
pub const MAX_QUICK_WINS: usize = 10;
/// Cap on ranked triage items; sized so a 500-ticket graph's serialized
/// triage body stays well under 16KB.
pub const MAX_TRIAGE_ITEMS: usize = 50;
/// Titles are truncated to this many chars to bound serialized size.
const MAX_TEXT_LEN: usize = 64;
/// A ready ticket is a quick win when its downstream impact is at most this
/// (1 = nothing depends on it).
const QUICK_WIN_MAX_IMPACT: u64 = 1;

/// Per-metric phase-2 status tags, flattened for the robot surface.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricStatuses {
    pub pagerank: MetricStatus,
    pub betweenness: MetricStatus,
    pub hits: MetricStatus,
    pub critical_path: MetricStatus,
    pub cycles: MetricStatus,
}

impl MetricStatuses {
    pub fn from_insights(insights: &GraphInsights) -> Self {
        Self {
            pagerank: insights.phase2.pagerank.status,
            betweenness: insights.phase2.betweenness.status,
            hits: insights.phase2.hits.status,
            critical_path: insights.phase2.critical_path.status,
            cycles: insights.phase2.cycles.status,
        }
    }
}

/// Phase-1 health summary over the blocking subgraph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthSummary {
    pub node_count: usize,
    pub edge_count: usize,
    pub density: f64,
    /// True when the blocking subgraph is cyclic (no topological order).
    pub cyclic: bool,
    /// Number of elementary cycles, when the cycles metric has a value.
    pub cycle_count: Option<usize>,
    /// Length of the longest blocking chain, when critical path computed.
    pub longest_chain_len: Option<usize>,
    /// Ticket with the most incoming `blocks` edges, if any edge exists.
    pub most_blocked: Option<TicketId>,
    /// Number of currently-ready tickets (from the passed-in ready list).
    pub ready_count: usize,
}

/// A load-bearing ticket ranked by critical-path impact.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Keystone {
    pub ticket: TicketId,
    pub impact: u64,
}

/// A chokepoint ranked by Brandes betweenness.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bottleneck {
    pub ticket: TicketId,
    pub betweenness: f64,
}

/// A ready ticket with low downstream impact: safe to knock out first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuickWin {
    pub ticket: TicketId,
    pub title: String,
    pub priority: TicketPriority,
    pub downstream_impact: u64,
}

/// `GET /api/rails/graph/insights` / `rails graph insights` body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InsightsView {
    pub content_hash: String,
    pub metric_statuses: MetricStatuses,
    pub health: HealthSummary,
    pub keystones: Vec<Keystone>,
    pub bottlenecks: Vec<Bottleneck>,
    pub quick_wins: Vec<QuickWin>,
}

/// One ranked ready ticket in the triage view.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriageItem {
    pub ticket: TicketId,
    pub title: String,
    pub score: u64,
    pub reason: String,
    /// Tickets directly or indirectly unblocked by completing this one.
    pub unblocks: usize,
}

/// `GET /api/rails/graph/triage` / `rails graph triage` body. Bounded:
/// `items` is capped at [`MAX_TRIAGE_ITEMS`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriageView {
    pub content_hash: String,
    pub metric_statuses: MetricStatuses,
    /// Total ready tickets before the `items` cap was applied.
    pub ready_count: usize,
    pub items: Vec<TriageItem>,
}

/// `GET /api/rails/graph/impact/:ticket_id` / `rails graph impact` body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImpactView {
    pub content_hash: String,
    pub metric_statuses: MetricStatuses,
    pub ticket: TicketId,
    /// Critical-path impact of this ticket, when the metric computed.
    pub critical_path_impact: Option<u64>,
    /// Tickets this ticket directly blocks.
    pub direct_dependents: Vec<TicketId>,
    /// All tickets reachable via `blocks` edges (includes direct), sorted.
    pub transitive_dependents: Vec<TicketId>,
    /// 1-based rank by PageRank among graph nodes, when available.
    pub pagerank_rank: Option<usize>,
    /// 1-based rank by betweenness among graph nodes, when available.
    pub betweenness_rank: Option<usize>,
}

/// Errors the view builders can return; the HTTP surface maps
/// [`ViewError::UnknownTicket`] to 404.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    UnknownTicket(TicketId),
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewError::UnknownTicket(id) => write!(f, "unknown ticket: {id}"),
        }
    }
}

impl std::error::Error for ViewError {}

/// Build the insights view: health summary + keystones + bottlenecks +
/// quick wins. `ready` is the shared A1 ready list for the same graph.
pub fn build_insights_view(
    insights: &GraphInsights,
    ready: &[Ticket],
    view: &GraphView,
) -> InsightsView {
    let health = HealthSummary {
        node_count: insights.phase1.node_count,
        edge_count: insights.phase1.edge_count,
        density: insights.phase1.density,
        cyclic: insights.phase1.topo_order.is_none(),
        cycle_count: insights
            .phase2
            .cycles
            .value
            .as_ref()
            .map(|cycles| cycles.len()),
        longest_chain_len: insights
            .phase2
            .critical_path
            .value
            .as_ref()
            .map(|cp| cp.longest_chain.len()),
        most_blocked: insights
            .phase1
            .in_degree
            .iter()
            .filter(|(_, &d)| d > 0)
            .max_by(|(a_id, a_d), (b_id, b_d)| a_d.cmp(b_d).then(b_id.cmp(a_id)))
            .map(|(id, _)| id.clone()),
        ready_count: ready.len(),
    };

    let keystones = insights
        .phase2
        .critical_path
        .value
        .as_ref()
        .map(|cp| {
            let mut ranked: Vec<(&TicketId, &u64)> = cp.impact.iter().collect();
            ranked.sort_by(|(a_id, a_i), (b_id, b_i)| b_i.cmp(a_i).then(a_id.cmp(b_id)));
            ranked
                .into_iter()
                .take(MAX_KEYSTONES)
                .map(|(id, &impact)| Keystone { ticket: id.clone(), impact })
                .collect()
        })
        .unwrap_or_default();

    let bottlenecks = insights
        .phase2
        .betweenness
        .value
        .as_ref()
        .map(|bw| {
            let mut ranked: Vec<(&TicketId, &f64)> =
                bw.iter().filter(|(_, &b)| b > 0.0).collect();
            ranked.sort_by(|(a_id, a_b), (b_id, b_b)| {
                b_b.partial_cmp(a_b).unwrap_or(std::cmp::Ordering::Equal).then(a_id.cmp(b_id))
            });
            ranked
                .into_iter()
                .take(MAX_BOTTLENECKS)
                .map(|(id, &betweenness)| Bottleneck { ticket: id.clone(), betweenness })
                .collect()
        })
        .unwrap_or_default();

    let mut quick_wins: Vec<QuickWin> = ready
        .iter()
        .filter_map(|t| {
            let impact = downstream_impact(insights, view, &t.id);
            (impact <= QUICK_WIN_MAX_IMPACT).then(|| QuickWin {
                ticket: t.id.clone(),
                title: truncate(&t.title),
                priority: t.priority,
                downstream_impact: impact,
            })
        })
        .collect();
    quick_wins.sort_by(|a, b| {
        a.priority
            .level()
            .cmp(&b.priority.level())
            .then(a.ticket.cmp(&b.ticket))
    });
    quick_wins.truncate(MAX_QUICK_WINS);

    InsightsView {
        content_hash: insights.content_hash.clone(),
        metric_statuses: MetricStatuses::from_insights(insights),
        health,
        keystones,
        bottlenecks,
        quick_wins,
    }
}

/// Build the triage view: ready tickets ranked by a deterministic score
/// (unblocks × 10 + critical-path impact + priority boost), each with a
/// human-readable reason and transitive unblock count. The item list is
/// capped at [`MAX_TRIAGE_ITEMS`] to bound the serialized body.
pub fn build_triage_view(
    insights: &GraphInsights,
    ready: &[Ticket],
    view: &GraphView,
) -> TriageView {
    let impact_of = |id: &TicketId| -> Option<u64> {
        insights
            .phase2
            .critical_path
            .value
            .as_ref()
            .map(|cp| cp.impact.get(id).copied().unwrap_or(1))
    };

    let mut items: Vec<TriageItem> = ready
        .iter()
        .map(|t| {
            let unblocks = transitive_dependents(view, &t.id).len();
            let impact = impact_of(&t.id);
            let priority_boost = 4u64.saturating_sub(t.priority.level() as u64);
            let score = unblocks as u64 * 10 + impact.unwrap_or(0) + priority_boost;
            let reason = match impact {
                Some(impact) => format!(
                    "unblocks {unblocks} ticket{}; critical-path impact {impact}; priority {}",
                    if unblocks == 1 { "" } else { "s" },
                    t.priority,
                ),
                None => format!(
                    "unblocks {unblocks} ticket{}; priority {} (impact unavailable: {})",
                    if unblocks == 1 { "" } else { "s" },
                    t.priority,
                    status_tag(insights.phase2.critical_path.status),
                ),
            };
            TriageItem {
                ticket: t.id.clone(),
                title: truncate(&t.title),
                score,
                reason,
                unblocks,
            }
        })
        .collect();
    items.sort_by(|a, b| b.score.cmp(&a.score).then(a.ticket.cmp(&b.ticket)));
    items.truncate(MAX_TRIAGE_ITEMS);

    TriageView {
        content_hash: insights.content_hash.clone(),
        metric_statuses: MetricStatuses::from_insights(insights),
        ready_count: ready.len(),
        items,
    }
}

/// Build the impact view for one ticket. `tickets` is the full ticket list
/// (a ticket with no edges is known but has empty dependents);
/// [`ViewError::UnknownTicket`] when the id is in neither the store nor the
/// graph.
pub fn build_impact_view(
    insights: &GraphInsights,
    view: &GraphView,
    tickets: &[Ticket],
    ticket: &TicketId,
) -> Result<ImpactView, ViewError> {
    let known =
        tickets.iter().any(|t| &t.id == ticket) || view.index_of(ticket).is_some();
    if !known {
        return Err(ViewError::UnknownTicket(ticket.clone()));
    }

    let critical_path_impact = insights
        .phase2
        .critical_path
        .value
        .as_ref()
        .and_then(|cp| cp.impact.get(ticket).copied());

    Ok(ImpactView {
        content_hash: insights.content_hash.clone(),
        metric_statuses: MetricStatuses::from_insights(insights),
        ticket: ticket.clone(),
        critical_path_impact,
        direct_dependents: direct_dependents(view, ticket),
        transitive_dependents: transitive_dependents(view, ticket),
        pagerank_rank: rank_of(insights.phase2.pagerank.value.as_ref(), ticket),
        betweenness_rank: rank_of(insights.phase2.betweenness.value.as_ref(), ticket),
    })
}

/// Downstream impact of a ticket: critical-path impact when that metric has
/// a value, otherwise a coarse fallback from the graph shape (1 when the
/// ticket blocks nothing, `u64::MAX` when it does but the metric is
/// unavailable).
fn downstream_impact(insights: &GraphInsights, view: &GraphView, id: &TicketId) -> u64 {
    if let Some(cp) = insights.phase2.critical_path.value.as_ref() {
        return cp.impact.get(id).copied().unwrap_or(1);
    }
    match view.index_of(id) {
        Some(i) if !view.adj[i].is_empty() => u64::MAX,
        _ => 1,
    }
}

/// Tickets directly blocked by `id` (outgoing `blocks` edges), sorted.
fn direct_dependents(view: &GraphView, id: &TicketId) -> Vec<TicketId> {
    match view.index_of(id) {
        Some(i) => view.adj[i].iter().map(|&j| view.nodes[j].clone()).collect(),
        None => Vec::new(),
    }
}

/// All tickets reachable from `id` via `blocks` edges, sorted, `id`
/// excluded. Empty when `id` is not a graph node.
fn transitive_dependents(view: &GraphView, id: &TicketId) -> Vec<TicketId> {
    let Some(start) = view.index_of(id) else {
        return Vec::new();
    };
    let mut seen: HashSet<usize> = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    let mut out = Vec::new();
    while let Some(u) = queue.pop_front() {
        for &v in &view.adj[u] {
            if seen.insert(v) {
                out.push(view.nodes[v].clone());
                queue.push_back(v);
            }
        }
    }
    out.sort();
    out
}

/// 1-based rank of `id` in a score map (descending score, id tiebreak);
/// `None` when the metric has no value or the id is absent.
fn rank_of(map: Option<&HashMap<TicketId, f64>>, id: &TicketId) -> Option<usize> {
    let map = map?;
    if !map.contains_key(id) {
        return None;
    }
    let mut ranked: Vec<(&TicketId, &f64)> = map.iter().collect();
    ranked.sort_by(|(a_id, a_s), (b_id, b_s)| {
        b_s.partial_cmp(a_s).unwrap_or(std::cmp::Ordering::Equal).then(a_id.cmp(b_id))
    });
    ranked.iter().position(|(rid, _)| *rid == id).map(|p| p + 1)
}

fn status_tag(status: MetricStatus) -> &'static str {
    match status {
        MetricStatus::Computed => "computed",
        MetricStatus::Approx => "approx",
        MetricStatus::Timeout => "timeout",
        MetricStatus::Skipped => "skipped",
    }
}

/// Char-safe truncation for bounding serialized size.
fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_TEXT_LEN {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_TEXT_LEN - 1).collect();
    out.push('…');
    out
}
