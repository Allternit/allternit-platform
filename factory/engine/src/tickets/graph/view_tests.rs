//! View-model tests for the B2 robot surface (spec B2 Gherkin):
//! diamond keystones/most-blocked/quick-wins, triage scoring + bounding,
//! impact view incl. unknown-id error.

use super::fixtures::{diamond, id, ticket};
use super::views::{
    self, build_impact_view, build_insights_view, build_triage_view, ViewError, MAX_TRIAGE_ITEMS,
};
use super::{GraphAnalytics, GraphView, InsightsConfig, MetricStatus};

fn compute(graph: &crate::dependencies::DependencyGraph) -> std::sync::Arc<super::GraphInsights> {
    GraphAnalytics::new().compute_insights(graph, &InsightsConfig::default())
}

#[test]
fn diamond_insights_view_keystone_most_blocked_quick_wins() {
    let graph = diamond();
    let insights = compute(&graph);
    let view = GraphView::from_graph(&graph);
    let tickets = vec![ticket("a"), ticket("b"), ticket("c"), ticket("d")];
    // Only A is ready: B,C are blocked by A, D by B and C.
    let ready = vec![ticket("a")];

    let out = build_insights_view(&insights, &ready, &view);

    // Gherkin: A ranks as top keystone by critical-path impact.
    assert_eq!(out.keystones[0].ticket, id("a"));
    assert_eq!(out.keystones[0].impact, 3);

    // Gherkin: D the most blocked.
    assert_eq!(out.health.most_blocked, Some(id("d")));

    // Gherkin: all metrics "computed" (small graph runs inline).
    let statuses = &out.metric_statuses;
    assert_eq!(
        (
            statuses.pagerank,
            statuses.betweenness,
            statuses.hits,
            statuses.critical_path,
            statuses.cycles,
        ),
        (
            MetricStatus::Computed,
            MetricStatus::Computed,
            MetricStatus::Computed,
            MetricStatus::Computed,
            MetricStatus::Computed,
        )
    );

    // Health summary sanity.
    assert_eq!(out.health.node_count, 4);
    assert_eq!(out.health.edge_count, 4);
    assert!(!out.health.cyclic);
    assert_eq!(out.health.cycle_count, Some(0));
    assert_eq!(out.health.longest_chain_len, Some(3));
    assert_eq!(out.health.ready_count, 1);
    assert_eq!(out.content_hash, insights.content_hash);

    // B and C are the chokepoints.
    assert_eq!(out.bottlenecks.len(), 2);
    assert_eq!(out.bottlenecks[0].ticket, id("b"));
    assert_eq!(out.bottlenecks[1].ticket, id("c"));

    // A blocks the whole diamond (impact 3): not a quick win.
    assert!(out.quick_wins.is_empty());

    // An isolated ready ticket blocks nothing: it is the quick win.
    let ready = vec![ticket("a"), ticket("e")];
    let out = build_insights_view(&insights, &ready, &view);
    assert_eq!(out.quick_wins.len(), 1);
    assert_eq!(out.quick_wins[0].ticket, id("e"));
    assert_eq!(out.quick_wins[0].downstream_impact, 1);
    let _ = tickets;
}

#[test]
fn diamond_triage_view_scores_and_reasons() {
    let graph = diamond();
    let insights = compute(&graph);
    let view = GraphView::from_graph(&graph);
    let ready = vec![ticket("a")];

    let out = build_triage_view(&insights, &ready, &view);

    assert_eq!(out.ready_count, 1);
    assert_eq!(out.items.len(), 1);
    let item = &out.items[0];
    assert_eq!(item.ticket, id("a"));
    // A transitively unblocks B, C, D.
    assert_eq!(item.unblocks, 3);
    // score = 3*10 (unblocks) + 3 (impact) + 2 (P2 boost).
    assert_eq!(item.score, 35);
    assert!(item.reason.contains("unblocks 3 tickets"), "{}", item.reason);
    assert!(item.reason.contains("impact 3"), "{}", item.reason);
}

#[test]
fn triage_body_bounded_on_500_ticket_graph() {
    // 500-ticket chain; every ticket ready (worst case for body size).
    let names: Vec<String> = (0..500).map(|i| format!("n{i:03}")).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let graph = super::fixtures::chain(&name_refs);
    let insights = compute(&graph);
    let view = GraphView::from_graph(&graph);
    let ready: Vec<_> = names.iter().map(|n| ticket(n)).collect();

    let out = build_triage_view(&insights, &ready, &view);

    assert_eq!(out.ready_count, 500);
    assert!(out.items.len() <= MAX_TRIAGE_ITEMS);

    // Compact serialization (the HTTP shape) must stay under 16KB.
    let body = serde_json::to_string(&out).unwrap();
    assert!(
        body.len() < 16 * 1024,
        "triage body is {} bytes, must be < 16384",
        body.len()
    );

    // Ranking is by downstream reach: the chain head wins.
    assert_eq!(out.items[0].ticket, id("n000"));
    assert_eq!(out.items[0].unblocks, 499);
}

#[test]
fn impact_view_direct_transitive_impact_and_ranks() {
    let graph = diamond();
    let insights = compute(&graph);
    let view = GraphView::from_graph(&graph);
    let tickets = vec![ticket("a"), ticket("b"), ticket("c"), ticket("d"), ticket("e")];

    let out = build_impact_view(&insights, &view, &tickets, &id("a")).unwrap();
    assert_eq!(out.critical_path_impact, Some(3));
    assert_eq!(out.direct_dependents, vec![id("b"), id("c")]);
    assert_eq!(out.transitive_dependents, vec![id("b"), id("c"), id("d")]);
    // D accumulates the most PageRank; A the least.
    assert_eq!(out.pagerank_rank, Some(4));
    assert!(out.betweenness_rank.is_some());

    let tail = build_impact_view(&insights, &view, &tickets, &id("d")).unwrap();
    assert_eq!(tail.critical_path_impact, Some(1));
    assert!(tail.direct_dependents.is_empty());
    assert!(tail.transitive_dependents.is_empty());
    assert_eq!(tail.pagerank_rank, Some(1));

    // Known ticket with no edges: empty impact, no centrality ranks.
    let edgeless = build_impact_view(&insights, &view, &tickets, &id("e")).unwrap();
    assert_eq!(edgeless.critical_path_impact, None);
    assert!(edgeless.transitive_dependents.is_empty());
    assert_eq!(edgeless.pagerank_rank, None);
    assert_eq!(edgeless.betweenness_rank, None);
}

#[test]
fn impact_view_unknown_ticket_errors() {
    let graph = diamond();
    let insights = compute(&graph);
    let view = GraphView::from_graph(&graph);
    let tickets = vec![ticket("a"), ticket("b"), ticket("c"), ticket("d")];

    let err = build_impact_view(&insights, &view, &tickets, &id("zz")).unwrap_err();
    assert_eq!(err, ViewError::UnknownTicket(id("zz")));
    assert_eq!(err.to_string(), "unknown ticket: T-zz");
}

#[test]
fn insights_view_serializes_fixed_shape() {
    let graph = diamond();
    let insights = compute(&graph);
    let view = GraphView::from_graph(&graph);
    let ready = vec![ticket("a")];

    let value = serde_json::to_value(build_insights_view(&insights, &ready, &view)).unwrap();
    for key in [
        "content_hash",
        "metric_statuses",
        "health",
        "keystones",
        "bottlenecks",
        "quick_wins",
    ] {
        assert!(value.get(key).is_some(), "missing key {key}");
    }
    assert_eq!(value["metric_statuses"]["pagerank"], "computed");

    // The views module is the single implementation both surfaces use.
    let _ = views::MAX_KEYSTONES;
}
