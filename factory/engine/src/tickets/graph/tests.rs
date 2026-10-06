//! Correctness tests on known fixtures (spec B1-R3 + Gherkin acceptance).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::dependencies::{DependencyEdge, DependencyGraph, DependencyKind};

use super::fixtures::{blocks, chain, diamond, id};
use super::{GraphAnalytics, GraphInsights, InsightsConfig, MetricStatus};

fn phase2_statuses(ins: &GraphInsights) -> Vec<MetricStatus> {
    vec![
        ins.phase2.pagerank.status,
        ins.phase2.betweenness.status,
        ins.phase2.hits.status,
        ins.phase2.critical_path.status,
        ins.phase2.cycles.status,
    ]
}

#[test]
fn diamond_keystone_blocked_and_chokepoints() {
    let engine = GraphAnalytics::new();
    let ins = engine.compute_insights(&diamond(), &InsightsConfig::default());

    // Gherkin: all metrics tagged "computed".
    assert_eq!(
        phase2_statuses(&ins),
        vec![MetricStatus::Computed; 5],
        "diamond is small: phase 2 runs inline, all computed"
    );

    // A ranks as top keystone by critical-path impact.
    let cp = ins.phase2.critical_path.value.as_ref().unwrap();
    let top = cp.impact.iter().max_by_key(|(_, &i)| i).unwrap().0;
    assert_eq!(*top, id("a"));
    assert_eq!(cp.impact[&id("a")], 3);
    assert_eq!(cp.impact[&id("b")], 2);
    assert_eq!(cp.impact[&id("c")], 2);
    assert_eq!(cp.impact[&id("d")], 1);

    // D is the most blocked (highest in-degree).
    let most_blocked = ins.phase1.in_degree.iter().max_by_key(|(_, &d)| d).unwrap().0;
    assert_eq!(*most_blocked, id("d"));
    assert_eq!(ins.phase1.in_degree[&id("d")], 2);

    // B and C tie in betweenness; both are chokepoints on A→D paths.
    let bw = ins.phase2.betweenness.value.as_ref().unwrap();
    assert!(bw[&id("b")] > 0.0);
    assert!((bw[&id("b")] - bw[&id("c")]).abs() < 1e-12, "B and C must tie");
    assert_eq!(bw[&id("a")], 0.0);
    assert_eq!(bw[&id("d")], 0.0);

    // Phase-1 facts are always computed.
    assert_eq!(ins.phase1.node_count, 4);
    assert_eq!(ins.phase1.edge_count, 4);
    assert_eq!(ins.phase1.density, 4.0 / 12.0);
    assert!(ins.phase1.topo_order.is_some());
}

#[test]
fn chain_topological_order_and_impact() {
    let engine = GraphAnalytics::new();
    let ins = engine.compute_insights(&chain(&["a", "b", "c", "d"]), &InsightsConfig::default());

    assert_eq!(
        ins.phase1.topo_order.as_ref().unwrap(),
        &vec![id("a"), id("b"), id("c"), id("d")]
    );

    let cp = ins.phase2.critical_path.value.as_ref().unwrap();
    assert_eq!(cp.longest_chain, vec![id("a"), id("b"), id("c"), id("d")]);
    assert_eq!(cp.impact[&id("a")], 4);
    assert_eq!(cp.impact[&id("d")], 1);

    // Interior nodes of a chain are the chokepoints.
    let bw = ins.phase2.betweenness.value.as_ref().unwrap();
    assert!(bw[&id("b")] > bw[&id("a")]);
    assert!(bw[&id("c")] > bw[&id("d")]);
}

#[test]
fn cycle_fixture_enumerates_the_cycle() {
    let mut g = DependencyGraph::new();
    blocks(&mut g, "a", "b");
    blocks(&mut g, "b", "c");
    blocks(&mut g, "c", "a");

    let engine = GraphAnalytics::new();
    let ins = engine.compute_insights(&g, &InsightsConfig::default());

    let cycles = ins.phase2.cycles.value.as_ref().unwrap();
    assert_eq!(ins.phase2.cycles.status, MetricStatus::Computed);
    assert_eq!(cycles.len(), 1);
    let members: HashSet<_> = cycles[0].iter().collect();
    assert_eq!(members, HashSet::from([&id("a"), &id("b"), &id("c")]));

    // Cyclic graph: no topo order, critical path skipped; flow metrics
    // still compute.
    assert!(ins.phase1.topo_order.is_none());
    assert_eq!(ins.phase2.critical_path.status, MetricStatus::Skipped);
    assert_eq!(ins.phase2.pagerank.status, MetricStatus::Computed);
    assert_eq!(ins.phase2.betweenness.status, MetricStatus::Computed);
}

#[test]
fn large_graph_forced_tiny_timeout_tags_phase2_timeout() {
    // Synthetic >200-node graph: 250-node chain plus extra edges so phase 2
    // has real work to do.
    let mut g = DependencyGraph::new();
    let names: Vec<String> = (0..250).map(|i| format!("n{i:03}")).collect();
    for w in names.windows(2) {
        g.add(DependencyEdge::new(id(&w[0]), id(&w[1]), DependencyKind::Blocks));
    }
    for i in 0..200 {
        g.add(DependencyEdge::new(
            id(&names[i]),
            id(&names[i + 50]),
            DependencyKind::Blocks,
        ));
    }

    let config = InsightsConfig {
        timeout_override: Some(Duration::from_millis(0)),
        ..Default::default()
    };
    let engine = GraphAnalytics::new();
    let ins = engine.compute_insights(&g, &config);

    // Phase 2 timed out; phase 1 still computed.
    assert_eq!(
        phase2_statuses(&ins),
        vec![MetricStatus::Timeout; 5],
        "forced tiny timeout must tag every phase-2 metric"
    );
    assert_eq!(ins.phase1.node_count, 250);
    assert!(ins.phase1.topo_order.is_some());

    // Timeout results are not cached: recomputation is possible.
    assert_eq!(engine.cache_len(), 0);
}

#[test]
fn same_hash_second_call_serves_from_cache() {
    let engine = GraphAnalytics::new();
    let g = diamond();

    let first = engine.compute_insights(&g, &InsightsConfig::default());
    assert_eq!(engine.computation_count(), 1);
    assert_eq!(engine.cache_len(), 1);

    let second = engine.compute_insights(&g, &InsightsConfig::default());
    assert_eq!(engine.computation_count(), 1, "second call must not recompute");
    assert!(Arc::ptr_eq(&first, &second), "second call must serve the cached Arc");
    assert_eq!(first.content_hash, second.content_hash);
}
