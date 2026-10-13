//! C6: candidate graph edits. Deterministic mutation operators (valid by
//! construction, still validated), RFC 6902 patches (the LLM proposer's
//! output), structural guardrails every candidate must keep, and a readable
//! node/edge diff for the human approving it.
//!
//! Every function here works on the unbound ComputeGraphIRV1 JSON of a task
//! type (the shipped file or an approved harness version); [`validate`]
//! then runs the candidate through `task_types::bind` (the Factory
//! validator) plus [`guardrails`].

use crate::agency_api::task_types::{self, s1, TaskType};
use allternit_factory_engine::kernel::graph::ComputeGraph;
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashSet, VecDeque};

pub const GATE_PRIMITIVE: &str = "ver.check_acceptance_evidence";
/// The minimum confidence an S1 node is assumed to use when its spec names none.
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.7;
pub const CONFIDENCE_STEP: f64 = 0.1;
pub const MIN_CONFIDENCE_RANGE: (f64, f64) = (0.5, 0.95);
/// A candidate may add at most this many nodes (MASS-style search bound).
pub const MAX_ADDED_NODES: usize = 4;

pub const OPERATORS: &[&str] = &["s2_to_s1", "min_confidence_up", "min_confidence_down", "add_verifier", "reorder_independent", "remove_redundant"];

/// One candidate edit.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// An operator from [`OPERATORS`], or `llm_patch`.
    pub operator: String,
    pub target: Option<String>,
    pub rationale: String,
    pub graph: Value,
    /// The RFC 6902 patch the candidate came from (LLM candidates).
    pub patch: Option<Value>,
    /// Ranking hint from the evidence (higher first).
    pub score: f64,
}

// ── graph helpers (JSON) ────────────────────────────────────────────────────

fn nodes(g: &Value) -> &[Value] {
    g["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn node<'a>(g: &'a Value, id: &str) -> Option<&'a Value> {
    nodes(g).iter().find(|n| n["node_id"] == id)
}

fn edges(g: &Value) -> &[Value] {
    g["edges"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn ext<'a>(n: &'a Value, k: &str) -> Option<&'a Value> {
    n["extensions"].get(k).filter(|v| !v.is_null())
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
}

fn id_of(n: &Value) -> String {
    n["node_id"].as_str().unwrap_or_default().to_string()
}

fn title(n: &Value) -> String {
    ext(n, "x-title").and_then(Value::as_str).map(String::from).unwrap_or_else(|| id_of(n))
}

fn is_writer(n: &Value) -> bool {
    ext(n, "x-bind_write_set").and_then(Value::as_bool).unwrap_or(false)
        || n["write_set"].as_array().is_some_and(|a| !a.is_empty())
        || n["lock_scope"].as_array().is_some_and(|a| !a.is_empty())
}

fn out_edges<'a>(g: &'a Value, id: &str) -> Vec<&'a Value> {
    edges(g).iter().filter(|e| e["from"] == id).collect()
}

fn in_edges<'a>(g: &'a Value, id: &str) -> Vec<&'a Value> {
    edges(g).iter().filter(|e| e["to"] == id).collect()
}

fn edge(from: &str, to: &str, kind: &str) -> Value {
    json!({ "from": from, "to": to, "edge_kind": kind })
}

fn fresh_id(g: &Value, want: &str) -> String {
    let mut id = want.to_string();
    let mut i = 2;
    while node(g, &id).is_some() {
        id = format!("{want}{i}");
        i += 1;
    }
    id
}

/// The stem a task type's graph versions share (`general.task.v1` -> `general.task`).
pub fn graph_stem(graph_id: &str) -> &str {
    match graph_id.rsplit_once(".v") {
        Some((stem, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => stem,
        _ => graph_id,
    }
}

// ── validation ──────────────────────────────────────────────────────────────

/// A writable resource that satisfies the task type's scheme (validation only).
pub fn sample_writable(t: &TaskType) -> Vec<String> {
    match t.write_scheme {
        None => vec![],
        Some("") => vec!["x:harness-check".into()],
        Some(s) => vec![format!("{s}:harness-check")],
    }
}

/// The candidate parses as a ComputeGraphIRV1, binds and validates for the
/// task type (`task_types::bind`: the Factory validator), and keeps every
/// structural guardrail of `base`.
pub fn validate(t: &TaskType, base: &Value, cand: &Value) -> Result<(), String> {
    let g = ComputeGraph::from_json(&cand.to_string()).map_err(|e| format!("not a ComputeGraphIRV1: {e}"))?;
    task_types::bind(g, t.id, t.write_scheme, "task.harness.check", &sample_writable(t)).map_err(|e| e.to_string())?;
    guardrails(t, base, cand)
}

fn ancestors(g: &Value, id: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut q = VecDeque::from([id.to_string()]);
    while let Some(cur) = q.pop_front() {
        for e in in_edges(g, &cur) {
            let f = e["from"].as_str().unwrap_or_default().to_string();
            if seen.insert(f.clone()) {
                q.push_back(f);
            }
        }
    }
    seen
}

/// Can `target` be reached from an entry node without passing `avoid`?
fn reachable_avoiding(g: &Value, target: &str, avoid: &str) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut q: VecDeque<String> = strs(&g["entry_nodes"]).into_iter().filter(|e| e != avoid).collect();
    while let Some(cur) = q.pop_front() {
        if cur == target {
            return true;
        }
        if !seen.insert(cur.clone()) {
            continue;
        }
        for e in out_edges(g, &cur) {
            let to = e["to"].as_str().unwrap_or_default();
            if to != avoid && !seen.contains(to) {
                q.push_back(to.to_string());
            }
        }
    }
    false
}

/// Structural guardrails (MASS-style: search only inside what is safe):
/// the completion gate, completion nodes, POLICY and WAIT nodes stay exactly
/// as they were; every contract criterion keeps a non-generative evidence
/// producer; no node gains write authority; every writer is still reachable
/// only through the POLICY nodes that guarded it; at most [`MAX_ADDED_NODES`] new nodes.
pub fn guardrails(t: &TaskType, base: &Value, cand: &Value) -> Result<(), String> {
    for b in nodes(base) {
        let id = id_of(b);
        let keep = b["primitive_id"] == GATE_PRIMITIVE || matches!(b["node_kind"].as_str(), Some("POLICY" | "WAIT"))
            || strs(&base["completion_nodes"]).contains(&id);
        if keep && node(cand, &id) != Some(b) {
            return Err(format!("guardrail: node {id} ({}) must stay unchanged", b["node_kind"].as_str().unwrap_or_default()));
        }
    }
    if cand["completion_nodes"] != base["completion_nodes"] {
        return Err("guardrail: completion_nodes must stay unchanged".into());
    }
    for b in nodes(base).iter().filter(|b| b["primitive_id"] == GATE_PRIMITIVE) {
        let id = id_of(b);
        let mut be: Vec<String> = out_edges(base, &id).iter().map(|e| e.to_string()).collect();
        let mut ce: Vec<String> = out_edges(cand, &id).iter().map(|e| e.to_string()).collect();
        be.sort();
        ce.sort();
        if be != ce {
            return Err(format!("guardrail: the completion gate {id}'s edges must stay unchanged"));
        }
    }
    let criteria: Vec<String> = task_types::catalog_contract(&t.completion_policy_id())
        .map(|c| c["require"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    for c in &criteria {
        let ok = nodes(cand).iter().any(|n| ext(n, "x-evidence").and_then(Value::as_str) == Some(c)
            && matches!(n["node_kind"].as_str(), Some("VERIFY" | "POLICY" | "WAIT" | "CONTROL")));
        if !ok {
            return Err(format!("guardrail: criterion {c} has no VERIFY/POLICY/WAIT/CONTROL evidence producer"));
        }
    }
    for n in nodes(cand).iter().filter(|n| is_writer(n)) {
        let id = id_of(n);
        let b = node(base, &id).filter(|b| is_writer(b) && b["primitive_id"] == n["primitive_id"]
            && ext(b, "x-bind_write_set") == ext(n, "x-bind_write_set") && b["write_set"] == n["write_set"]);
        if b.is_none() {
            return Err(format!("guardrail: node {id} may not gain or change write authority"));
        }
        let guards: Vec<String> = ancestors(base, &id).into_iter()
            .filter(|a| node(base, a).is_some_and(|x| x["node_kind"] == "POLICY")).collect();
        // The guard must dominate the writer: no path from an entry node reaches it around the guard.
        if let Some(p) = guards.iter().find(|p| reachable_avoiding(cand, &id, p)) {
            return Err(format!("guardrail: writer {id} must still run after policy node {p}"));
        }
    }
    if nodes(cand).len() > nodes(base).len() + MAX_ADDED_NODES {
        return Err(format!("guardrail: a candidate adds at most {MAX_ADDED_NODES} nodes"));
    }
    Ok(())
}

// ── deterministic mutation operators ────────────────────────────────────────

fn set_nodes(g: &mut Value, f: impl FnOnce(&mut Vec<Value>)) {
    if let Some(a) = g["nodes"].as_array_mut() {
        f(a)
    }
}

fn set_edges(g: &mut Value, f: impl FnOnce(&mut Vec<Value>)) {
    if let Some(a) = g["edges"].as_array_mut() {
        f(a)
    }
}

fn node_mut<'a>(g: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    g["nodes"].as_array_mut()?.iter_mut().find(|n| n["node_id"] == id)
}

/// Is `id` the escalation target (`x-twin_of` or ESCALATE `target`) of another node?
fn is_twin(g: &Value, id: &str) -> bool {
    node(g, id).and_then(|n| ext(n, "x-twin_of")).is_some()
        || nodes(g).iter().any(|n| n["on_failure"]["target"] == id && n["on_failure"]["strategy"] == "ESCALATE")
}

/// S2 VERIFY node -> S1 pass/fail decision with an S2 twin on its ESCALATE
/// edge (abstentions and low confidence still reach the S2 path).
pub fn s2_to_s1(g: &Value) -> Vec<Candidate> {
    let mut out = vec![];
    for n in nodes(g) {
        let id = id_of(n);
        if n["cognitive_role"] != "S2" || n["node_kind"] != "VERIFY" || ext(n, s1::SPEC_KEY).is_some() || is_twin(g, &id) {
            continue;
        }
        let mut c = g.clone();
        let twin_id = fresh_id(g, &format!("{id}_s2"));
        let mut twin = n.clone();
        twin["node_id"] = json!(twin_id);
        twin["extensions"]["x-twin_of"] = json!(id);
        twin["extensions"]["x-title"] = json!(format!("{} (escalated)", title(n)));
        let question = format!("Does the result pass: {}?", title(n).to_lowercase());
        let s1n = node_mut(&mut c, &id).expect("node exists");
        s1n["cognitive_role"] = json!("S1");
        s1n["on_failure"] = json!({ "strategy": "ESCALATE", "target": twin_id });
        s1n["extensions"][s1::SPEC_KEY] = json!({ "kind": "verify", "question": question,
            "options": [{ "id": "pass", "text": "It passes." }, { "id": "fail", "text": "It does not pass." }], "pass": ["pass"] });
        let outs: Vec<Value> = out_edges(g, &id).into_iter().cloned().collect();
        set_nodes(&mut c, |a| a.push(twin));
        set_edges(&mut c, |a| {
            a.retain(|e| !(e["from"] == id.as_str() && e["edge_kind"] == "FAILURE"));
            a.push(edge(&id, &twin_id, "FAILURE"));
            for e in &outs {
                let mut e2 = e.clone();
                e2["from"] = json!(twin_id);
                a.push(e2);
            }
        });
        out.push(Candidate { operator: "s2_to_s1".into(), target: Some(id.clone()), patch: None, score: 3.0,
            rationale: format!("{id} is an S2 (model) verification with a pass/fail answer: decide it as S1 and escalate to an S2 twin ({twin_id}) only on abstention or low confidence"),
            graph: c });
    }
    out
}

/// Raise (`up`) or lower an S1 node's `min_confidence` by [`CONFIDENCE_STEP`].
pub fn min_confidence(g: &Value, up: bool) -> Vec<Candidate> {
    let mut out = vec![];
    for n in nodes(g) {
        let Some(spec) = ext(n, s1::SPEC_KEY) else { continue };
        if n["cognitive_role"] != "S1" {
            continue;
        }
        let id = id_of(n);
        let cur = spec["min_confidence"].as_f64().unwrap_or(DEFAULT_MIN_CONFIDENCE);
        let next = ((cur + if up { CONFIDENCE_STEP } else { -CONFIDENCE_STEP }) * 100.0).round() / 100.0;
        let next = next.clamp(MIN_CONFIDENCE_RANGE.0, MIN_CONFIDENCE_RANGE.1);
        if (next - cur).abs() < 1e-9 {
            continue;
        }
        let mut c = g.clone();
        node_mut(&mut c, &id).expect("node exists")["extensions"][s1::SPEC_KEY]["min_confidence"] = json!(next);
        out.push(Candidate {
            operator: if up { "min_confidence_up" } else { "min_confidence_down" }.into(), target: Some(id.clone()), patch: None, score: 0.5,
            rationale: if up { format!("{id}: escalate more S1 answers to the S2 twin (min_confidence {cur} -> {next})") }
                       else { format!("{id}: keep more S1 answers at S1 instead of escalating (min_confidence {cur} -> {next})") },
            graph: c,
        });
    }
    out
}

/// Insert a deterministic S0 existence check after a generative node whose
/// NORMAL successor is not a verifier.
pub fn add_verifier(g: &Value) -> Vec<Candidate> {
    let mut out = vec![];
    for n in nodes(g) {
        let id = id_of(n);
        if !matches!(n["cognitive_role"].as_str(), Some("S2")) || n["node_kind"] != "COMPUTE" || is_twin(g, &id) {
            continue;
        }
        let Some(produced) = strs(&n["outputs"]).into_iter().find(|o| o.starts_with("candidate:")) else { continue };
        let normal: Vec<&Value> = out_edges(g, &id).into_iter().filter(|e| e["edge_kind"] == "NORMAL").collect();
        let [e] = normal.as_slice() else { continue };
        let succ = e["to"].as_str().unwrap_or_default().to_string();
        if node(g, &succ).is_none_or(|s| s["node_kind"] == "VERIFY") {
            continue;
        }
        let check = fresh_id(g, &format!("{id}_check"));
        // The node and its escalation twin (same output) both go through the check.
        let mut from: BTreeSet<String> = BTreeSet::from([id.clone()]);
        if let Some(t) = n["on_failure"]["target"].as_str().filter(|_| n["on_failure"]["strategy"] == "ESCALATE") {
            if node(g, t).is_some_and(|x| strs(&x["outputs"]).contains(&produced)) {
                from.insert(t.to_string());
            }
        }
        let mut c = g.clone();
        set_nodes(&mut c, |a| a.push(json!({
            "node_id": check, "primitive_id": "ver.check_artifact_existence", "node_kind": "VERIFY", "cognitive_role": "S0",
            "inputs": [produced], "outputs": [format!("receipt:{check}")], "read_set": n["read_set"].clone(),
            "write_set": [], "lock_scope": [], "evidence_required": [], "on_failure": { "strategy": "FAIL" },
            "extensions": { "x-title": format!("Check: {}", title(n)) },
        })));
        set_edges(&mut c, |a| {
            for e in a.iter_mut() {
                if e["to"] == succ.as_str() && e["edge_kind"] == "NORMAL" && from.contains(e["from"].as_str().unwrap_or_default()) {
                    e["to"] = json!(check);
                }
            }
            a.push(edge(&check, &succ, "NORMAL"));
        });
        out.push(Candidate { operator: "add_verifier".into(), target: Some(id.clone()), patch: None, score: 1.0,
            rationale: format!("add a deterministic check ({check}) that {id}'s {produced} exists before {succ} uses it"), graph: c });
    }
    out
}

/// A node is a plain S0 step (no evidence, no writes, not the gate).
fn plain_s0(n: &Value) -> bool {
    n["cognitive_role"] == "S0" && n["node_kind"] == "COMPUTE" && ext(n, "x-evidence").is_none() && !is_writer(n)
        && n["primitive_id"] != GATE_PRIMITIVE && n["wait_gate"].is_null()
}

/// Swap two adjacent independent S0 steps (A -> B, B reads nothing A
/// produces and vice versa, no other edges touch the pair).
pub fn reorder_independent(g: &Value) -> Vec<Candidate> {
    let mut out = vec![];
    let completion = strs(&g["completion_nodes"]);
    for a in nodes(g).iter().filter(|n| plain_s0(n)) {
        let aid = id_of(a);
        let aout = out_edges(g, &aid);
        let [ab] = aout.as_slice() else { continue };
        if ab["edge_kind"] != "NORMAL" {
            continue;
        }
        let bid = ab["to"].as_str().unwrap_or_default().to_string();
        let Some(b) = node(g, &bid).filter(|b| plain_s0(b)) else { continue };
        let bout = out_edges(g, &bid);
        let [bn] = bout.as_slice() else { continue };
        if bn["edge_kind"] != "NORMAL" || in_edges(g, &bid).len() != 1 || completion.contains(&aid) || completion.contains(&bid) {
            continue;
        }
        let (ai, ao, bi, bo) = (strs(&a["inputs"]), strs(&a["outputs"]), strs(&b["inputs"]), strs(&b["outputs"]));
        if bi.iter().any(|x| ao.contains(x)) || ai.iter().any(|x| bo.contains(x)) {
            continue;
        }
        let next = bn["to"].as_str().unwrap_or_default().to_string();
        let mut c = g.clone();
        set_edges(&mut c, |es| {
            for e in es.iter_mut() {
                let (f, t) = (e["from"].as_str().unwrap_or_default().to_string(), e["to"].as_str().unwrap_or_default().to_string());
                if f == aid && t == bid {
                    *e = edge(&bid, &aid, "NORMAL");
                } else if f == bid && t == next {
                    *e = edge(&aid, &next, "NORMAL");
                } else if t == aid {
                    e["to"] = json!(bid);
                }
            }
        });
        if let Some(en) = c["entry_nodes"].as_array_mut() {
            for x in en.iter_mut().filter(|x| **x == aid.as_str()) {
                *x = json!(bid);
            }
        }
        out.push(Candidate { operator: "reorder_independent".into(), target: Some(format!("{aid},{bid}")), patch: None, score: 0.2,
            rationale: format!("{aid} and {bid} are independent S0 steps: run {bid} first"), graph: c });
    }
    out
}

/// Remove an S0 step whose outputs no node reads (one way in, one way out).
pub fn remove_redundant(g: &Value) -> Vec<Candidate> {
    let mut out = vec![];
    let (entry, completion) = (strs(&g["entry_nodes"]), strs(&g["completion_nodes"]));
    for n in nodes(g).iter().filter(|n| plain_s0(n)) {
        let id = id_of(n);
        if entry.contains(&id) || completion.contains(&id) {
            continue;
        }
        let (ins, outs) = (in_edges(g, &id), out_edges(g, &id));
        let ([i], [o]) = (ins.as_slice(), outs.as_slice()) else { continue };
        if i["edge_kind"] != "NORMAL" || o["edge_kind"] != "NORMAL" {
            continue;
        }
        let produced = strs(&n["outputs"]);
        if nodes(g).iter().any(|m| m["node_id"] != id.as_str() && strs(&m["inputs"]).iter().any(|x| produced.contains(x))) {
            continue;
        }
        let (p, s) = (i["from"].as_str().unwrap_or_default().to_string(), o["to"].as_str().unwrap_or_default().to_string());
        let mut c = g.clone();
        set_nodes(&mut c, |a| a.retain(|m| m["node_id"] != id.as_str()));
        set_edges(&mut c, |a| {
            a.retain(|e| e["from"] != id.as_str() && e["to"] != id.as_str());
            if !a.iter().any(|e| e["from"] == p.as_str() && e["to"] == s.as_str()) {
                a.push(edge(&p, &s, "NORMAL"));
            }
        });
        out.push(Candidate { operator: "remove_redundant".into(), target: Some(id.clone()), patch: None, score: 1.0,
            rationale: format!("no node reads {id}'s outputs ({}): drop the step", produced.join(", ")), graph: c });
    }
    out
}

/// Every operator in `ops` (all of [`OPERATORS`] when empty) over `g`.
pub fn mutations(g: &Value, ops: &[String]) -> Vec<Candidate> {
    let on = |o: &str| ops.is_empty() || ops.iter().any(|x| x == o);
    let mut out = vec![];
    if on("s2_to_s1") { out.extend(s2_to_s1(g)); }
    if on("min_confidence_up") { out.extend(min_confidence(g, true)); }
    if on("min_confidence_down") { out.extend(min_confidence(g, false)); }
    if on("add_verifier") { out.extend(add_verifier(g)); }
    if on("reorder_independent") { out.extend(reorder_independent(g)); }
    if on("remove_redundant") { out.extend(remove_redundant(g)); }
    out
}

// ── RFC 6902 ────────────────────────────────────────────────────────────────

fn unescape(tok: &str) -> String {
    tok.replace("~1", "/").replace("~0", "~")
}

fn split_ptr(path: &str) -> Result<Vec<String>, String> {
    if path.is_empty() {
        return Ok(vec![]);
    }
    let rest = path.strip_prefix('/').ok_or_else(|| format!("bad JSON pointer `{path}`"))?;
    Ok(rest.split('/').map(unescape).collect())
}

fn parent_mut<'a>(doc: &'a mut Value, toks: &[String]) -> Result<(&'a mut Value, String), String> {
    let (last, head) = toks.split_last().ok_or("the root cannot be a target")?;
    let mut cur = doc;
    for t in head {
        cur = match cur {
            Value::Object(m) => m.get_mut(t).ok_or_else(|| format!("no member `{t}`"))?,
            Value::Array(a) => a.get_mut(t.parse::<usize>().map_err(|_| format!("bad index `{t}`"))?).ok_or_else(|| format!("index {t} out of range"))?,
            _ => return Err(format!("cannot index into a scalar at `{t}`")),
        };
    }
    Ok((cur, last.clone()))
}

fn get_ptr(doc: &Value, path: &str) -> Result<Value, String> {
    doc.pointer(path).cloned().ok_or_else(|| format!("no value at `{path}`"))
}

fn remove_at(doc: &mut Value, path: &str) -> Result<Value, String> {
    let toks = split_ptr(path)?;
    let (p, last) = parent_mut(doc, &toks)?;
    match p {
        Value::Object(m) => m.remove(&last).ok_or_else(|| format!("no member `{last}`")),
        Value::Array(a) => {
            let i = last.parse::<usize>().map_err(|_| format!("bad index `{last}`"))?;
            (i < a.len()).then(|| a.remove(i)).ok_or_else(|| format!("index {i} out of range"))
        }
        _ => Err("cannot remove from a scalar".into()),
    }
}

fn add_at(doc: &mut Value, path: &str, v: Value, replace: bool) -> Result<(), String> {
    let toks = split_ptr(path)?;
    if toks.is_empty() {
        *doc = v;
        return Ok(());
    }
    let (p, last) = parent_mut(doc, &toks)?;
    match p {
        Value::Object(m) => {
            if replace && !m.contains_key(&last) {
                return Err(format!("no member `{last}` to replace"));
            }
            m.insert(last, v);
            Ok(())
        }
        Value::Array(a) => {
            if last == "-" && !replace {
                a.push(v);
                return Ok(());
            }
            let i = last.parse::<usize>().map_err(|_| format!("bad index `{last}`"))?;
            if replace {
                *a.get_mut(i).ok_or_else(|| format!("index {i} out of range"))? = v;
            } else if i <= a.len() {
                a.insert(i, v);
            } else {
                return Err(format!("index {i} out of range"));
            }
            Ok(())
        }
        _ => Err("cannot add into a scalar".into()),
    }
}

pub const MAX_PATCH_OPS: usize = 32;

/// Apply an RFC 6902 JSON Patch (add, remove, replace, move, copy, test).
/// All or nothing: any failing op rejects the patch.
pub fn apply_patch(doc: &Value, patch: &Value) -> Result<Value, String> {
    let ops = patch.as_array().ok_or("a patch is an array of operations")?;
    if ops.is_empty() || ops.len() > MAX_PATCH_OPS {
        return Err(format!("a patch has 1-{MAX_PATCH_OPS} operations"));
    }
    let mut d = doc.clone();
    for (i, op) in ops.iter().enumerate() {
        let path = op["path"].as_str().ok_or_else(|| format!("op {i}: path is required"))?;
        let r = match op["op"].as_str() {
            Some("add") => op.get("value").cloned().ok_or("value is required".to_string()).and_then(|v| add_at(&mut d, path, v, false)),
            Some("replace") => op.get("value").cloned().ok_or("value is required".to_string()).and_then(|v| add_at(&mut d, path, v, true)),
            Some("remove") => remove_at(&mut d, path).map(|_| ()),
            Some("move") => op["from"].as_str().ok_or("from is required".to_string())
                .and_then(|f| remove_at(&mut d, f)).and_then(|v| add_at(&mut d, path, v, false)),
            Some("copy") => op["from"].as_str().ok_or("from is required".to_string())
                .and_then(|f| get_ptr(&d, f)).and_then(|v| add_at(&mut d, path, v, false)),
            Some("test") => get_ptr(&d, path).and_then(|v| if Some(&v) == op.get("value") { Ok(()) } else { Err(format!("test failed at `{path}`")) }),
            other => Err(format!("unsupported op {other:?}")),
        };
        r.map_err(|e| format!("op {i}: {e}"))?;
    }
    Ok(d)
}

// ── readable diff ───────────────────────────────────────────────────────────

fn field_changes(a: &Value, b: &Value, prefix: &str, out: &mut Vec<Value>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let (va, vb) = (x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null));
                if va != vb {
                    field_changes(va, vb, &format!("{prefix}{}{k}", if prefix.is_empty() { "" } else { "." }), out);
                }
            }
        }
        _ => out.push(json!({ "field": prefix, "before": a, "after": b })),
    }
}

/// Nodes added / removed / changed (field by field) and edges added /
/// removed, plus one line per change for the approval screen.
pub fn diff(base: &Value, cand: &Value) -> Value {
    let ids = |g: &Value| -> BTreeSet<String> { nodes(g).iter().map(id_of).collect() };
    let (bi, ci) = (ids(base), ids(cand));
    let mut lines = vec![];
    let added: Vec<Value> = ci.difference(&bi).filter_map(|id| node(cand, id).cloned()).collect();
    for n in &added {
        lines.push(format!("+ node {} ({} {} {})", id_of(n), n["node_kind"].as_str().unwrap_or_default(), n["cognitive_role"].as_str().unwrap_or_default(), n["primitive_id"].as_str().unwrap_or_default()));
    }
    let removed: Vec<String> = bi.difference(&ci).cloned().collect();
    for id in &removed {
        lines.push(format!("- node {id}"));
    }
    let mut changed = Map::new();
    for id in bi.intersection(&ci) {
        let (a, b) = (node(base, id).unwrap(), node(cand, id).unwrap());
        if a != b {
            let mut f = vec![];
            field_changes(a, b, "", &mut f);
            for c in &f {
                lines.push(format!("~ node {id}: {} {} -> {}", c["field"].as_str().unwrap_or_default(), c["before"], c["after"]));
            }
            changed.insert(id.clone(), json!(f));
        }
    }
    let ekey = |e: &Value| format!("{} -> {} ({})", e["from"].as_str().unwrap_or_default(), e["to"].as_str().unwrap_or_default(), e["edge_kind"].as_str().unwrap_or_default());
    let (be, ce): (BTreeSet<String>, BTreeSet<String>) = (edges(base).iter().map(ekey).collect(), edges(cand).iter().map(ekey).collect());
    let eadd: Vec<&String> = ce.difference(&be).collect();
    let erem: Vec<&String> = be.difference(&ce).collect();
    lines.extend(eadd.iter().map(|e| format!("+ edge {e}")));
    lines.extend(erem.iter().map(|e| format!("- edge {e}")));
    if base["entry_nodes"] != cand["entry_nodes"] {
        lines.push(format!("~ entry_nodes {} -> {}", base["entry_nodes"], cand["entry_nodes"]));
    }
    json!({ "summary": lines, "nodes_added": added, "nodes_removed": removed, "nodes_changed": changed,
            "edges_added": eadd, "edges_removed": erem })
}
