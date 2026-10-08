//! WP-C3b: connector unit tests (replay = no double-apply, fence refusal,
//! approval/trigger required) and executor e2e for COMPUTER_USE (fake
//! computer-use gateway) and CAMPAIGN (in-process).

use super::{campaign, computer};
use crate::agency_api::store::{AgencyStore, EV_CAMPAIGN_STATE};
use crate::agency_api::tests::{call, post, run_limits, setup, wait_settled, E2E_ENV};
use crate::agency_api::{executor, guard, safety, task_types};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Fake computer-use gateway: counts direct-mode executions.
async fn fake_gateway() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = axum::Router::new().route("/v1/computer-use/execute", axum::routing::post(move |axum::Json(b): axum::Json<Value>| {
        let h = h.clone();
        async move {
            assert_eq!(b["mode"], "direct");
            h.fetch_add(1, Ordering::SeqCst);
            axum::Json(json!({ "run_id": b["run_id"], "session_id": b["session_id"], "status": "completed" }))
        }
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, hits)
}

/// P1's protocol as `Exec::effect_with` runs it: prepare under the fence,
/// apply once, commit; a committed key is served, a stale worker stops.
async fn fenced<F: std::future::Future<Output = anyhow::Result<String>>>(db: &crate::db::DbHandle, run: &str, epoch: i64, key: &str, f: F) -> Result<String, &'static str> {
    match safety::prepare(db, key, run, "N", "tool.execute", "h", epoch, false).unwrap() {
        safety::Prepared::Committed(prev) => Ok(prev),
        safety::Prepared::Stale => Err("stale"),
        safety::Prepared::Fresh { .. } => {
            let r = f.await.map_err(|_| "failed")?;
            if !safety::commit(db, key, run, epoch, &r).unwrap() { return Err("stale"); }
            Ok(r)
        }
        _ => Err("other"),
    }
}

fn shot() -> Vec<Value> { vec![json!({ "kind": "screenshot" })] }
fn click() -> Vec<Value> { vec![json!({ "kind": "click", "target": { "x": 5, "y": 6 } })] }

#[tokio::test]
async fn computer_connector_replay_fence_and_approval() {
    let _env = E2E_ENV.lock().await;
    let t = setup().await;
    let (url, hits) = fake_gateway().await;
    *computer::ACU_URL_OVERRIDE.lock().unwrap() = Some(url);
    let db = &t.st.db;
    // Replay: the second drive serves the committed result, no second action.
    let e1 = safety::acquire_fence(db, "run_c3b_cu", "w1").unwrap();
    let r1 = fenced(db, "run_c3b_cu", e1, "k1", computer::dispatch(&t.st, "u1", "local", &shot(), false, "k1")).await.unwrap();
    let r2 = fenced(db, "run_c3b_cu", e1, "k1", computer::dispatch(&t.st, "u1", "local", &shot(), false, "k1")).await.unwrap();
    assert_eq!((r1.clone(), hits.load(Ordering::SeqCst)), (r2, 1), "replay must not act twice");
    assert!(r1.starts_with("computer:local:"));
    // Fence: a worker whose token was taken over never reaches the computer.
    let e2 = safety::acquire_fence(db, "run_c3b_cu", "w2").unwrap();
    assert_eq!(fenced(db, "run_c3b_cu", e1, "k2", computer::dispatch(&t.st, "u1", "local", &shot(), false, "k2")).await, Err("stale"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    // Approval: a click without an approval is refused before dispatch.
    let e = computer::dispatch(&t.st, "u1", "local", &click(), false, "k3").await.unwrap_err();
    assert!(e.to_string().contains("approval required"), "{e}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    fenced(db, "run_c3b_cu", e2, "k3", computer::dispatch(&t.st, "u1", "local", &click(), true, "k3")).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    // Someone else's (or an unknown) computer is not reachable.
    assert!(computer::dispatch(&t.st, "u1", "cmp_not_mine", &shot(), false, "k4").await.is_err());
    // Control lease on this Mac: the agent takes it for input; once the
    // person takes over, agent input is refused (423) and never dispatched,
    // while read-only observing still works.
    {
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
             VALUES ('cmp_mac', 'local', 'host', 'running', 'user', 'u1', 'Mac', 'macos', 'dev-1', 'free')",
            [],
        ).unwrap();
    }
    let e3 = safety::acquire_fence(db, "run_c3b_cu", "w3").unwrap();
    fenced(db, "run_c3b_cu", e3, "k5", computer::dispatch(&t.st, "u1", "local", &click(), true, "k5")).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    {
        let conn = db.connect().unwrap();
        let held = crate::computer_control_lease::current(&conn, "cmp_mac", chrono::Utc::now().timestamp()).unwrap().unwrap();
        assert_eq!(held.holder.kind, crate::computer_control_lease::HolderKind::Agent);
        let me = crate::computer_control_lease::Holder { kind: crate::computer_control_lease::HolderKind::User, id: "u1".into(), label: Some("Eoj".into()), device_id: Some("dev-1".into()) };
        crate::computer_control_lease::take(&conn, "cmp_mac", &me, chrono::Utc::now().timestamp()).unwrap();
    }
    let locked = computer::dispatch(&t.st, "u1", "local", &click(), true, "k6").await.unwrap_err();
    assert!(locked.to_string().starts_with(computer::LOCKED_PREFIX), "{locked}");
    assert_eq!(hits.load(Ordering::SeqCst), 3, "a locked agent click never reaches the computer");
    assert!(computer::dispatch(&t.st, "u1", "local", &shot(), false, "k7").await.is_ok());
    *computer::ACU_URL_OVERRIDE.lock().unwrap() = None;
}

async fn new_campaign(t: &crate::agency_api::tests::T, user: &str) -> String {
    let (st, _, b) = call(&t.app, post("/v1/campaigns", user, None, json!({ "objective": "Grow the newsletter" }))).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn campaign_connector_replay_fence_and_trigger() {
    let t = setup().await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let db = &t.st.db;
    let cmp = new_campaign(&t, "u1").await;
    let load = || async { s.load_object(EV_CAMPAIGN_STATE, &cmp, "u1").await.unwrap().unwrap() };
    // Q19: no explicit trigger, no cycle.
    let e = campaign::advance(&s, "u1", &cmp, "run_a", "ka", "step", false).await.unwrap_err();
    assert!(e.to_string().contains("explicit trigger"), "{e}");
    assert!(load().await["cycles"].is_null());
    // Replay: one cycle however often the effect is re-driven.
    let e1 = safety::acquire_fence(db, "run_a", "w1").unwrap();
    let r1 = fenced(db, "run_a", e1, "ka", campaign::advance(&s, "u1", &cmp, "run_a", "ka", "step", true)).await.unwrap();
    let r2 = fenced(db, "run_a", e1, "ka", campaign::advance(&s, "u1", &cmp, "run_a", "ka", "step", true)).await.unwrap();
    let r3 = campaign::advance(&s, "u1", &cmp, "run_a", "ka", "step", true).await.unwrap(); // connector-level dedupe too
    assert_eq!((&r1, &r2), (&r3, &r1));
    let c = load().await;
    assert_eq!(c["cycles"].as_array().unwrap().len(), 1);
    assert_eq!(c["budget_usage"]["steps"], 1);
    assert_eq!(c["run_ids"], json!(["run_a"]));
    assert_eq!((c["scheduling"].as_str(), c["next_wake"].is_null()), (Some("disabled_pending_golive"), true), "never self-wakes");
    // Fence: a stale worker never advances the campaign.
    safety::acquire_fence(db, "run_a", "w2").unwrap();
    assert_eq!(fenced(db, "run_a", e1, "kb", campaign::advance(&s, "u1", &cmp, "run_a", "kb", "step", true)).await, Err("stale"));
    assert_eq!(load().await["cycles"].as_array().unwrap().len(), 1);
    // Owner scoping, paused campaigns, budget.
    assert!(campaign::advance(&s, "u2", &cmp, "run_b", "kc", "step", true).await.is_err(), "another user's campaign is not found");
    let (st, _, _) = call(&t.app, post(&format!("/v1/campaigns/{cmp}/pause"), "u1", None, json!({}))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(campaign::advance(&s, "u1", &cmp, "run_b", "kc", "step", true).await.unwrap_err().to_string().contains("paused"));
    let mut c = load().await;
    c["status"] = json!("active");
    c["budget"]["max_steps"] = json!(1);
    s.save_object(EV_CAMPAIGN_STATE, &cmp, "u1", &c).await.unwrap();
    assert!(campaign::advance(&s, "u1", &cmp, "run_b", "kc", "step", true).await.unwrap_err().to_string().contains("budget"));
}

// ── executor e2e ────────────────────────────────────────────────────────────

struct Env { runs: tempfile::TempDir }

fn env() -> Env {
    let runs = tempfile::tempdir().unwrap();
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    // Superset of the default (BUG_FIX stays on): parallel tests share the env.
    std::env::set_var(task_types::ENABLE_ENV, format!("BUG_FIX,{}", task_types::ids().join(",")));
    Env { runs }
}

async fn start(t: &crate::agency_api::tests::T, e: &Env, key: &str, tt: &str, res: &str, script: Value) -> String {
    let (st, _, b) = call(&t.app, post("/v1/agency", "u1", Some(key), json!({ "goal": "do it", "task_type": tt, "workspace": { "resources": [res] } }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    std::fs::create_dir_all(e.runs.path().join(&id)).unwrap();
    std::fs::write(e.runs.path().join(&id).join("scripted.json"), script.to_string()).unwrap();
    id
}

async fn drive(t: &crate::agency_api::tests::T, s: &AgencyStore, id: &str) -> crate::agency_api::store::RunRecord {
    assert!(executor::admit_and_start(t.st.clone(), id.to_string(), run_limits(&[(guard::ORGS_ENV, "user:u1")])).await, "admitted");
    wait_settled(s, id).await
}

async fn answer_open(t: &crate::agency_api::tests::T, rec: &crate::agency_api::store::RunRecord, kind: &str) -> Value {
    let att = rec.attention.iter().find(|a| a["status"] == "open").cloned().unwrap_or_else(|| panic!("no open attention: {}", rec.run));
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{}/responses", att["id"].as_str().unwrap()), "u1", None, json!({ "type": kind }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    att
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn computer_use_runs_through_the_gateway_after_approval() {
    let _env = E2E_ENV.lock().await;
    let e = env();
    let t = setup().await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let (url, hits) = fake_gateway().await;
    *computer::ACU_URL_OVERRIDE.lock().unwrap() = Some(url);
    let action = json!({ "actions": click() }).to_string();
    let id = start(&t, &e, "c3b-cu-0001", "COMPUTER_USE", "computer:local", json!({ "live": true, "outputs": { "C02": action } })).await;
    let rec = drive(&t, &s, &id).await;
    assert_eq!(rec.run["status"], "needs_attention", "{}", rec.run);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no action before the approval");
    let att = answer_open(&t, &rec, "approval").await;
    assert_eq!((att["wait_kind"].as_str(), att["node_id"].as_str(), att["consequential"].as_bool()), (Some("effect_approval"), Some("C04"), Some(true)));
    let rec = drive(&t, &s, &id).await;
    assert_eq!(rec.run["status"], "completed", "{}", rec.run);
    assert_eq!(rec.run["completion"]["status"], "verified");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "acted exactly once");
    let j = safety::journal(&t.st.db, &id).unwrap();
    assert!(j.iter().any(|r| r["node_id"] == "C04" && r["status"] == "committed"), "{j:?}");

    // A rejected action never runs and the run fails through its undo step.
    let id = start(&t, &e, "c3b-cu-0002", "COMPUTER_USE", "computer:local", json!({ "live": true, "outputs": { "C02": action } })).await;
    let rec = drive(&t, &s, &id).await;
    answer_open(&t, &rec, "rejection").await;
    let rec = drive(&t, &s, &id).await;
    assert_eq!(rec.run["status"], "failed", "{}", rec.run);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    *computer::ACU_URL_OVERRIDE.lock().unwrap() = None;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn campaign_cycle_runs_only_on_an_explicit_trigger() {
    let _env = E2E_ENV.lock().await;
    let e = env();
    let t = setup().await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let cmp = new_campaign(&t, "u1").await;
    let id = start(&t, &e, "c3b-cmp-0001", "CAMPAIGN", &format!("campaign:{cmp}"), json!({ "live": true })).await;
    let rec = drive(&t, &s, &id).await;
    assert_eq!(rec.run["status"], "needs_attention", "parks at the wake gate: {}", rec.run);
    let c = s.load_object(EV_CAMPAIGN_STATE, &cmp, "u1").await.unwrap().unwrap();
    assert!(c["cycles"].is_null(), "no cycle without a trigger");
    let att = answer_open(&t, &rec, "approval").await;
    assert_eq!(att["wait_kind"], "wake");
    let rec = drive(&t, &s, &id).await;
    assert_eq!(rec.run["status"], "completed", "{}", rec.run);
    let c = s.load_object(EV_CAMPAIGN_STATE, &cmp, "u1").await.unwrap().unwrap();
    assert_eq!(c["cycles"].as_array().unwrap().len(), 1, "{c}");
    assert_eq!(c["run_ids"], json!([id]));
    assert_eq!(c["scheduling"], "disabled_pending_golive");
    assert!(c["next_wake"].is_null());

    // A paused campaign does not advance even when triggered.
    let (st, _, _) = call(&t.app, post(&format!("/v1/campaigns/{cmp}/pause"), "u1", None, json!({}))).await;
    assert_eq!(st, StatusCode::OK);
    let id = start(&t, &e, "c3b-cmp-0002", "CAMPAIGN", &format!("campaign:{cmp}"), json!({ "live": true })).await;
    let rec = drive(&t, &s, &id).await;
    answer_open(&t, &rec, "approval").await;
    let rec = drive(&t, &s, &id).await;
    assert_eq!(rec.run["status"], "failed", "{}", rec.run);
    let c = s.load_object(EV_CAMPAIGN_STATE, &cmp, "u1").await.unwrap().unwrap();
    assert_eq!(c["cycles"].as_array().unwrap().len(), 1);
}
