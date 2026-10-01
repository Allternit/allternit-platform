//! WP-X1: task-type graphs, completion contracts and eval sets.

use super::runner::{run_scripted, Script, Status};
use super::*;
use crate::agency_api::{catalog, compiler::TemplateRegistry};
use allternit_commrails::kernel::router::Role;
use serde_json::{json, Value};

const EVALS: &[&str] = &[
    include_str!("evals/general_task.v1.json"),
    include_str!("evals/thread_work.v1.json"),
    include_str!("evals/computer_use.v1.json"),
    include_str!("evals/research_doc.v1.json"),
    include_str!("evals/template.v1.json"),
    include_str!("evals/campaign.v1.json"),
];

fn sample_resources(t: &TaskType) -> Vec<String> {
    match t.write_scheme {
        None => vec![],
        Some("") => vec!["template:sample".into()],
        Some(s) => vec![format!("{s}:sample")],
    }
}

#[test]
fn every_graph_instantiates_and_passes_the_kernel_validator() {
    for t in TASK_TYPES {
        let g = instantiate(t, "task.x", &sample_resources(t)).unwrap_or_else(|e| panic!("{}: {e}", t.id));
        assert_eq!(g.graph_id, t.graph_id);
        assert_eq!(g.completion_nodes, vec!["persist".to_string()], "{}", t.id);
        // Writers carry the declared resources; nothing else writes.
        for n in &g.nodes {
            if n.write_set.is_empty() { continue; }
            assert_eq!(n.write_set, sample_resources(t), "{} {}", t.id, n.node_id);
            assert!(n.capability_request.is_some(), "{} {} writes without a capability request", t.id, n.node_id);
            assert_eq!(n.cognitive_role.as_deref(), Some("S0"), "{} {}: only deterministic nodes write", t.id, n.node_id);
        }
    }
}

#[test]
fn writing_graphs_fail_closed_without_declared_resources() {
    for t in TASK_TYPES {
        match t.write_scheme {
            None => assert!(instantiate(t, "task.x", &["fs:x".into()]).is_err(), "{} must refuse writes", t.id),
            Some(_) => {
                assert!(instantiate(t, "task.x", &[]).is_err(), "{}", t.id);
                assert!(instantiate(t, "task.x", &["nocolon".into()]).is_err(), "{}", t.id);
            }
        }
    }
    let thread = get("THREAD_WORK").unwrap();
    assert!(instantiate(thread, "task.x", &["fs:/etc/passwd".into()]).is_err(), "scheme must match");
}

#[test]
fn completion_contract_matches_the_verifier_gate() {
    for t in TASK_TYPES {
        let g = graph(t).unwrap();
        let policy = completion_policy(t).unwrap_or_else(|| panic!("{} has no contract", t.id));
        assert!(!policy.allow_partial, "{}", t.id);
        let crit: Vec<String> = policy.require.iter().map(|r| r.criterion_id.clone()).collect();
        assert!(policy.require.iter().all(|r| r.blocking), "{}", t.id);

        let gate = g.node("gate").unwrap();
        assert_eq!(gate.node_kind, "VERIFY");
        assert_eq!(gate.cognitive_role.as_deref(), Some("S0"), "{}: the gate is deterministic", t.id);
        assert_eq!(gate.evidence_required, crit, "{}: gate requires exactly the contract", t.id);
        assert_eq!(g.node("persist").unwrap().evidence_required, crit, "{}", t.id);

        // Every criterion has exactly one producer, and it's a verifier-side node.
        for c in &crit {
            let producers: Vec<_> = g.nodes.iter().filter(|n| evidence_of(n) == Some(c.as_str())).collect();
            assert_eq!(producers.len(), 1, "{}: criterion {c} needs one producer", t.id);
            let p = producers[0];
            assert!(matches!(p.node_kind.as_str(), "VERIFY" | "POLICY" | "WAIT" | "CONTROL"),
                "{}: {c} produced by {} node {}", t.id, p.node_kind, p.node_id);
        }
        // No node claims a criterion outside the contract.
        for n in &g.nodes {
            if let Some(c) = evidence_of(n) {
                assert!(crit.iter().any(|x| x == c), "{}: {} produces unknown {c}", t.id, n.node_id);
            }
        }
        // The catalog knows every criterion and the contract.
        for c in &crit {
            assert!(catalog::criterion(c).is_some(), "catalog lacks criterion {c}");
        }
        let cc = catalog::completion_contract(&t.completion_policy_id()).unwrap();
        assert_eq!(cc["task_type"], t.id);
        let tmpl = KernelTaskTemplate(t);
        assert_eq!(crate::agency_api::compiler::RunTemplate::completion_policy(&tmpl), t.completion_policy_id());
    }
}

#[test]
fn no_vendor_or_model_names_in_graphs_or_contracts() {
    const WORDS: &[&str] = &["openai", "anthropic", "claude", "codex", "gpt", "gemini", "llama", "mistral",
        "deepseek", "qwen", "sonnet", "opus", "haiku", "kimi", "grok"];
    let mut texts: Vec<&str> = TASK_TYPES.iter().map(|t| t.graph_json).collect();
    texts.push(CONTRACTS);
    texts.extend(EVALS);
    for s in texts {
        let l = s.to_ascii_lowercase();
        for w in WORDS {
            assert!(!l.contains(w), "vendor/model word `{w}` in task-type data");
        }
    }
}

#[test]
fn registry_hooks_expose_new_types_and_keep_bug_fix() {
    let reg = TemplateRegistry::default();
    let bf = reg.get("BUG_FIX").unwrap();
    assert_eq!(bf.completion_policy(), "completion.bug_fix");
    for id in ids() {
        let tm = reg.get(id).unwrap_or_else(|| panic!("{id} not registered"));
        assert_eq!(tm.id(), id);
        assert_eq!(tm.source(), "kernel");
        let t = get(id).unwrap();
        let params = json!({ "writable_resources": sample_resources(t), "task_id": format!("task.{id}.1") });
        let g = tm.instantiate("do the thing", &params).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(g.wih_policy["requires_lease_for_write"], true);
        assert_eq!(g.wih_policy["graph_id"], t.graph_id);
        assert!(g.nodes.iter().all(|n| n["id"].is_string() && n["role"].is_string()));
        if t.write_scheme.is_some() {
            assert!(tm.instantiate("x", &json!({})).is_err(), "{id} must fail closed via the template too");
        }
    }
    assert!(reg.get("NOPE").is_none());
}

#[test]
fn generative_nodes_route_to_the_pool_and_deterministic_ones_stay_s0() {
    for t in TASK_TYPES {
        let script: Script = serde_json::from_value(json!({
            "wait": g_waits(t).into_iter().map(|w| (w, "open")).collect::<std::collections::HashMap<_, _>>()
        })).unwrap();
        let out = run_scripted(t.id, "goal", &json!({ "writable_resources": sample_resources(t) }), &script);
        assert_eq!(out.status, Status::Completed, "{}: {}", t.id, out.reason);
        for (id, plan) in &out.plans {
            let node = graph(t).unwrap().node(id).cloned().unwrap();
            if node.capability_request.is_none() {
                assert_eq!(plan.cognitive_role, Role::S0, "{} {id}", t.id);
            }
        }
        assert!(out.plans.iter().any(|(_, p)| p.cognitive_role == Role::S2), "{}: some step must be generative", t.id);
    }
}

fn g_waits(t: &TaskType) -> Vec<String> {
    graph(t).unwrap().nodes.iter().filter(|n| n.node_kind == "WAIT").map(|n| n.node_id.clone()).collect()
}

/// The eval sets: every case of every task type, run with the scripted executor.
#[test]
fn eval_sets_pass_with_the_scripted_executor() {
    let mut covered = Vec::new();
    for raw in EVALS {
        let set: Value = serde_json::from_str(raw).unwrap();
        let tt = set["task_type"].as_str().unwrap();
        covered.push(tt.to_string());
        let cases = set["cases"].as_array().unwrap();
        assert!(cases.len() >= 4, "{tt}: eval set too small");
        let mut statuses = std::collections::HashSet::new();
        for c in cases {
            let id = c["id"].as_str().unwrap();
            let script: Script = serde_json::from_value(c["script"].clone()).unwrap();
            let params = c.get("params").cloned().unwrap_or_else(|| json!({}));
            let out = run_scripted(tt, c["goal"].as_str().unwrap(), &params, &script);
            let exp = &c["expect"];
            let tag = format!("{tt}/{id}: {} ({})", out.status.as_str(), out.reason);
            assert_eq!(out.status.as_str(), exp["status"].as_str().unwrap(), "{tag}");
            statuses.insert(out.status.as_str());
            if let Some(s) = exp["stopped_at"].as_str() {
                assert_eq!(out.stopped_at.as_deref(), Some(s), "{tag}");
            }
            for v in exp["visited"].as_array().into_iter().flatten() {
                assert!(out.visited_ids().contains(&v.as_str().unwrap()), "{tag}: expected visit {v}");
            }
            for v in exp["not_visited"].as_array().into_iter().flatten() {
                assert!(!out.visited_ids().contains(&v.as_str().unwrap()), "{tag}: must not visit {v}");
            }
            if let Some(m) = exp["missing"].as_array() {
                let m: Vec<&str> = m.iter().filter_map(Value::as_str).collect();
                assert_eq!(out.missing, m, "{tag}");
            }
            if out.status == Status::Completed {
                // Verifier-owned completion: every contract criterion is backed by a receipt ref.
                let p = completion_policy(get(tt).unwrap()).unwrap();
                assert!(allternit_commrails::judge::completion::missing_evidence(&p, &out.evidence).is_empty(), "{tag}");
                assert!(out.visited.iter().all(|(_, ok)| *ok) || out.visited.iter().any(|(n, _)| n.ends_with("_deep")), "{tag}");
            }
        }
        assert!(statuses.contains("completed"), "{tt}: needs a passing case");
        assert!(statuses.len() >= 2, "{tt}: needs a non-passing case");
    }
    let mut want: Vec<String> = ids().into_iter().map(String::from).collect();
    want.sort();
    covered.sort();
    assert_eq!(covered, want, "one eval set per task type");
}
