//! Section 4: templates. One JSON document per template, owner-scoped.
//!
//! Runs: a template with only S0 steps (s0_command, verifier) runs locally
//! with no model. Because a template carries argv to execute, local execution
//! is OFF unless `ALLTERNIT_KERNEL_UI_S0_EXEC=1` (the run is parked `waiting`
//! with a reason otherwise). Commands run with no shell, a cleared environment
//! (PATH/LANG only), a per-run scratch dir, and a 60s timeout. Templates with
//! model steps (s1_decision, s2_generate) park per `ALLTERNIT_AGENCY_EXECUTE`;
//! the kernel executor has no template-graph path yet, so they never start.
//! attention / wait / parallel steps are storable but not yet executable.

use super::*;
use crate::agency_api::{executor, store::{new_id, AgencyStore, RunRecord}};
use axum::extract::{Path, Query, State};
use axum::Extension;
use serde::Deserialize;

pub const S0_EXEC_ENV: &str = "ALLTERNIT_KERNEL_UI_S0_EXEC";
const KINDS: &[&str] = &["s0_command", "s1_decision", "s2_generate", "verifier", "attention", "wait", "parallel"];

#[derive(Deserialize)]
pub struct Q {
    scope: Option<String>,
}

fn text(b: &Value, k: &str, max: usize) -> Option<String> {
    b.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty() && s.len() <= max).map(str::to_string)
}

/// Validate a template body and derive the server-owned fields.
fn build(id: &str, b: &Value, scope: &str) -> Result<Value, KErr> {
    let name = text(b, "name", 120).ok_or_else(|| KErr::bad("name is required"))?;
    let slash = text(b, "slash", 32).filter(|s| s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')).ok_or_else(|| KErr::bad("slash must be 1-32 chars of [a-z0-9-]"))?;
    let steps = b["steps"].as_array().filter(|a| !a.is_empty() && a.len() <= 50).ok_or_else(|| KErr::bad("steps must be a non-empty array"))?;
    let mut out = vec![];
    for s in steps {
        let kind = s["kind"].as_str().filter(|k| KINDS.contains(k)).ok_or_else(|| KErr::bad("unknown step kind"))?;
        let label = text(s, "label", 200).ok_or_else(|| KErr::bad("every step needs a label"))?;
        let mut step = json!({ "kind": kind, "label": label, "capability": s.get("capability").cloned().unwrap_or(Value::Null) });
        if kind == "s0_command" {
            let argv = s["command"].as_array().filter(|a| !a.is_empty() && a.len() <= 64 && a.iter().all(|x| x.as_str().is_some_and(|x| !x.is_empty())))
                .ok_or_else(|| KErr::bad("s0_command needs command: a non-empty argv array of strings"))?;
            step["command"] = json!(argv);
        }
        out.push(step);
    }
    if b.get("completion").is_some_and(|c| c != "all_pass") {
        return Err(KErr::bad("completion must be all_pass"));
    }
    let auto_run = match b.get("auto_run") {
        None | Some(Value::Null) => Value::Null,
        Some(a) if a.is_object() => json!({ "before_done_of": a.get("before_done_of").and_then(Value::as_str) }),
        _ => return Err(KErr::bad("auto_run must be null or an object")),
    };
    // Reproducible only when pinned inputs are declared (server-derived).
    let pinned = b.get("pinned_inputs").filter(|p| p.as_object().is_some_and(|o| !o.is_empty()) || p.as_array().is_some_and(|a| !a.is_empty())).cloned();
    let uses_model = out.iter().any(|s| matches!(s["kind"].as_str(), Some("s1_decision" | "s2_generate")));
    Ok(json!({ "id": id, "name": name, "category": text(b, "category", 60).unwrap_or_else(|| "general".into()), "slash": slash,
        "scope": scope, "steps": out, "completion": "all_pass", "auto_run": auto_run,
        "reproducible": pinned.is_some(), "uses_model": uses_model, "pinned_inputs": pinned }))
}

fn load(c: &Connection, id: &str, owner: &str) -> Result<Value, KErr> {
    let raw: Option<String> = c.query_row("SELECT doc_json FROM kernel_ui_templates WHERE id = ?1 AND owner_id = ?2", params![id, owner], |r| r.get(0)).optional()?;
    raw.and_then(|s| serde_json::from_str(&s).ok()).ok_or_else(|| KErr::not_found("template not found"))
}

fn slash_taken(c: &Connection, owner: &str, scope: &str, slash: &str, except: &str) -> Result<bool, KErr> {
    let mut st = c.prepare("SELECT id, doc_json FROM kernel_ui_templates WHERE owner_id = ?1 AND scope = ?2")?;
    let rows: Vec<(String, String)> = st.query_map(params![owner, scope], |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().collect();
    Ok(rows.iter().any(|(i, d)| i != except && serde_json::from_str::<Value>(d).is_ok_and(|v| v["slash"] == json!(slash))))
}

fn scope_of(q: &Q, u: &AuthUser, b: Option<&Value>) -> Result<String, KErr> {
    let s = q.scope.clone().or_else(|| b.and_then(|b| b["scope"].as_str().map(str::to_string)))
        .unwrap_or_else(|| format!("org:{}", u.organization_id.clone().filter(|o| !o.is_empty()).unwrap_or_else(|| "default".into())));
    authorize_scope(&s, u)?;
    Ok(s)
}

pub async fn list(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> KRes {
    let scope = q.scope.clone();
    if let Some(s) = &scope {
        authorize_scope(s, &u)?;
    }
    Ok(Json(Value::Array(blocking(st.db.clone(), move |c| {
        let mut stmt = c.prepare("SELECT doc_json FROM kernel_ui_templates WHERE owner_id = ?1 AND (?2 IS NULL OR scope = ?2) ORDER BY updated_at")?;
        let rows = stmt.query_map(params![u.user_id, scope], |r| r.get::<_, String>(0))?;
        Ok(rows.flatten().filter_map(|s| serde_json::from_str::<Value>(&s).ok()).collect())
    }).await?)))
}

pub async fn create(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>, Json(b): Json<Value>) -> KRes {
    let scope = scope_of(&q, &u, Some(&b))?;
    let id = new_id("tpl");
    let doc = build(&id, &b, &scope)?;
    Ok(Json(blocking(st.db.clone(), move |c| {
        if slash_taken(c, &u.user_id, &scope, doc["slash"].as_str().unwrap_or_default(), &id)? {
            return Err(KErr::conflict("slash already used in this scope"));
        }
        c.execute("INSERT INTO kernel_ui_templates (id, owner_id, scope, doc_json, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, u.user_id, scope, doc.to_string(), now()])?;
        Ok(doc)
    }).await?))
}

pub async fn update(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>, Query(q): Query<Q>, Json(b): Json<Value>) -> KRes {
    Ok(Json(blocking(st.db.clone(), move |c| {
        let old = load(c, &id, &u.user_id)?;
        let asked = q.scope.clone().or_else(|| b["scope"].as_str().map(str::to_string));
        let scope = match asked {
            Some(s) => { authorize_scope(&s, &u)?; s }
            None => old["scope"].as_str().unwrap_or_default().to_string(),
        };
        let doc = build(&id, &b, &scope)?;
        if slash_taken(c, &u.user_id, &scope, doc["slash"].as_str().unwrap_or_default(), &id)? {
            return Err(KErr::conflict("slash already used in this scope"));
        }
        c.execute("UPDATE kernel_ui_templates SET scope = ?3, doc_json = ?4, updated_at = ?5 WHERE id = ?1 AND owner_id = ?2", params![id, u.user_id, scope, doc.to_string(), now()])?;
        Ok(doc)
    }).await?))
}

pub async fn remove(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>) -> KRes {
    blocking(st.db.clone(), move |c| {
        if c.execute("DELETE FROM kernel_ui_templates WHERE id = ?1 AND owner_id = ?2", params![id, u.user_id])? == 0 {
            return Err(KErr::not_found("template not found"));
        }
        Ok(())
    }).await?;
    Ok(Json(json!({ "deleted": true })))
}

fn s0_exec_on() -> bool {
    std::env::var(S0_EXEC_ENV).is_ok_and(|v| v == "1")
}

pub async fn run(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>, body: Option<Json<Value>>) -> KRes {
    let b = body.map(|j| j.0).unwrap_or(Value::Null);
    let uid = u.user_id.clone();
    let tpl = blocking(st.db.clone(), move |c| load(c, &id, &uid)).await?;
    let uses_model = tpl["uses_model"] == json!(true);
    let steps = tpl["steps"].as_array().cloned().unwrap_or_default();
    if !uses_model {
        if let Some(bad) = steps.iter().find(|s| matches!(s["kind"].as_str(), Some("attention" | "wait" | "parallel"))) {
            return Err(KErr::bad(format!("step kind {} is not yet executable", bad["kind"].as_str().unwrap_or_default())));
        }
    }
    let org = crate::agency_api::guard::org_of(&u);
    let run_id = new_id("run");
    let ts = crate::agency_api::store::now();
    let thread = b["thread_id"].as_str().map(str::to_string).unwrap_or_else(|| new_id("thr"));
    let run = json!({
        "id": run_id, "object": "run", "status": "accepted", "status_reason": "template run", "terminal": false,
        "agent": "template", "thread_id": thread, "campaign_id": null, "goal": format!("Template /{}", tpl["slash"].as_str().unwrap_or_default()),
        "created_at": ts, "updated_at": ts, "finished_at": null, "version": 0, "budget": {},
        "budget_usage": { "seconds": 0.0, "cost_usd": 0.0, "steps": 0, "spend_halted": false },
        "attention": null, "open_attention_count": 0,
        "completion": { "status": "pending", "required": ["template.steps"], "criteria": [{ "criterion": "template.steps", "result": "pending", "receipt_ids": [] }] },
        "output": null, "cancellation": null, "error": null,
        "links": { "self": format!("/v1/runs/{run_id}"), "events": format!("/v1/runs/{run_id}/events"), "artifacts": format!("/v1/runs/{run_id}/artifacts"),
                   "receipts": format!("/v1/runs/{run_id}/receipts"), "attention": format!("/v1/runs/{run_id}/attention") },
        "metadata": { "template_id": tpl["id"], "inputs": b.get("inputs").cloned().unwrap_or(Value::Null) }, "resolved": {}, "effect_receipt_ids": [],
    });
    let task_ir = json!({ "task_type": "TEMPLATE", "org_id": org, "template_id": tpl["id"] });
    let s = AgencyStore::new(st.rails.ledger.clone());
    let rec = {
        let _g = s.lock().await;
        let rec = s.save(RunRecord { owner: u.user_id.clone(), idempotency_key: None, run, task_ir, attention: vec![] }).await.map_err(KErr::internal)?;
        s.emit(&run_id, 1, "run.status_changed", json!({ "data": { "from": null, "to": "accepted", "terminal": false, "reason": "created" } })).await.map_err(KErr::internal)?;
        let park: Option<String> = if uses_model {
            Some(if executor::enabled() { "template model steps are not yet executable: the kernel executor has no template graph".to_string() } else { executor::PARKED_REASON.to_string() })
        } else if !s0_exec_on() {
            Some(format!("local S0 execution is off on this server ({S0_EXEC_ENV} is not set)"))
        } else {
            None
        };
        match park {
            Some(reason) => s.transition(rec, "waiting", Some(&reason)).await.map_err(KErr::internal)?,
            None => s.transition(rec, "running", Some("running S0 steps locally")).await.map_err(KErr::internal)?,
        }
    };
    if rec.run["status"] == "running" {
        let (s2, rid, h) = (AgencyStore::new(st.rails.ledger.clone()), run_id.clone(), tokio::runtime::Handle::current());
        tokio::task::spawn_blocking(move || drive_s0(&h, &s2, &rid, &steps));
    }
    Ok(Json(json!({ "run_id": run_id, "status": rec.run["status"], "status_reason": rec.run["status_reason"] })))
}

fn run_argv(argv: &[String], dir: &std::path::Path) -> Result<(bool, String), String> {
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(dir).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default()).env("LANG", "C")
        .env("HOME", dir).env("TMPDIR", dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                let mut t = String::from_utf8_lossy(&out.stdout).to_string();
                t.push_str(&String::from_utf8_lossy(&out.stderr));
                return Ok((status.success(), t.chars().take(2000).collect()));
            }
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                return Err("timed out after 60s".into());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
}

/// Runs the S0 steps in order; every step emits `run.progress` with the
/// section-6 speed fields (tokens null/0: no model).
fn drive_s0(h: &tokio::runtime::Handle, s: &AgencyStore, run_id: &str, steps: &[Value]) {
    let dir = std::env::temp_dir().join(format!("kernel-ui-{run_id}"));
    let _ = std::fs::create_dir_all(&dir);
    let (mut ok, mut reason) = (true, String::from("all steps passed"));
    for (i, step) in steps.iter().enumerate() {
        let (started_at, t0) = (crate::agency_api::store::now(), std::time::Instant::now());
        let kind = step["kind"].as_str().unwrap_or_default();
        let passed = if !ok {
            false
        } else if kind == "s0_command" {
            let argv: Vec<String> = step["command"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
            match run_argv(&argv, &dir) {
                Ok((p, _)) => p,
                Err(e) => { reason = e; false }
            }
        } else {
            true // verifier: passes only while every earlier step passed (ok)
        };
        let ms = t0.elapsed().as_millis() as u64;
        if !passed && ok {
            ok = false;
            if reason == "all steps passed" {
                reason = format!("step {} ({}) failed", i + 1, step["label"].as_str().unwrap_or_default());
            }
        }
        let v = h.block_on(s.load_run(run_id)).ok().flatten().and_then(|r| r.run["version"].as_i64()).unwrap_or(0);
        let _ = h.block_on(s.emit(run_id, v, "run.progress", json!({ "data": { "step": step["label"], "kind": kind, "attempt": 1,
            "outcome": if passed { "committed" } else { "failed" }, "started_at": started_at, "duration_ms": ms,
            "tokens_in": null, "tokens_out": null, "tokens": 0, "tok_per_s": null, "wait_ms": 0 } })));
        let _ = h.block_on(s.charge_usage(run_id, ms as f64 / 1000.0, 0.0, 1, 0));
    }
    let _ = std::fs::remove_dir_all(&dir);
    h.block_on(async {
        let _g = s.lock().await;
        if let Ok(Some(mut rec)) = s.load_run(run_id).await {
            rec.run["completion"]["status"] = json!(if ok { "verified" } else { "failed" });
            rec.run["completion"]["criteria"][0]["result"] = json!(if ok { "pass" } else { "fail" });
            let _ = s.transition(rec, if ok { "completed" } else { "failed" }, Some(&reason)).await;
        }
    });
}
