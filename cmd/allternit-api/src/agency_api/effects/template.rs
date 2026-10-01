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
        ("waiting", _) if reason == QUEUED => crate::agency_api::template_exec::drive(h, st, &s, &id, org)?,
        ("running", Some(steps)) => drive_s0(h, &s, &id, &steps),
        ("running", None) => bail!("template run {id} is still running from an earlier attempt (fail closed)"),
        _ => {}
    }
    let done = h.block_on(s.load_run(&id))?.ok_or_else(|| anyhow!("template run {id} vanished"))?;
    match done.run["status"].as_str().unwrap_or_default() {
        "completed" => Ok(format!("template:{template_id}:run:{id}")),
        st => bail!("template run {id} ended `{st}`: {}", done.run["status_reason"].as_str().unwrap_or_default()),
    }
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
}
