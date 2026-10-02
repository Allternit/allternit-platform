//! WP-C3a: the `template:` effect connector (TEMPLATE).
//!
//! Starts (or advances) a kernel-UI template run for the run's owner through
//! the existing templates code (`kernel_ui::templates::start_run`: owner-scoped
//! lookup, same parking rules) and drives it to a terminal state in-line:
//! model templates on `template_exec::drive` (its own fencing token, journaled
//! steps), S0-only templates on the local S0 driver (only when local exec is
//! on). The effect succeeds only when the child run completed (verifier-owned
//! `all_pass`); anything else fails closed with the child's status.
//!
//! Called only from inside `Exec::effect_with` (P1's fenced, prepared →
//! committed journal), so a committed effect is never re-applied. The child
//! run also carries the effect's stable idempotency key: a takeover after a
//! crash finds the same child instead of starting a second one, and a model
//! child still queued is driven on (re-driving is replay-safe).

use crate::agency_api::store::AgencyStore;
use crate::kernel_ui::templates::{drive_s0, start_run, QUEUED};
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use tokio::runtime::Handle;

/// Run template `template_id` for `owner`; returns `template:<id>:run:<child>`.
pub fn run(h: &Handle, st: &AppState, owner: &str, org: &str, parent: &str, template_id: &str, key: &str, inputs: Value) -> Result<String> {
    if template_id.is_empty() || template_id.len() > 128 || !template_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        bail!("bad template id `{template_id}`");
    }
    let s = AgencyStore::new(st.rails.ledger.clone());
    let idem = format!("agency:{key}");
    let (child, fresh_s0) = match h.block_on(s.find_by_idempotency(owner, &idem))? {
        Some(rec) => (rec, None),
        None => {
            let body = json!({ "inputs": inputs, "thread_id": format!("thr_{parent}") });
            let (rec, steps, _) = h.block_on(start_run(st, owner, org, template_id, &body, Some(idem), true))
                .map_err(|e| anyhow!("template {template_id}: {}", e.1))?;
            (rec, Some(steps))
        }
    };
    let id = child.run["id"].as_str().unwrap_or_default().to_string();
    let (status, reason) = (child.run["status"].as_str().unwrap_or_default(), child.run["status_reason"].as_str().unwrap_or_default());
    match (status, fresh_s0) {
        // G2: the child stopped for a person. Still open → park the parent on
        // it (not a failure). Answered → re-queue the child and drive it on
        // (its journal resumes after the answered step); rejected → it fails.
        ("needs_attention", _) => {
            if let Some(att) = child.attention.iter().rev().find(|a| a["status"] == "open") {
                return Err(anyhow::Error::new(NeedsPerson { child_run: id, attention: att.clone() }));
            }
            let rejected = child.attention.iter().rev().find(|a| a["status"] == "resolved")
                .is_some_and(|a| a["resolution"]["outcome"] == "rejected");
            if requeue_child(h, &s, &id, rejected)? { crate::agency_api::template_exec::drive(h, st, &s, &id, org)?; }
        }
        ("waiting", _) if reason == QUEUED => crate::agency_api::template_exec::drive(h, st, &s, &id, org)?,
        ("running", Some(steps)) => drive_s0(h, &s, &id, &steps),
        ("running", None) => bail!("template run {id} is still running from an earlier attempt (fail closed)"),
        _ => {}
    }
    let done = h.block_on(s.load_run(&id))?.ok_or_else(|| anyhow!("template run {id} vanished"))?;
    match done.run["status"].as_str().unwrap_or_default() {
        "completed" => Ok(format!("template:{template_id}:run:{id}")),
        "needs_attention" => match done.attention.iter().rev().find(|a| a["status"] == "open") {
            Some(att) => Err(anyhow::Error::new(NeedsPerson { child_run: id, attention: att.clone() })),
            None => bail!("template run {id} is waiting on a person with no open request (fail closed)"),
        },
        st => bail!("template run {id} ended `{st}`: {}", done.run["status_reason"].as_str().unwrap_or_default()),
    }
}

/// G2: attention reason of a parent run parked on its child template's request.
pub const CHILD_ATTENTION_REASON: &str = "template_child_attention";

/// G2: the child template run stopped for a person (an open attention item).
/// Not a failure: the executor parks the parent run on it (linked by the
/// child's attention id) and leaves the effect retryable, so the answer
/// resumes the parent, which re-checks the same child (idempotency key).
#[derive(Debug)]
pub struct NeedsPerson {
    pub child_run: String,
    pub attention: Value,
}

impl std::fmt::Display for NeedsPerson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "template run {} needs a person: {}", self.child_run, self.attention["title"].as_str().unwrap_or("attention"))
    }
}

impl std::error::Error for NeedsPerson {}

/// The child's request was answered: queue it for its next drive, or fail it
/// on a rejection. `false` = nothing to drive (someone else moved it on).
fn requeue_child(h: &Handle, s: &AgencyStore, id: &str, rejected: bool) -> Result<bool> {
    h.block_on(async {
        let _g = s.lock().await;
        let Some(rec) = s.load_run(id).await? else { bail!("template run {id} vanished") };
        if rec.run["status"] != "needs_attention" { return Ok(false); }
        if rejected {
            s.transition(rec, "failed", Some("stopped by the approver")).await?;
            return Ok(false);
        }
        s.transition(rec, "waiting", Some(QUEUED)).await?;
        Ok(true)
    })
}

/// Test fixture: an owner-scoped model template ending in a verifier.
#[cfg(test)]
pub(crate) fn seed(db: &crate::db::DbHandle, id: &str, owner: &str, steps: Value) {
    let uses_model = steps.as_array().is_some_and(|a| a.iter().any(|s| matches!(s["kind"].as_str(), Some("s1_decision" | "s2_generate"))));
    let doc = json!({ "id": id, "name": id, "category": "general", "slash": id, "scope": format!("user:{owner}"), "steps": steps,
        "completion": "all_pass", "auto_run": null, "reproducible": false, "uses_model": uses_model, "pinned_inputs": null });
    db.connect().unwrap().execute("INSERT INTO kernel_ui_templates (id, owner_id, scope, doc_json, updated_at) VALUES (?1, ?2, ?3, ?4, 'x')",
        rusqlite::params![id, owner, format!("user:{owner}"), doc.to_string()]).unwrap();
}

#[cfg(test)]
pub(crate) fn model_steps() -> Value {
    json!([{ "kind": "s2_generate", "label": "Draft", "capability": null }, { "kind": "verifier", "label": "Check", "capability": null }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agency_api::tests::{setup, E2E_ENV};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn starts_once_per_key_and_fails_closed() {
        let _env = E2E_ENV.lock().await;
        let runs = tempfile::tempdir().unwrap();
        std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
        std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
        let t = setup().await;
        seed(&t.st.db, "tpl-ok", "u1", model_steps());
        seed(&t.st.db, "tpl-bad", "u1", json!([{ "kind": "s2_generate", "label": "Draft", "capability": null }]));
        let st = t.st.clone();
        tokio::task::spawn_blocking(move || {
            let h = Handle::current();
            let a = run(&h, &st, "u1", "user:u1", "run_p", "tpl-ok", "run_p:P03:template:tpl-ok", json!({})).unwrap();
            let b = run(&h, &st, "u1", "user:u1", "run_p", "tpl-ok", "run_p:P03:template:tpl-ok", json!({})).unwrap();
            assert_eq!(a, b, "replay finds the same child run (no second start)");
            assert!(a.starts_with("template:tpl-ok:run:"), "{a}");
            assert!(run(&h, &st, "u2", "user:u2", "run_q", "tpl-ok", "k2", json!({})).is_err(), "someone else's template");
            assert!(run(&h, &st, "u1", "user:u1", "run_q", "nope", "k3", json!({})).is_err(), "missing template");
            assert!(run(&h, &st, "u1", "user:u1", "run_q", "tpl-bad", "k4", json!({})).is_err(), "no verifier: parked, fails closed");
            assert!(run(&h, &st, "u1", "user:u1", "run_q", "../x", "k5", json!({})).is_err(), "bad id");
            // Through P1's fenced path: replay = no second child; stale fence = never called.
            use crate::agency_api::{effect_thread::fenced, safety::acquire_fence};
            let (db, key) = (&st.db, "run_f:P03:tool.execute:1");
            let e1 = acquire_fence(db, "run_f", "w1").unwrap();
            let calls = std::cell::Cell::new(0);
            let apply = || { calls.set(calls.get() + 1); run(&h, &st, "u1", "user:u1", "run_f", "tpl-ok", key, json!({})) };
            let x = fenced(db, "run_f", key, e1, apply).unwrap();
            assert_eq!((fenced(db, "run_f", key, e1, apply).unwrap(), calls.get()), (x, 1));
            let _e2 = acquire_fence(db, "run_f", "w2").unwrap();
            assert!(fenced(db, "run_f", "run_f:P03:tool.execute:2", e1, apply).is_none());
            assert_eq!(calls.get(), 1, "connector never ran under a stale fence");
        }).await.unwrap();
    }

    /// G2: a child template that stops for a person parks the parent run
    /// (linked to the child's item) instead of failing it; the answer —
    /// given on the parent's item or on the child's — resumes the parent,
    /// which re-checks the same child under the fence; only a real failure
    /// (a rejection) fails the effect.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn child_attention_parks_the_parent_and_its_answer_resumes_it() {
        use crate::agency_api::tests::{call, post, run_limits, wait_settled};
        use crate::agency_api::{executor, guard, task_types};
        use axum::http::StatusCode;
        let _env = E2E_ENV.lock().await;
        let runs = tempfile::tempdir().unwrap();
        std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
        std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
        // Superset of the default (BUG_FIX stays on): parallel tests share the env.
        std::env::set_var(task_types::ENABLE_ENV, format!("BUG_FIX,{}", task_types::ids().join(",")));
        let t = setup().await;
        let s = AgencyStore::new(t.st.rails.ledger.clone());
        let limits = run_limits(&[(guard::ORGS_ENV, "user:u1"), (guard::ORG_RUNS_PER_HOUR_ENV, "1000")]);
        // A model template whose middle step stops for a person.
        seed(&t.st.db, "tpl-att", "u1", json!([
            { "kind": "s2_generate", "label": "Draft", "capability": null },
            { "kind": "attention", "label": "ok?", "capability": null },
            { "kind": "verifier", "label": "Check", "capability": null },
        ]));

        async fn start_parent(t: &crate::agency_api::tests::T, runs: &tempfile::TempDir, key: &str) -> String {
            let (st, _, b) = call(&t.app, post("/v1/agency", "u1", Some(key), json!({
                "goal": "run the template", "task_type": "TEMPLATE", "workspace": { "resources": ["template:tpl-att"] } }))).await;
            assert_eq!(st, StatusCode::ACCEPTED, "{b}");
            let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
            std::fs::create_dir_all(runs.path().join(&id)).unwrap();
            std::fs::write(runs.path().join(&id).join("scripted.json"), json!({ "live": true }).to_string()).unwrap();
            id
        }

        async fn answer(t: &crate::agency_api::tests::T, att_id: &str, kind: &str) -> Value {
            let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att_id}/responses"), "u1", None, json!({ "type": kind }))).await;
            assert_eq!(st, StatusCode::OK, "{b}");
            serde_json::from_str(&b).unwrap()
        }

        async fn parked(t: &crate::agency_api::tests::T, s: &AgencyStore, runs: &tempfile::TempDir, limits: &guard::Limits, key: &str)
            -> (String, Value, String, String) {
            let id = start_parent(t, runs, key).await;
            assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await, "admitted");
            let rec = wait_settled(s, &id).await;
            assert_eq!(rec.run["status"], "needs_attention", "{}", rec.run);
            let att = rec.attention.iter().find(|a| a["status"] == "open").cloned().expect("an open item");
            assert_eq!(att["reason"], CHILD_ATTENTION_REASON, "{att}");
            let (child, child_att) = (att["child_run_id"].as_str().unwrap().to_string(), att["child_attention_id"].as_str().unwrap().to_string());
            let c = s.load_run(&child).await.unwrap().expect("the child run");
            assert_eq!(c.run["status"], "needs_attention", "{}", c.run);
            assert!(c.attention.iter().any(|a| a["id"] == child_att.as_str() && a["status"] == "open" && a["reason"] == "template_attention"),
                "the child parked on its attention step: {:?}", c.attention);
            (id, att, child, child_att)
        }

        // (a) Approve on the parent's item: the answer forwards to the child,
        // the parent re-queues, re-checks the same child and completes.
        let (id, att, child, _) = parked(&t, &s, &runs, &limits, "g2-att-a1").await;
        let r = answer(&t, att["id"].as_str().unwrap(), "approval").await;
        assert_eq!(r["run"]["status"], "waiting", "{r}");
        assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await, "re-admitted");
        let rec = wait_settled(&s, &id).await;
        assert_eq!(rec.run["status"], "completed", "{}", rec.run);
        assert_eq!(rec.run["completion"]["status"], "verified");
        let c = s.load_run(&child).await.unwrap().unwrap();
        assert_eq!(c.run["status"], "completed", "the re-check drove the same child: {}", c.run);
        // One parent item, one child run, one committed effect: the replayed
        // re-drive after the answer never re-applied the effect.
        assert_eq!(rec.attention.iter().filter(|a| a["reason"] == CHILD_ATTENTION_REASON).count(), 1, "{:?}", rec.attention);
        let kids = s.runs_with_status(&["completed"]).await.unwrap().into_iter()
            .filter(|r| r.run["agent"] == "template" && r.run["metadata"]["template_id"] == "tpl-att").count();
        assert_eq!(kids, 1, "one child template run for this parent");
        let j = crate::agency_api::safety::journal(&t.st.db, &id).unwrap();
        assert_eq!(j.iter().filter(|r| r["tool"] == "tool.execute" && r["status"] == "committed").count(), 1,
            "one committed effect, never re-applied after the answer: {j:?}");

        // (b) Approve on the child's item itself: the linked parent item is
        // resolved with the same answer and the parent is released.
        let (id, att, child, child_att) = parked(&t, &s, &runs, &limits, "g2-att-b1").await;
        let r = answer(&t, &child_att, "approval").await;
        assert_eq!(r["attention_id"], child_att);
        let rec = s.load_run(&id).await.unwrap().unwrap();
        let pa = rec.attention.iter().find(|a| a["id"].as_str() == att["id"].as_str()).unwrap();
        assert_eq!(pa["status"], json!("resolved"), "parent item released");
        assert_eq!(pa["resolution"]["outcome"], json!("approved"));
        assert_eq!(rec.run["status"], "waiting", "{}", rec.run);
        assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await, "re-admitted");
        let rec = wait_settled(&s, &id).await;
        assert_eq!(rec.run["status"], "completed", "{}", rec.run);
        assert_eq!(s.load_run(&child).await.unwrap().unwrap().run["status"], "completed");

        // (c) A rejection is a real failure: the child fails, the effect fails.
        let (id, att, child, _) = parked(&t, &s, &runs, &limits, "g2-att-c1").await;
        answer(&t, att["id"].as_str().unwrap(), "rejection").await;
        assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await, "re-admitted");
        let rec = wait_settled(&s, &id).await;
        assert_eq!(rec.run["status"], "failed", "{}", rec.run);
        assert_eq!(s.load_run(&child).await.unwrap().unwrap().run["status"], "failed", "the rejection failed the child");
    }
}
