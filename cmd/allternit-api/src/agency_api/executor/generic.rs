//! WP-X2: the generic driver for the X1 task types (everything but BUG_FIX).
//!
//! Walks a task type's graph node by node, by `node_kind` / role:
//! * COMPUTE S0: deterministic work here (parse, context, receipts);
//! * COMPUTE with a write set: the effect, only after a POLICY node authorized
//!   it, through P1's fenced, idempotent, capped effect path (`Exec::effect`);
//! * COMPUTE S2/S3 and S2 VERIFY: cognition through gizzi-code (or the
//!   scripted executor), journaled so a re-drive replays it; a failure follows
//!   the node's ESCALATE edge to its S3 twin (tier escalation);
//! * VERIFY S0 / POLICY / CONTROL: deterministic, receipt-backed;
//! * WAIT: parks the run on an attention request; the requester's approval or
//!   rejection (or an explicit trigger) re-queues it and the gate opens or closes;
//! * the `gate` node checks the task type's completion contract against the
//!   receipt-backed evidence (verifier-owned completion, N21-style).
//!
//! Effects need a surface adapter. This server has one for `fs:` (the document
//! is written in the run's disposable directory and published as an
//! artifact), `thread:` and `template:` (WP-C3a, `agency_api/effects/`),
//! `computer:` and `campaign:` (WP-C3b, `agency_api/effects/`);
//! other schemes fail closed outside the scripted executor.

use super::*;
use crate::agency_api::task_types::{self, evidence_of, TaskType};
use std::collections::HashSet;

pub const TASK_WAIT_REASON: &str = "task_wait";
const GATE_PRIMITIVE: &str = "ver.check_acceptance_evidence";
const MAX_STEPS: usize = 64;
const SYSTEM: &str = "You are one step of a verified agent run. Answer with the requested content only.";

/// Dev/conformance script (scripted executor only), read from
/// `<runs dir>/<run_id>/scripted.json`: nodes that fail, criteria whose
/// verifier withholds evidence.
#[derive(Default, serde::Deserialize)]
struct Script {
    #[serde(default)]
    fail: HashSet<String>,
    #[serde(default)]
    withhold: HashSet<String>,
    /// WP-C3b: the scripted text a node produces (default: a placeholder).
    #[serde(default)]
    outputs: HashMap<String, String>,
    /// WP-C3b: effects go through the real connectors (default: recorded refs).
    #[serde(default)]
    live: bool,
}

fn next(g: &ComputeGraph, from: &str, kind: &str) -> Option<String> {
    g.edges.iter().find(|e| e.from == from && e.kind() == kind).map(|e| e.to.clone())
}

impl Exec<'_> {
    fn verify_receipt(&self, node: &str, verifier: &str, pass: bool, extra: Value) -> Result<String> {
        let cs = self.st.rails.receipts.chain_store()?;
        let mut body = json!({ "envelope": { "schema_id": VERIFY_RECEIPT, "schema_version": "1.0.0", "run_id": self.run_id, "node_id": node },
            "type": "verification", "verifier": verifier, "result": if pass { "PASS" } else { "FAIL" }, "fence": FENCE });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) { b.extend(e.clone()); }
        let r = cs.append(body)?;
        let id = r["chain"]["receipt_id"].as_str().unwrap_or_default().to_string();
        self.emit("receipt.appended", json!({ "receipt_id": id, "receipt_type": "verification", "step": node }))?;
        Ok(id)
    }

    /// Cognition for a generative node, journaled (replay = no model call).
    fn generate(&mut self, plan: &ExecutionPlan, node: &str, prompt: &str, script: &Script) -> Step<String> {
        let jkey = format!("{}:{node}:model.generate:1", self.run_id);
        if let Some(v) = super::super::safety::journaled_value(&self.st.db, &jkey)? {
            return Ok(v.as_str().unwrap_or_default().to_string());
        }
        let t0 = Instant::now();
        let (text, usage) = if scripted() {
            if script.fail.contains(node) {
                return Err(StepErr::Fail(anyhow!("scripted: {node} produced no usable output")));
            }
            (script.outputs.get(node).cloned().unwrap_or_else(|| format!("scripted output of {node}")), crate::gizzi_completion::Usage::default())
        } else {
            let _ledger = crate::usage_ledger::enter(crate::usage_ledger::LedgerCtx::surface("agency")
                .run(&self.run_id, Some(node)).tier(&format!("{:?}", plan.cognitive_role)).tenant(Some(&self.org), None));
            let model = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == plan.backend_id))
                .and_then(|e| e.extensions.as_ref()?.get("x-model_ref")?.as_str()?.split_once('/'))
                .map(|(p, m)| (p.to_string(), m.to_string()));
            match self.h.block_on(crate::gizzi_completion::complete_ephemeral_usage(prompt, Some(SYSTEM), model.as_ref())) {
                Ok((t, u)) if !t.trim().is_empty() => (t, u),
                Ok(_) => return Err(StepErr::Fail(anyhow!("{node}: empty model output"))),
                Err(e) => {
                    super::super::safety::cool_down(&plan.backend_id);
                    return Err(StepErr::Fail(anyhow!("{node}: model error: {e}")));
                }
            }
        };
        self.note_split(usage.tokens_in, usage.tokens_out);
        let est = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == plan.backend_id)).map(|e| e.cost).unwrap_or(0.0);
        self.charge_tokens(t0.elapsed().as_secs_f64(), if usage.cost_usd > 0.0 { usage.cost_usd } else { est }, 1, usage.tokens)?;
        if !super::super::safety::journal_value(&self.st.db, &jkey, &self.run_id, node, "model.generate", self.epoch, &json!(text))? {
            return Err(StepErr::Stop);
        }
        Ok(text)
    }

    /// The WAIT gate's state from the run's attention items: Some(true) open,
    /// Some(false) closed, None = nobody answered yet (park).
    fn wait_state(&self, node: &str) -> Result<Option<bool>> {
        let rec = self.h.block_on(self.s.load_run(&self.run_id))?.ok_or_else(|| anyhow!("run vanished"))?;
        Ok(rec.attention.iter().rev().find(|a| a["reason"] == TASK_WAIT_REASON && a["node_id"] == node && a["status"] == "resolved")
            .map(|a| a["resolution"]["outcome"] == "approved"))
    }

    /// One effect node: the declared write set, through the fenced effect path.
    fn apply_effect(&mut self, node: &str, primitive: &str, write_set: &[String], content: &str, script: &Script) -> Step<String> {
        let fail = scripted() && script.fail.contains(node);
        let sc = scripted() && !script.live;
        // ── WP-C3b: computer:/campaign: connectors (approval first, then fenced dispatch) ──
        let c3b = if !sc && write_set.iter().any(|r| r.starts_with("computer:") || r.starts_with("campaign:")) {
            Some(self.c3b_ctx(node, write_set, content)?)
        } else { None };
        // ── end WP-C3b ──
        let digest = allternit_commrails::receipts::jcs::sha256_tagged(content.as_bytes());
        let tool = if primitive == "mut.create_file" { "tool.write_file" } else { "tool.execute" };
        let (ws, body) = (write_set.to_vec(), content.to_string());
        // ── WP-C3a: connector context (the run's owner identity, no new auth) ──
        let (st, h, org, run_id, nd) = (self.st, self.h, self.org.clone(), self.run_id.clone(), node.to_string());
        let owner = self.h.block_on(self.s.load_run(&self.run_id))?.map(|r| r.owner).unwrap_or_default();
        // ── end WP-C3a ──
        // G2: a child template that stops for a person parks this run instead
        // of failing it (the receipt chain flattens the error, so it comes back here).
        let parked: std::cell::RefCell<Option<super::super::effect_template::NeedsPerson>> = Default::default();
        let park_slot = &parked;
        let res = self.effect_with(node, tool, "MUTATE", json!({ "write_set": write_set, "content_digest": digest }), false, move |w| {
            if fail { bail!("scripted: effect failed"); }
            let mut refs = vec![];
            for r in &ws {
                let (scheme, target) = r.split_once(':').ok_or_else(|| anyhow!("bad resource {r}"))?;
                match scheme {
                    "fs" => {
                        let rel = Path::new(target.trim_start_matches('/'));
                        if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) { bail!("fs path escapes the run directory: {target}"); }
                        let p = w.root.join("out").join(rel);
                        std::fs::create_dir_all(p.parent().unwrap_or(&w.root))?;
                        std::fs::write(&p, &body)?;
                        refs.push(format!("fs:{}:{digest}", rel.display()));
                    }
                    // ── WP-C3a connectors: stable key per (run, node, resource) ──
                    "thread" => {
                        let key = format!("{run_id}:{nd}:{r}");
                        refs.push(super::super::effect_thread::post(&st.db, &owner, &run_id, &nd, target, &key, &body)?);
                        // G2: the same reply, live in the thread's open session (deduped by key).
                        // Best effort: the ledger post above is the record.
                        if let Err(e) = h.block_on(super::super::effect_thread::deliver_live(&st.db, &owner, target, &run_id, &key, &body)) {
                            tracing::warn!(%run_id, thread_id = %target, error = %e, "thread post is on the ledger but did not reach the live session");
                        }
                    }
                    "template" => match super::super::effect_template::run(h, st, &owner, &org, &run_id, target, &format!("{run_id}:{nd}:{r}"), json!({ "goal": body })) {
                        Ok(r) => refs.push(r),
                        Err(e) => match e.downcast::<super::super::effect_template::NeedsPerson>() {
                            Ok(np) => {
                                // A fixed message on purpose: the journaled failure
                                // must classify `retryable` no matter what text the
                                // child's attention item carries (step labels are
                                // user-controlled and could trip a permanent
                                // keyword in `safety::classify_error`).
                                *park_slot.borrow_mut() = Some(np);
                                bail!("template child stopped for a person: parked on it, re-checks after the answer");
                            }
                            Err(e) => return Err(e),
                        },
                    },
                    // ── end WP-C3a ──
                    // ── WP-C3b ──
                    "computer" | "campaign" if c3b.is_some() => {
                        refs.push(c3b.as_ref().and_then(|c| c.apply(scheme, target, &body)).unwrap_or_else(|| Err(anyhow!("no connector")))?);
                    }
                    // ── end WP-C3b ──
                    _ if sc => refs.push(format!("{scheme}:{target}:{digest}")),
                    _ => bail!("no effect adapter for `{scheme}:` on this server (fail closed)"),
                }
            }
            Ok(format!("effect:{}", refs.join(",")))
        });
        match parked.into_inner() {
            Some(np) if res.is_err() => Err(self.park_on_child(node, np)),
            _ => res,
        }
    }

    /// G2: park this run on a child template's open attention item. One
    /// parent item per child item (a re-drive that finds the child still
    /// waiting does not open a second one). The effect was recorded as a
    /// retryable failure, so the drive after the answer re-checks the child.
    fn park_on_child(&self, node: &str, np: super::super::effect_template::NeedsPerson) -> StepErr {
        use super::super::effect_template::CHILD_ATTENTION_REASON;
        let child_att = np.attention["id"].as_str().unwrap_or_default().to_string();
        let r = self.h.block_on(async {
            let rec = self.s.load_run(&self.run_id).await?.ok_or_else(|| anyhow!("run vanished"))?;
            if rec.attention.iter().any(|a| a["status"] == "open" && a["child_attention_id"] == child_att.as_str()) {
                return anyhow::Ok(());
            }
            let title = np.attention["title"].as_str().unwrap_or("A template step needs you");
            let detail = format!("Step {node} runs a template that stopped for a person:\n{}\nAnswering here answers the template's request; the run then picks up where it left off.",
                np.attention["detail"].as_str().unwrap_or_default());
            self.s.park_attention(&self.run_id, CHILD_ATTENTION_REASON, title, &detail, json!({ "node_id": node, "child_run_id": np.child_run,
                "child_attention_id": child_att, "consequential": np.attention["consequential"].as_bool().unwrap_or(false) })).await?;
            Ok(())
        });
        match r { Ok(()) => StepErr::Stop, Err(e) => StepErr::Fail(e) }
    }

    /// Drive a task type's graph to verifier-owned completion.
    pub(super) fn run_task_type(&mut self, t: &TaskType, goal: &str, write_set: &[String]) -> Step<()> {
        let events = self.h.block_on(self.s.events_of_type(allternit_commrails::judge::events::POLICY_SET))?;
        if effective_policy(&events, &self.dag_id, Some("gate")).close_by != CloseBy::Verifier {
            return Err(StepErr::Fail(anyhow!("completion is not verifier-owned for this run (fail closed)")));
        }
        let policy = task_types::completion_policy(t).ok_or_else(|| anyhow!("{} has no completion contract", t.id))?;
        let script: Script = if scripted() {
            std::fs::read_to_string(self.ws.root.join("scripted.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
        } else { Script::default() };
        let mut evidence: Vec<String> = vec![];
        let mut out: HashMap<String, String> = HashMap::new(); // output name → value
        let mut failed_nodes: Vec<String> = vec![];
        let mut authorized = false;
        let mut doc: Option<(String, String)> = None;
        let mut cur = self.graph.entry_nodes.first().cloned().unwrap_or_default();
        for _ in 0..MAX_STEPS {
            let node = self.graph.node(&cur).cloned().ok_or_else(|| anyhow!("graph has no node {cur}"))?;
            let (ran, plan) = self.route_node(&cur)?;
            let unmet = node.evidence_required.iter().any(|c| !evidence.iter().any(|e| e.starts_with(&format!("{c}:"))));
            let role = node.cognitive_role.clone().unwrap_or_default();
            let input = node.inputs.iter().filter_map(|i| out.get(i)).cloned().collect::<Vec<_>>().join("\n");
            let s_fail = scripted() && script.fail.contains(&cur);
            let mut proof = json!({});
            let ok: bool = if node.primitive_id == GATE_PRIMITIVE && !node.evidence_required.is_empty() {
                let missing = missing_evidence(&policy, &evidence);
                let vid = self.verify_receipt(&cur, &policy.policy_id, missing.is_empty(), json!({ "evidence_refs": evidence, "missing": missing }))?;
                self.emit("verification.completed", json!({ "result": if missing.is_empty() { "pass" } else { "fail" }, "receipt_id": vid, "missing": missing }))?;
                out.insert("receipt:completion_check".into(), vid);
                out.insert("missing".into(), missing.join(", "));
                missing.is_empty()
            } else if node.node_kind == "WAIT" {
                match self.wait_state(&cur)? {
                    Some(open) => open && !unmet,
                    None => {
                        let kind = node.wait_gate.as_ref().and_then(|w| w["kind"].as_str()).unwrap_or_default().to_string();
                        self.h.block_on(self.s.park_attention(&self.run_id, TASK_WAIT_REASON,
                            if kind == "wake" { "Waiting for an explicit trigger" } else { "Waiting for your acceptance" },
                            &format!("Step {cur} ({kind}) needs an answer. Approve to continue, reject to stop."),
                            json!({ "node_id": cur, "wait_kind": kind, "consequential": false })))?;
                        return Err(StepErr::Stop);
                    }
                }
            } else if node.node_kind == "POLICY" {
                // Deterministic: the writers' sets stay inside the declared resources, under the strict fence.
                let declared: HashSet<&String> = write_set.iter().collect();
                let inside = self.graph.nodes.iter().all(|n| n.write_set.iter().all(|w| declared.contains(w)));
                let allow = inside && !write_set.is_empty() && !s_fail && !unmet;
                let cs = self.st.rails.receipts.chain_store()?;
                let r = cs.append(json!({ "envelope": { "schema_id": POLICY_RECEIPT, "schema_version": "1.0.0", "run_id": self.run_id, "node_id": cur },
                    "type": "policy_decision", "decision": if allow { "allow" } else { "deny" }, "write_set": write_set, "fence": FENCE }))?;
                let rid = r["chain"]["receipt_id"].as_str().unwrap_or_default().to_string();
                self.emit("receipt.appended", json!({ "receipt_id": rid, "receipt_type": "policy_decision", "step": cur }))?;
                authorized = allow;
                proof = json!({ "policy_receipt_id": rid });
                allow
            } else if !node.write_set.is_empty() {
                if !authorized || unmet {
                    false // no effect without a policy authorization (N13)
                } else {
                    let content = out.get("candidate:draft").or_else(|| out.get("candidate:response")).or_else(|| out.get("candidate:reply"))
                        .or_else(|| out.get("candidate:action")).or_else(|| out.get("candidate:step")).cloned().unwrap_or_else(|| input.clone());
                    match self.apply_effect(&cur, &node.primitive_id, &node.write_set, &content, &script) {
                        Ok(r) => {
                            if node.primitive_id == "mut.create_file" { doc = Some((write_set[0].clone(), content)); }
                            for o in &node.outputs { out.insert(o.clone(), r.clone()); }
                            proof = json!({ "effect": r });
                            true
                        }
                        Err(StepErr::Stop) => return Err(StepErr::Stop),
                        Err(StepErr::Fail(e)) => { tracing::info!(run_id = %self.run_id, node = %cur, error = %e, "effect failed"); false }
                    }
                }
            } else if role == "S2" || role == "S3" {
                let prompt = if node.node_kind == "VERIFY" {
                    format!("Goal: {goal}\n\nResult:\n{input}\n\nDoes the result meet the goal? First line: PASS or FAIL, then one sentence.")
                } else {
                    let hint = if node.outputs.iter().any(|o| o == "candidate:action") { crate::agency_api::effects::computer::ACTION_HINT }
                        else if node.outputs.iter().any(|o| o == "candidate:step") { crate::agency_api::effects::campaign::STEP_HINT } else { "" }; // WP-C3b
                    format!("Goal: {goal}\n\nContext:\n{input}\n\nProduce: {}{hint}", node.extensions.as_ref().and_then(|e| e.get("x-title")).and_then(Value::as_str).unwrap_or("the step's output"))
                };
                match self.generate(&plan, &cur, &prompt, &script) {
                    Ok(text) => {
                        let pass = node.node_kind != "VERIFY" || scripted() || text.trim_start().to_ascii_uppercase().starts_with("PASS");
                        for o in &node.outputs { out.insert(o.clone(), text.clone()); }
                        pass && !unmet
                    }
                    Err(StepErr::Stop) => return Err(StepErr::Stop),
                    Err(StepErr::Fail(e)) => { tracing::info!(run_id = %self.run_id, node = %cur, error = %e, "generative step failed"); false }
                }
            } else {
                // Deterministic S0 (COMPUTE / VERIFY / CONTROL).
                let pass = !s_fail && !unmet && match node.primitive_id.as_str() {
                    "ver.check_artifact_existence" => node.inputs.iter().all(|i| out.get(i).is_some_and(|v| !v.trim().is_empty()))
                        && (node.node_id != "R08" || doc.as_ref().is_some_and(|(r, _)| self.ws.root.join("out").join(r.trim_start_matches("fs:").trim_start_matches('/')).exists())),
                    "ver.check_unresolved_failures" => failed_nodes.is_empty(),
                    "ver.behavior_check" => node.inputs.iter().all(|i| out.get(i).is_some_and(|v| !v.is_empty())),
                    "ver.requirement_check" => {
                        // Citations: every `[src:…]` in the draft names a source this run read.
                        let read = out.get("receipt:sources").cloned().unwrap_or_default();
                        input.match_indices("[src:").all(|(i, _)| input[i..].split(']').next().is_some_and(|c| read.contains(&c[5..])))
                    }
                    _ => true,
                };
                if pass {
                    let v = format!("{}:{}", node.primitive_id, allternit_commrails::receipts::jcs::sha256_tagged(input.as_bytes()));
                    for o in &node.outputs { out.entry(o.clone()).or_insert_with(|| v.clone()); }
                    if node.primitive_id == "ctl.checkpoint" {
                        let jk = format!("{}:{cur}:checkpoint:1", self.run_id);
                        if !super::super::safety::journal_value(&self.st.db, &jk, &self.run_id, &cur, "checkpoint", self.epoch, &json!(out))? {
                            return Err(StepErr::Stop);
                        }
                    }
                }
                pass
            };
            self.close_node(&ran, &plan, ok)?;
            if ok {
                if let Some(c) = evidence_of(&node) {
                    if !script.withhold.contains(c) {
                        let rid = self.verify_receipt(&cur, c, true, json!({ "criterion": c, "proof": proof }))?;
                        evidence.push(format!("{c}:receipt:{rid}"));
                    }
                }
                match next(&self.graph, &cur, "NORMAL") {
                    Some(n) => { cur = n; continue; }
                    None if self.graph.completion_nodes.contains(&cur) => return self.complete_task(t, &evidence, doc),
                    None => return Err(StepErr::Fail(anyhow!("{cur} has no successor and is not a completion node"))),
                }
            }
            failed_nodes.push(cur.clone());
            let is_gate = node.primitive_id == GATE_PRIMITIVE;
            let strategy = node.on_failure.strategy.clone();
            match next(&self.graph, &cur, "FAILURE") {
                Some(target) if matches!(strategy.as_str(), "FALLBACK" | "ESCALATE") && !is_gate => cur = target,
                Some(target) => {
                    // ROLLBACK, or the completion gate asking for evidence: run the target, then stop.
                    let (tr, tp) = self.route_node(&target)?;
                    self.close_node(&tr, &tp, true)?;
                    let missing = out.get("missing").cloned().unwrap_or_default();
                    let why = if is_gate { format!("verifier: missing evidence for {missing}") } else { format!("{cur} failed ({strategy}); {target} ran") };
                    return self.fail_task(t, &evidence, &why, is_gate);
                }
                None => return self.fail_task(t, &evidence, &format!("{cur} failed"), false),
            }
        }
        Err(StepErr::Fail(anyhow!("step cap reached")))
    }

    fn criteria_patch(&self, evidence: &[String], status: &str) -> Result<Value> {
        let rec = self.h.block_on(self.s.load_run(&self.run_id))?.ok_or_else(|| anyhow!("run vanished"))?;
        let criteria: Vec<Value> = rec.run["completion"]["criteria"].as_array().cloned().unwrap_or_default().into_iter().map(|mut c| {
            let id = c["criterion"].as_str().unwrap_or_default().to_string();
            let ok = evidence.iter().find(|e| e.starts_with(&format!("{id}:")));
            c["result"] = json!(if ok.is_some() { "pass" } else { "fail" });
            c["receipt_ids"] = json!(ok.map(|e| vec![e.rsplit(':').next().unwrap_or_default().to_string()]).unwrap_or_default());
            c
        }).collect();
        Ok(json!({ "completion": { "status": status, "required": rec.run["completion"]["required"], "criteria": criteria } }))
    }

    fn fail_task(&mut self, _t: &TaskType, evidence: &[String], why: &str, unverified: bool) -> Step<()> {
        let patch = self.criteria_patch(evidence, if unverified { "unverified" } else { "failed" })?;
        self.run_receipt("failed", "")?;
        self.h.block_on(finish(self.s, &self.run_id, "failed", why, Some(patch)))?;
        Ok(())
    }

    fn complete_task(&mut self, t: &TaskType, evidence: &[String], doc: Option<(String, String)>) -> Step<()> {
        let mut patch = self.criteria_patch(evidence, "verified")?;
        let mut arts = vec![];
        if let Some((r, content)) = doc {
            let art = json!({ "id": new_id("art"), "object": "artifact", "run_id": self.run_id, "name": r.trim_start_matches("fs:"),
                "kind": "document", "mime_type": "text/markdown", "hash": allternit_commrails::receipts::jcs::sha256_tagged(content.as_bytes()),
                "size_bytes": content.len(), "verification_status": "verified", "created_at": now(),
                "content": if content.len() <= 256 * 1024 { json!(content) } else { Value::Null } });
            self.emit("artifact.created", art.clone())?;
            arts.push(art["id"].clone());
        }
        let rid = self.run_receipt("completed", "")?;
        patch["completion"]["run_receipt_id"] = json!(rid);
        patch["output"] = json!({ "summary": format!("Done; verified by {} receipt-backed criteria.", evidence.len()), "artifact_ids": arts });
        let why = format!("verified by {}", t.completion_policy_id());
        self.h.block_on(finish(self.s, &self.run_id, "completed", &why, Some(patch)))?;
        Ok(())
    }
}
