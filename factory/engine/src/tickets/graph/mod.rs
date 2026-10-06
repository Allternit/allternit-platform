//! Graph analytics for the Rails **ticket dependency graph**.
//!
//! This module operates exclusively on the ticket dependency graph in
//! [`crate::dependencies`] ([`DependencyGraph`]) — specifically the
//! `blocks` edge subgraph that `tickets::ready`/`blocked` derive readiness
//! from.
//!
//! BUILDER TRAP: this has nothing to do with `rails/src/work/graph.rs`.
//! That file is the unrelated work/DAG system (WIH nodes, `ready_nodes`,
//! `would_create_cycle`) with deceptively similar function names. Do not
//! touch, reuse, or "unify" it from here.
//!
//! Two-phase orchestration (spec B1-R2):
//!
//! - Phase 1 (instant): degree stats, topological order, density.
//! - Phase 2 (heavy): PageRank, Brandes betweenness, HITS, critical path,
//!   cycle enumeration. For large graphs (nodes > 200 or edges > 1000)
//!   phase 2 runs off the calling thread with a size-aware timeout of
//!   `min(500ms + 1ms/edge, 5s)`; below that threshold it runs inline and
//!   is still tagged [`MetricStatus::Computed`].
//!
//! Results are cached in a `Mutex<HashMap<hash, Arc<GraphInsights>>>`
//! keyed by a blake3 content hash of the sorted edge set. Pure Rust, no
//! new crates.

mod algorithms;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod view_tests;
pub mod views;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::dependencies::DependencyGraph;
use crate::rails_id::TicketId;

pub use algorithms::GraphView;

/// Default node count above which phase 2 moves off-thread with a timeout.
pub const DEFAULT_LARGE_GRAPH_NODES: usize = 200;
/// Default edge count above which phase 2 moves off-thread with a timeout.
pub const DEFAULT_LARGE_GRAPH_EDGES: usize = 1000;
/// PageRank damping factor.
pub const DEFAULT_PAGERANK_DAMPING: f64 = 0.85;

/// Per-metric status tag (spec B1-R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricStatus {
    /// Metric was fully computed.
    Computed,
    /// Metric was computed but is approximate (iteration cap hit, cycle
    /// enumeration truncated).
    Approx,
    /// Phase 2 exceeded the size-aware timeout; no value available.
    Timeout,
    /// Metric does not apply to this graph (e.g. critical path on a
    /// cyclic graph).
    Skipped,
}

/// A metric value plus its status tag.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric<T> {
    pub status: MetricStatus,
    pub value: Option<T>,
}

impl<T> Metric<T> {
    fn computed(value: T) -> Self {
        Self { status: MetricStatus::Computed, value: Some(value) }
    }

    fn tagged(status: MetricStatus) -> Self {
        Self { status, value: None }
    }
}

/// Phase-1 instant static facts about the blocking subgraph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Phase1Facts {
    /// Tickets participating in at least one `blocks` edge.
    pub node_count: usize,
    /// Number of `blocks` edges.
    pub edge_count: usize,
    /// `E / (N * (N - 1))` over the blocking subgraph; 0 for N < 2.
    pub density: f64,
    /// Per-ticket count of incoming `blocks` edges (tickets blocking it).
    pub in_degree: HashMap<TicketId, usize>,
    /// Per-ticket count of outgoing `blocks` edges (tickets it blocks).
    pub out_degree: HashMap<TicketId, usize>,
    /// Topological order of the blocking subgraph (blockers first), or
    /// `None` when the subgraph is cyclic.
    pub topo_order: Option<Vec<TicketId>>,
}

impl Phase1Facts {
    pub const STATUS: MetricStatus = MetricStatus::Computed;
}

/// HITS scores: hub epics vs authority utilities.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HitsScores {
    pub hubs: HashMap<TicketId, f64>,
    pub authorities: HashMap<TicketId, f64>,
}

/// Critical path through the blocking subgraph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CriticalPath {
    /// The longest blocking chain, blockers first.
    pub longest_chain: Vec<TicketId>,
    /// Per-ticket impact: `1 + max(impact of tickets it directly blocks)`.
    pub impact: HashMap<TicketId, u64>,
}

/// Phase-2 heavy metrics, each individually status-tagged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Phase2Metrics {
    /// Load-bearing tickets.
    pub pagerank: Metric<HashMap<TicketId, f64>>,
    /// Chokepoints (Brandes' algorithm).
    pub betweenness: Metric<HashMap<TicketId, f64>>,
    /// Hub epics vs authority utilities.
    pub hits: Metric<HitsScores>,
    /// Longest blocking chain + per-ticket impact.
    pub critical_path: Metric<CriticalPath>,
    /// Elementary cycles in the blocking subgraph.
    pub cycles: Metric<Vec<Vec<TicketId>>>,
}

impl Phase2Metrics {
    fn timeout() -> Self {
        Self {
            pagerank: Metric::tagged(MetricStatus::Timeout),
            betweenness: Metric::tagged(MetricStatus::Timeout),
            hits: Metric::tagged(MetricStatus::Timeout),
            critical_path: Metric::tagged(MetricStatus::Timeout),
            cycles: Metric::tagged(MetricStatus::Timeout),
        }
    }
}

/// The full analytics snapshot for one graph content hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphInsights {
    /// blake3 hash of the sorted edge set (hex).
    pub content_hash: String,
    /// Phase-1 facts; always [`MetricStatus::Computed`].
    pub phase1: Phase1Facts,
    /// Phase-2 heavy metrics with per-metric status tags.
    pub phase2: Phase2Metrics,
    pub computed_at: DateTime<Utc>,
}

/// Tunables for [`GraphAnalytics::compute_insights`].
#[derive(Clone, Debug)]
pub struct InsightsConfig {
    /// Phase 2 goes off-thread when nodes exceed this. Default 200.
    pub large_graph_nodes: usize,
    /// Phase 2 goes off-thread when edges exceed this. Default 1000.
    pub large_graph_edges: usize,
    /// PageRank damping factor. Default 0.85.
    pub pagerank_damping: f64,
    /// PageRank convergence epsilon (L1 delta). Default 1e-6.
    pub pagerank_epsilon: f64,
    /// PageRank iteration cap; hitting it tags the metric `Approx`.
    pub pagerank_max_iterations: usize,
    /// HITS convergence epsilon (L1 delta). Default 1e-6.
    pub hits_epsilon: f64,
    /// HITS iteration cap; hitting it tags the metric `Approx`.
    pub hits_max_iterations: usize,
    /// Override the size-aware phase-2 timeout. `None` uses
    /// `min(500ms + 1ms/edge, 5s)`.
    pub timeout_override: Option<Duration>,
    /// Cycle enumeration cap; hitting it tags the metric `Approx`.
    pub max_cycles: usize,
}

impl Default for InsightsConfig {
    fn default() -> Self {
        Self {
            large_graph_nodes: DEFAULT_LARGE_GRAPH_NODES,
            large_graph_edges: DEFAULT_LARGE_GRAPH_EDGES,
            pagerank_damping: DEFAULT_PAGERANK_DAMPING,
            pagerank_epsilon: 1e-6,
            pagerank_max_iterations: 100,
            hits_epsilon: 1e-6,
            hits_max_iterations: 100,
            timeout_override: None,
            max_cycles: 1000,
        }
    }
}

impl InsightsConfig {
    /// Size-aware phase-2 timeout: `min(500ms + 1ms/blocking-edge, 5s)`.
    pub fn phase2_timeout(&self, edge_count: usize) -> Duration {
        if let Some(t) = self.timeout_override {
            return t;
        }
        let ms = 500u64.saturating_add(edge_count as u64).min(5000);
        Duration::from_millis(ms)
    }
}

/// Analytics engine: two-phase orchestration plus a content-hash cache.
pub struct GraphAnalytics {
    cache: Mutex<HashMap<String, Arc<GraphInsights>>>,
    computations: AtomicUsize,
}

impl Default for GraphAnalytics {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphAnalytics {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            computations: AtomicUsize::new(0),
        }
    }

    /// Number of times insights were actually computed (cache misses).
    pub fn computation_count(&self) -> usize {
        self.computations.load(Ordering::SeqCst)
    }

    /// Number of cached insight snapshots.
    pub fn cache_len(&self) -> usize {
        self.cache.lock().expect("insights cache poisoned").len()
    }

    /// Two-phase orchestration per spec B1-R2.
    ///
    /// Phase 1 always runs inline and is tagged `Computed`. Phase 2 runs
    /// inline (tagged `Computed`) for graphs at or below the size
    /// thresholds; above them it runs on a spawned thread bounded by the
    /// size-aware timeout, tagging every phase-2 metric `Timeout` on
    /// expiry. Timeout results are NOT cached, so a later call with a
    /// viable budget recomputes.
    pub fn compute_insights(
        &self,
        graph: &DependencyGraph,
        config: &InsightsConfig,
    ) -> Arc<GraphInsights> {
        let content_hash = algorithms::content_hash(graph);
        if let Some(hit) = self.cache.lock().expect("insights cache poisoned").get(&content_hash)
        {
            return Arc::clone(hit);
        }

        let view = GraphView::from_graph(graph);
        let phase1 = algorithms::phase1_facts(&view);

        let blocking_edges = view.edge_count();
        let large = view.node_count() > config.large_graph_nodes
            || blocking_edges > config.large_graph_edges;
        let timeout = config.phase2_timeout(blocking_edges);

        let (phase2, timed_out) = if large {
            let worker_view = view.clone();
            let worker_config = config.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(algorithms::phase2_metrics(&worker_view, &worker_config));
            });
            match rx.recv_timeout(timeout) {
                Ok(metrics) => (metrics, false),
                Err(_) => (Phase2Metrics::timeout(), true),
            }
        } else {
            (algorithms::phase2_metrics(&view, config), false)
        };

        let insights = Arc::new(GraphInsights {
            content_hash,
            phase1,
            phase2,
            computed_at: Utc::now(),
        });
        self.computations.fetch_add(1, Ordering::SeqCst);
        if !timed_out {
            self.cache
                .lock()
                .expect("insights cache poisoned")
                .insert(insights.content_hash.clone(), Arc::clone(&insights));
        }
        insights
    }
}
