//! Pure-Rust graph algorithms over the ticket `blocks` subgraph.
//!
//! No new crates: PageRank, HITS, and Brandes betweenness are implemented
//! in-crate per the spec constraint. Everything here operates on
//! [`GraphView`], a compact index-based projection of
//! [`crate::dependencies::DependencyGraph`] — never on the unrelated
//! `work/graph.rs` DAG.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::dependencies::{DependencyGraph, DependencyKind};
use crate::rails_id::TicketId;

use super::{
    CriticalPath, HitsScores, InsightsConfig, Metric, MetricStatus, Phase1Facts, Phase2Metrics,
};

/// Compact index-based projection of the blocking subgraph.
///
/// Nodes are the endpoints of `blocks` edges, sorted by ticket id for
/// deterministic output; adjacency lists are sorted likewise.
#[derive(Clone, Debug)]
pub struct GraphView {
    pub nodes: Vec<TicketId>,
    index: HashMap<TicketId, usize>,
    /// Outgoing `blocks` edges: tickets this node blocks.
    pub adj: Vec<Vec<usize>>,
    /// Incoming `blocks` edges: tickets blocking this node.
    pub radj: Vec<Vec<usize>>,
}

impl GraphView {
    pub fn from_graph(graph: &DependencyGraph) -> Self {
        let mut node_set: HashSet<TicketId> = HashSet::new();
        let mut edges: Vec<(TicketId, TicketId)> = Vec::new();
        for edge in graph.edges_of_kind(DependencyKind::Blocks) {
            node_set.insert(edge.from.clone());
            node_set.insert(edge.to.clone());
            edges.push((edge.from.clone(), edge.to.clone()));
        }

        let mut nodes: Vec<TicketId> = node_set.into_iter().collect();
        nodes.sort();
        let index: HashMap<TicketId, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();

        let mut adj = vec![Vec::new(); nodes.len()];
        let mut radj = vec![Vec::new(); nodes.len()];
        for (from, to) in edges {
            let (u, v) = (index[&from], index[&to]);
            adj[u].push(v);
            radj[v].push(u);
        }
        for list in adj.iter_mut().chain(radj.iter_mut()) {
            list.sort_unstable();
        }
        Self { nodes, index, adj, radj }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Index of a ticket in the node table, if it participates in the
    /// blocking subgraph.
    pub fn index_of(&self, id: &TicketId) -> Option<usize> {
        self.index.get(id).copied()
    }

    pub fn edge_count(&self) -> usize {
        self.adj.iter().map(Vec::len).sum()
    }

    fn id(&self, i: usize) -> TicketId {
        self.nodes[i].clone()
    }
}

/// blake3 content hash (hex) of the graph's sorted edge set, covering all
/// edge kinds so any graph change invalidates cached insights.
pub fn content_hash(graph: &DependencyGraph) -> String {
    let mut edges: Vec<String> = graph
        .edges()
        .map(|e| format!("{}\t{}\t{}", e.from, e.to, e.kind))
        .collect();
    edges.sort();
    let mut hasher = blake3::Hasher::new();
    for edge in &edges {
        hasher.update(edge.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// Phase 1: degree stats, topological order, density.
pub fn phase1_facts(view: &GraphView) -> Phase1Facts {
    let n = view.node_count();
    let e = view.edge_count();
    let in_degree: HashMap<TicketId, usize> = view
        .nodes
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), view.radj[i].len()))
        .collect();
    let out_degree: HashMap<TicketId, usize> = view
        .nodes
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), view.adj[i].len()))
        .collect();
    Phase1Facts {
        node_count: n,
        edge_count: e,
        density: if n < 2 { 0.0 } else { e as f64 / (n * (n - 1)) as f64 },
        in_degree,
        out_degree,
        topo_order: topological_order(view),
    }
}

/// Kahn's algorithm; `None` when the blocking subgraph is cyclic.
pub fn topological_order(view: &GraphView) -> Option<Vec<TicketId>> {
    let n = view.node_count();
    let mut indeg: Vec<usize> = view.radj.iter().map(Vec::len).collect();
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(u) = queue.pop_front() {
        order.push(u);
        for &v in &view.adj[u] {
            indeg[v] -= 1;
            if indeg[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    if order.len() == n {
        Some(order.into_iter().map(|i| view.id(i)).collect())
    } else {
        None
    }
}

/// Phase 2: PageRank, betweenness, HITS, critical path, cycle enumeration.
pub fn phase2_metrics(view: &GraphView, config: &InsightsConfig) -> Phase2Metrics {
    let (pagerank, pr_converged) = pagerank(
        view,
        config.pagerank_damping,
        config.pagerank_epsilon,
        config.pagerank_max_iterations,
    );
    let (hits, hits_converged) = hits(view, config.hits_epsilon, config.hits_max_iterations);
    let (cycles, capped) = enumerate_cycles(view, config.max_cycles);

    Phase2Metrics {
        pagerank: if pr_converged {
            Metric::computed(score_map(view, pagerank))
        } else {
            Metric { status: MetricStatus::Approx, value: Some(score_map(view, pagerank)) }
        },
        betweenness: Metric::computed(score_map(view, betweenness(view))),
        hits: if hits_converged {
            Metric::computed(hits)
        } else {
            Metric { status: MetricStatus::Approx, value: Some(hits) }
        },
        critical_path: match critical_path(view) {
            Some(cp) => Metric::computed(cp),
            None => Metric::tagged(MetricStatus::Skipped),
        },
        cycles: {
            let cycles = cycles
                .into_iter()
                .map(|c| c.into_iter().map(|i| view.id(i)).collect())
                .collect();
            if capped {
                Metric { status: MetricStatus::Approx, value: Some(cycles) }
            } else {
                Metric::computed(cycles)
            }
        },
    }
}

fn score_map(view: &GraphView, scores: Vec<f64>) -> HashMap<TicketId, f64> {
    view.nodes
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), scores[i]))
        .collect()
}

/// PageRank with damping; dangling nodes redistribute uniformly.
/// Returns (scores, converged).
fn pagerank(view: &GraphView, damping: f64, epsilon: f64, max_iterations: usize) -> (Vec<f64>, bool) {
    let n = view.node_count();
    if n == 0 {
        return (Vec::new(), true);
    }
    let mut rank = vec![1.0 / n as f64; n];
    let mut converged = false;
    for _ in 0..max_iterations {
        let dangling: f64 = (0..n).filter(|&u| view.adj[u].is_empty()).map(|u| rank[u]).sum();
        let base = (1.0 - damping) / n as f64 + damping * dangling / n as f64;
        let mut next = vec![base; n];
        for u in 0..n {
            if view.adj[u].is_empty() {
                continue;
            }
            let share = damping * rank[u] / view.adj[u].len() as f64;
            for &v in &view.adj[u] {
                next[v] += share;
            }
        }
        let delta: f64 = (0..n).map(|i| (next[i] - rank[i]).abs()).sum();
        rank = next;
        if delta < epsilon {
            converged = true;
            break;
        }
    }
    (rank, converged)
}

/// Brandes' algorithm for unweighted directed betweenness centrality.
fn betweenness(view: &GraphView) -> Vec<f64> {
    let n = view.node_count();
    let mut cb = vec![0.0f64; n];
    for s in 0..n {
        let mut stack: Vec<usize> = Vec::with_capacity(n);
        let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut sigma = vec![0.0f64; n];
        let mut dist = vec![-1i64; n];
        sigma[s] = 1.0;
        dist[s] = 0;
        let mut queue = VecDeque::from([s]);
        while let Some(v) = queue.pop_front() {
            stack.push(v);
            for &w in &view.adj[v] {
                if dist[w] < 0 {
                    dist[w] = dist[v] + 1;
                    queue.push_back(w);
                }
                if dist[w] == dist[v] + 1 {
                    sigma[w] += sigma[v];
                    pred[w].push(v);
                }
            }
        }
        let mut delta = vec![0.0f64; n];
        while let Some(w) = stack.pop() {
            for &v in &pred[w] {
                delta[v] += (sigma[v] / sigma[w]) * (1.0 + delta[w]);
            }
            if w != s {
                cb[w] += delta[w];
            }
        }
    }
    cb
}

/// HITS: hubs and authorities, L2-normalized each iteration.
/// Returns (scores, converged).
fn hits(view: &GraphView, epsilon: f64, max_iterations: usize) -> (HitsScores, bool) {
    let n = view.node_count();
    let mut hubs = vec![1.0f64; n];
    let mut auth = vec![1.0f64; n];
    let mut converged = false;
    for _ in 0..max_iterations {
        let mut new_auth = vec![0.0f64; n];
        for v in 0..n {
            for &u in &view.radj[v] {
                new_auth[v] += hubs[u];
            }
        }
        normalize(&mut new_auth);
        let mut new_hubs = vec![0.0f64; n];
        for u in 0..n {
            for &v in &view.adj[u] {
                new_hubs[u] += new_auth[v];
            }
        }
        normalize(&mut new_hubs);
        let delta: f64 = (0..n)
            .map(|i| (new_auth[i] - auth[i]).abs() + (new_hubs[i] - hubs[i]).abs())
            .sum();
        auth = new_auth;
        hubs = new_hubs;
        if delta < epsilon {
            converged = true;
            break;
        }
    }
    (
        HitsScores {
            hubs: score_map(view, hubs),
            authorities: score_map(view, auth),
        },
        converged,
    )
}

fn normalize(v: &mut [f64]) {
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Longest blocking chain and per-node impact (`1 + max downstream`).
/// `None` when the blocking subgraph is cyclic.
fn critical_path(view: &GraphView) -> Option<CriticalPath> {
    let topo = topological_order(view)?;
    let n = view.node_count();
    let mut index_topo = Vec::with_capacity(n);
    for id in &topo {
        index_topo.push(view.index[id]);
    }

    let mut impact = vec![1u64; n];
    let mut next: Vec<Option<usize>> = vec![None; n];
    for &u in index_topo.iter().rev() {
        let mut best = 0u64;
        let mut best_next = None;
        for &v in &view.adj[u] {
            if impact[v] > best {
                best = impact[v];
                best_next = Some(v);
            }
        }
        impact[u] = 1 + best;
        next[u] = best_next;
    }

    let mut longest_chain = Vec::new();
    if let Some((start, _)) = impact.iter().enumerate().max_by_key(|(_, &i)| i) {
        let mut cur = Some(start);
        while let Some(u) = cur {
            longest_chain.push(view.id(u));
            cur = next[u];
        }
    }

    let impact = view
        .nodes
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), impact[i]))
        .collect();
    Some(CriticalPath { longest_chain, impact })
}

/// Enumerate elementary cycles, each reported once from its minimum-index
/// node (path order, start node not repeated at the end). Returns
/// (cycles, capped) — capped when `max_cycles` was hit mid-enumeration.
fn enumerate_cycles(view: &GraphView, max_cycles: usize) -> (Vec<Vec<usize>>, bool) {
    let n = view.node_count();
    let mut cycles = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut on_path = vec![false; n];
    let mut capped = false;

    fn dfs(
        u: usize,
        start: usize,
        view: &GraphView,
        stack: &mut Vec<usize>,
        on_path: &mut [bool],
        cycles: &mut Vec<Vec<usize>>,
        max_cycles: usize,
        capped: &mut bool,
    ) {
        if *capped {
            return;
        }
        stack.push(u);
        on_path[u] = true;
        for &v in &view.adj[u] {
            if v == start {
                if stack.len() >= 2 {
                    cycles.push(stack.clone());
                    if cycles.len() >= max_cycles {
                        *capped = true;
                    }
                }
            } else if v > start && !on_path[v] {
                dfs(v, start, view, stack, on_path, cycles, max_cycles, capped);
                if *capped {
                    break;
                }
            }
        }
        stack.pop();
        on_path[u] = false;
    }

    for start in 0..n {
        dfs(
            start,
            start,
            view,
            &mut stack,
            &mut on_path,
            &mut cycles,
            max_cycles,
            &mut capped,
        );
        if capped {
            break;
        }
    }
    (cycles, capped)
}
