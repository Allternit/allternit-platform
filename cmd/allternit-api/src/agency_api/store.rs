//! Durable Agency run/campaign/replay store on the commrails ledger.
//!
//! Every mutation appends a full snapshot event (`agency.run.state`, …) to the
//! on-disk ledger, so a Run survives restart and its current state is the last
//! snapshot. Public run events are appended as `agency.event` with a gap-free
//! per-run `seq`; SSE reads them back from the ledger tail. No SQL migration.
//!
//! Budget (Q11): `charge` records usage; on exhaustion it sets
//! `budget_usage.spend_halted = true` in a snapshot BEFORE it emits the
//! threshold event or opens the `budget_exhausted` attention request, and
//! `admit_effect` refuses every effect while halted.

use allternit_factory_engine::core::types::{Actor, ActorType, AllternitEvent, EventScope, LedgerQuery};
use allternit_factory_engine::ledger::Ledger;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub const EV_RUN_STATE: &str = "agency.run.state";
pub const EV_RUN_EVENT: &str = "agency.event";
pub const EV_CAMPAIGN_STATE: &str = "agency.campaign.state";
pub const EV_REPLAY_STATE: &str = "agency.replay.state";

static WRITE_LOCK: Mutex<()> = Mutex::const_new(());

pub const TERMINAL: &[&str] = &["completed", "partial", "failed", "cancelled"];

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// One stored run: owner + public Run + internal records.
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub owner: String,
    pub idempotency_key: Option<String>,
    pub run: Value,
    pub task_ir: Value,
    pub attention: Vec<Value>,
}

#[derive(Debug)]
pub enum EffectDenied {
    NotFound,
    SpendHalted,
    NotRunnable(String),
}

/// In-memory index over the agency events of one ledger. Built from the
/// ledger once (first use), then kept current by every append. The ledger
/// stays the source of truth: a restart rebuilds the index from it.
#[derive(Default)]
struct Inner {
    loaded: bool,
    tick: u64,
    /// run_id -> (latest snapshot, tick of that snapshot)
    runs: HashMap<String, (RunRecord, u64)>,
    /// (owner, idempotency_key) -> run_id of the first snapshot carrying it
    idem: HashMap<(String, String), String>,
    /// run_id -> public events in append order
    events: HashMap<String, Vec<Value>>,
    /// (type, id, owner) -> (object, tick)
    objects: HashMap<(String, String, String), (Value, u64)>,
}

impl Inner {
    fn apply(&mut self, ty: &str, evt: &AllternitEvent) {
        let p = &evt.payload;
        self.tick += 1;
        match ty {
            EV_RUN_STATE => {
                let id = p["run_id"].as_str().unwrap_or_default().to_string();
                let rec = AgencyStore::record_from(evt);
                if let Some(k) = &rec.idempotency_key {
                    self.idem.entry((rec.owner.clone(), k.clone())).or_insert_with(|| id.clone());
                }
                self.runs.insert(id, (rec, self.tick));
            }
            EV_RUN_EVENT => {
                let id = p["run_id"].as_str().unwrap_or_default().to_string();
                self.events.entry(id).or_default().push(p["event"].clone());
            }
            EV_CAMPAIGN_STATE | EV_REPLAY_STATE => {
                let key = (ty.to_string(), p["id"].as_str().unwrap_or_default().to_string(), p["owner"].as_str().unwrap_or_default().to_string());
                self.objects.insert(key, (p["object"].clone(), self.tick));
            }
            _ => {}
        }
    }
}

struct Shared {
    /// Held so the registry key (its pointer) can never be reused.
    _ledger: Arc<Ledger>,
    inner: RwLock<Inner>,
    scans: AtomicUsize,
}

/// One index per ledger: `store(st)` builds an `AgencyStore` per request, so
/// the index must outlive the handle.
static INDEXES: std::sync::Mutex<Option<HashMap<usize, Arc<Shared>>>> = std::sync::Mutex::new(None);

fn shared_for(ledger: &Arc<Ledger>) -> Arc<Shared> {
    let mut g = INDEXES.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .entry(Arc::as_ptr(ledger) as usize)
        .or_insert_with(|| Arc::new(Shared { _ledger: ledger.clone(), inner: RwLock::new(Inner::default()), scans: AtomicUsize::new(0) }))
        .clone()
}

#[derive(Clone)]
pub struct AgencyStore {
    ledger: Arc<Ledger>,
    shared: Arc<Shared>,
}

fn scope(run_id: &str) -> EventScope {
    EventScope { run_id: Some(run_id.to_string()), ..Default::default() }
}

impl AgencyStore {
    pub fn new(ledger: Arc<Ledger>) -> Self {
        let shared = shared_for(&ledger);
        Self { ledger, shared }
    }

    /// Number of full-ledger scans done to build the index (test hook: stays
    /// at 1 no matter how many gets/lists follow).
    pub fn ledger_scans(&self) -> usize {
        self.shared.scans.load(Ordering::SeqCst)
    }

    /// Read access to the index, building it from the ledger on first use.
    async fn index(&self) -> anyhow::Result<tokio::sync::RwLockReadGuard<'_, Inner>> {
        loop {
            let g = self.shared.inner.read().await;
            if g.loaded {
                return Ok(g);
            }
            drop(g);
            let mut w = self.shared.inner.write().await;
            if !w.loaded {
                self.load_into(&mut w).await?;
            }
        }
    }

    async fn load_into(&self, w: &mut Inner) -> anyhow::Result<()> {
        self.shared.scans.fetch_add(1, Ordering::SeqCst);
        for e in self.ledger.query(LedgerQuery::default()).await? {
            let ty = e.r#type.clone();
            w.apply(&ty, &e);
        }
        w.loaded = true;
        Ok(())
    }

    async fn append(&self, ty: &str, run_id: Option<&str>, payload: Value) -> anyhow::Result<()> {
        // Write lock across append + index update keeps the two in step.
        let mut w = self.shared.inner.write().await;
        if !w.loaded {
            self.load_into(&mut w).await?;
        }
        let evt = AllternitEvent {
            event_id: String::new(),
            ts: String::new(),
            actor: Actor { r#type: ActorType::Gate, id: "agency-api".into() },
            scope: run_id.map(scope),
            r#type: ty.into(),
            payload,
            provenance: None,
        };
        self.ledger.append(evt.clone()).await?;
        w.apply(ty, &evt);
        Ok(())
    }

    async fn of_type(&self, ty: &str) -> anyhow::Result<Vec<AllternitEvent>> {
        self.ledger.query(LedgerQuery { r#type: Some(ty.into()), ..Default::default() }).await
    }

    /// Append a raw ledger event (e.g. `JudgePolicySet` for the run's DAG).
    pub async fn append_raw(&self, ty: &str, run_id: &str, payload: Value) -> anyhow::Result<()> {
        self.append(ty, Some(run_id), payload).await
    }

    /// Ledger events of one type (cheaper than `raw_events` on a large ledger).
    pub async fn events_of_type(&self, ty: &str) -> anyhow::Result<Vec<AllternitEvent>> {
        self.of_type(ty).await
    }

    pub async fn raw_events(&self) -> anyhow::Result<Vec<AllternitEvent>> {
        self.ledger.query(LedgerQuery::default()).await
    }

    fn record_from(evt: &AllternitEvent) -> RunRecord {
        let p = &evt.payload;
        RunRecord {
            owner: p["owner"].as_str().unwrap_or_default().to_string(),
            idempotency_key: p["idempotency_key"].as_str().map(str::to_string),
            run: p["run"].clone(),
            task_ir: p["task_ir"].clone(),
            attention: p["attention"].as_array().cloned().unwrap_or_default(),
        }
    }

    pub async fn load_run(&self, run_id: &str) -> anyhow::Result<Option<RunRecord>> {
        Ok(self.index().await?.runs.get(run_id).map(|(r, _)| r.clone()))
    }

    /// Latest snapshot per run for `owner`, newest first.
    pub async fn list_runs(&self, owner: &str) -> anyhow::Result<Vec<RunRecord>> {
        let g = self.index().await?;
        let mut v: Vec<&(RunRecord, u64)> = g.runs.values().filter(|(r, _)| r.owner == owner).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(v.into_iter().map(|(r, _)| r.clone()).collect())
    }

    /// Latest snapshot of every run (any owner) whose status is in `statuses`.
    pub async fn runs_with_status(&self, statuses: &[&str]) -> anyhow::Result<Vec<RunRecord>> {
        let g = self.index().await?;
        let mut v: Vec<&(RunRecord, u64)> = g.runs.values().filter(|(r, _)| statuses.iter().any(|s| r.run["status"] == *s)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(v.into_iter().map(|(r, _)| r.clone()).collect())
    }

    pub async fn find_by_idempotency(&self, owner: &str, key: &str) -> anyhow::Result<Option<RunRecord>> {
        let g = self.index().await?;
        Ok(g.idem.get(&(owner.to_string(), key.to_string())).and_then(|id| g.runs.get(id)).map(|(r, _)| r.clone()))
    }

    /// Persist a snapshot (bumps `version`, `updated_at`).
    pub async fn save(&self, mut rec: RunRecord) -> anyhow::Result<RunRecord> {
        let run_id = rec.run["id"].as_str().unwrap_or_default().to_string();
        let v = rec.run["version"].as_i64().unwrap_or(0) + 1;
        rec.run["version"] = json!(v);
        rec.run["updated_at"] = json!(now());
        let open: Vec<&Value> = rec.attention.iter().filter(|a| a["status"] == "open").collect();
        rec.run["attention"] = open.first().map(|a| (*a).clone()).unwrap_or(Value::Null);
        rec.run["open_attention_count"] = json!(open.len());
        self.append(
            EV_RUN_STATE,
            Some(&run_id),
            json!({ "run_id": run_id, "owner": rec.owner, "idempotency_key": rec.idempotency_key,
                    "run": rec.run, "task_ir": rec.task_ir, "attention": rec.attention }),
        )
        .await?;
        Ok(rec)
    }

    /// Public run events (ordered by seq).
    pub async fn events(&self, run_id: &str) -> anyhow::Result<Vec<Value>> {
        Ok(self.index().await?.events.get(run_id).cloned().unwrap_or_default())
    }

    /// Append one public event; `extra` is merged at top level (`data`, …).
    pub async fn emit(&self, run_id: &str, run_version: i64, ty: &str, extra: Value) -> anyhow::Result<Value> {
        let seq = self.index().await?.events.get(run_id).map_or(0, Vec::len) as i64 + 1;
        let mut ev = json!({ "id": format!("evt_{seq:010}"), "seq": seq, "run_id": run_id, "type": ty,
                             "created_at": now(), "run_version": run_version });
        if let (Some(o), Some(x)) = (ev.as_object_mut(), extra.as_object()) {
            for (k, v) in x {
                o.insert(k.clone(), v.clone());
            }
        }
        self.append(EV_RUN_EVENT, Some(run_id), json!({ "run_id": run_id, "event": ev })).await?;
        Ok(ev)
    }

    /// Move the run to `to`, persist, and emit `run.status_changed`.
    pub async fn transition(&self, mut rec: RunRecord, to: &str, reason: Option<&str>) -> anyhow::Result<RunRecord> {
        let from = rec.run["status"].as_str().unwrap_or("accepted").to_string();
        let terminal = TERMINAL.contains(&to);
        rec.run["status"] = json!(to);
        rec.run["status_reason"] = json!(reason);
        rec.run["terminal"] = json!(terminal);
        if terminal {
            rec.run["finished_at"] = json!(now());
        }
        let rec = self.save(rec).await?;
        let rid = rec.run["id"].as_str().unwrap_or_default().to_string();
        self.emit(&rid, rec.run["version"].as_i64().unwrap_or(0), "run.status_changed",
                  json!({ "data": { "from": from, "to": to, "terminal": terminal, "reason": reason, "run_receipt_id": null } }))
            .await?;
        Ok(rec)
    }

    pub async fn lock(&self) -> tokio::sync::MutexGuard<'static, ()> {
        WRITE_LOCK.lock().await
    }

    /// Gate for any billable/effectful step. Refuses while spend is halted
    /// (Q11), while paused/waiting on a human, and after a terminal state.
    pub async fn admit_effect(&self, run_id: &str) -> Result<(), EffectDenied> {
        let rec = self.load_run(run_id).await.ok().flatten().ok_or(EffectDenied::NotFound)?;
        if rec.run["budget_usage"]["spend_halted"] == true {
            return Err(EffectDenied::SpendHalted);
        }
        let st = rec.run["status"].as_str().unwrap_or_default();
        if matches!(st, "paused" | "needs_attention" | "cancelling") || TERMINAL.contains(&st) {
            return Err(EffectDenied::NotRunnable(st.to_string()));
        }
        Ok(())
    }

    /// Record usage. On exhaustion: halt spend first, then emit
    /// `budget.threshold`, then (on_exhaustion=request_attention) open a
    /// `budget_exhausted` attention request, else fail the run.
    pub async fn charge(&self, run_id: &str, seconds: f64, cost_usd: f64, steps: i64) -> anyhow::Result<RunRecord> {
        self.charge_usage(run_id, seconds, cost_usd, steps, 0).await
    }

    /// [`charge`] plus model tokens (counted toward the daily caps).
    pub async fn charge_usage(&self, run_id: &str, seconds: f64, cost_usd: f64, steps: i64, tokens: u64) -> anyhow::Result<RunRecord> {
        let _g = self.lock().await;
        let mut rec = self.load_run(run_id).await?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let u = &mut rec.run["budget_usage"];
        u["tokens"] = json!(u["tokens"].as_u64().unwrap_or(0) + tokens);
        u["seconds"] = json!(u["seconds"].as_f64().unwrap_or(0.0) + seconds);
        u["cost_usd"] = json!(u["cost_usd"].as_f64().unwrap_or(0.0) + cost_usd);
        u["steps"] = json!(u["steps"].as_i64().unwrap_or(0) + steps);
        // Section 6 run summary totals (model time only counts token steps).
        let ms = (seconds * 1000.0) as u64;
        let (dur, mms, tk) = (u["duration_ms"].as_u64().unwrap_or(0) + ms,
            u["model_ms"].as_u64().unwrap_or(0) + if tokens > 0 { ms } else { 0 }, u["tokens"].as_u64().unwrap_or(0));
        u["duration_ms"] = json!(dur);
        u["model_ms"] = json!(mms);
        rec.run["speed"] = json!({ "duration_ms": dur, "tokens": tk, "model_ms": mms, "wait_ms": 0,
            "tok_per_s": (tk > 0 && mms > 0).then(|| (tk as f64 * 10000.0 / mms as f64).round() / 10.0) });
        let b = rec.run["budget"].clone();
        let u = rec.run["budget_usage"].clone();
        let over = |lim: &str, used: &str| b[lim].as_f64().is_some_and(|l| u[used].as_f64().unwrap_or(0.0) >= l);
        let dim = [("max_cost_usd", "cost_usd"), ("max_seconds", "seconds"), ("max_steps", "steps")]
            .into_iter()
            .find(|(l, used)| over(l, used))
            .map(|(l, _)| l);
        let Some(dim) = dim else { return self.save(rec).await };
        if rec.run["budget_usage"]["spend_halted"] == true {
            return self.save(rec).await;
        }
        // 1) hard-stop spend, durably, before anything asks a human.
        rec.run["budget_usage"]["spend_halted"] = json!(true);
        let rec = self.save(rec).await?;
        let v = rec.run["version"].as_i64().unwrap_or(0);
        self.emit(run_id, v, "budget.threshold", json!({ "data": { "dimension": dim, "percent": 100, "spend_halted": true } })).await?;
        // 2) then ask (or stop).
        if rec.run["budget"]["on_exhaustion"] == "stop" {
            return self.transition(rec, "failed", Some("budget exhausted")).await;
        }
        let mut rec = rec;
        let att = json!({
            "id": new_id("att"), "object": "attention_request", "run_id": run_id, "status": "open",
            "reason": "budget_exhausted", "title": format!("Budget exhausted ({dim}). Raise the budget or stop the run."),
            "created_at": now(), "resolution": null
        });
        rec.attention.push(att.clone());
        self.emit(run_id, v, "attention.requested", json!({ "data": { "attention": att } })).await?;
        self.transition(rec, "needs_attention", Some("budget_exhausted")).await
    }

    /// Today's spend (UTC, by run creation day) globally and for `org`.
    pub async fn daily_spend(&self, org: &str) -> anyhow::Result<(super::guard::Spend, super::guard::Spend)> {
        let g = self.index().await?;
        Ok(super::guard::spend_today(g.runs.values().map(|(r, _)| (&r.run, &r.task_ir)), org))
    }

    /// A daily spending cap (global or org) is reached: halt spend durably
    /// first, then open the "budget cap reached" attention request and park
    /// the run in `needs_attention` (Q11 ordering, same as `charge`).
    pub async fn park_for_cap(&self, run_id: &str, scope: &str, dimension: &str) -> anyhow::Result<RunRecord> {
        let _g = self.lock().await;
        let mut rec = self.load_run(run_id).await?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let st = rec.run["status"].as_str().unwrap_or_default().to_string();
        if TERMINAL.contains(&st.as_str()) || st == "needs_attention" {
            return Ok(rec);
        }
        rec.run["budget_usage"]["spend_halted"] = json!(true);
        let rec = self.save(rec).await?;
        let v = rec.run["version"].as_i64().unwrap_or(0);
        self.emit(run_id, v, "budget.threshold", json!({ "data": { "dimension": format!("daily_{scope}_{dimension}"), "percent": 100, "spend_halted": true } })).await?;
        let mut rec = rec;
        let att = json!({
            "id": new_id("att"), "object": "attention_request", "run_id": run_id, "status": "open",
            "reason": super::guard::CAP_REASON, "title": super::guard::CAP_TITLE,
            "detail": format!("The {scope} daily {dimension} cap is reached. Spend stopped; approve after the cap resets or is raised to continue."),
            "created_at": now(), "resolution": null
        });
        rec.attention.push(att.clone());
        self.emit(run_id, v, "attention.requested", json!({ "data": { "attention": att } })).await?;
        self.transition(rec, "needs_attention", Some(super::guard::CAP_TITLE)).await
    }

    /// Agent rules `approvals.spend_over_usd`: raise attention once per run
    /// when its spend crosses the threshold, then park. Returns whether the
    /// run was parked (false = already raised or not applicable).
    pub async fn park_for_spend(&self, run_id: &str, threshold: f64) -> anyhow::Result<bool> {
        let _g = self.lock().await;
        let mut rec = self.load_run(run_id).await?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let st = rec.run["status"].as_str().unwrap_or_default().to_string();
        if TERMINAL.contains(&st.as_str()) || st == "needs_attention" || rec.attention.iter().any(|a| a["reason"] == super::guard::SPEND_REASON) {
            return Ok(false);
        }
        let spent = rec.run["budget_usage"]["cost_usd"].as_f64().unwrap_or(0.0);
        if spent < threshold {
            return Ok(false);
        }
        let v = rec.run["version"].as_i64().unwrap_or(0);
        let att = json!({
            "id": new_id("att"), "object": "attention_request", "run_id": run_id, "status": "open",
            "reason": super::guard::SPEND_REASON, "title": super::guard::SPEND_TITLE,
            "detail": format!("This run has spent ${spent:.2}, past the ${threshold:.2} approval threshold. Approve to continue or reject to stop."),
            "created_at": now(), "resolution": null
        });
        rec.attention.push(att.clone());
        self.emit(run_id, v, "attention.requested", json!({ "data": { "attention": att } })).await?;
        self.transition(rec, "needs_attention", Some(super::guard::SPEND_TITLE)).await?;
        Ok(true)
    }

    /// Fail closed with attention: open an attention request and park the run
    /// in `needs_attention` (routing policy cannot be satisfied, a template
    /// attention step). `extra` fields are merged into the request.
    pub async fn park_attention(&self, run_id: &str, reason: &str, title: &str, detail: &str, extra: Value) -> anyhow::Result<RunRecord> {
        let _g = self.lock().await;
        let mut rec = self.load_run(run_id).await?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
        if TERMINAL.contains(&rec.run["status"].as_str().unwrap_or_default()) {
            return Ok(rec);
        }
        let v = rec.run["version"].as_i64().unwrap_or(0);
        let mut att = json!({ "id": new_id("att"), "object": "attention_request", "run_id": run_id, "status": "open",
            "reason": reason, "title": title, "detail": detail, "created_at": now(), "resolution": null });
        if let (Some(a), Some(e)) = (att.as_object_mut(), extra.as_object()) {
            a.extend(e.clone());
        }
        rec.attention.push(att.clone());
        self.emit(run_id, v, "attention.requested", json!({ "data": { "attention": att } })).await?;
        self.transition(rec, "needs_attention", Some(title)).await
    }

    /// WP-P1: halt spend durably FIRST (Q11 ordering), then open an attention
    /// request and park the run in `needs_attention` (per-run caps, stuck
    /// detection, an effect whose outcome is unknown).
    pub async fn park_halted(&self, run_id: &str, reason: &str, title: &str, detail: &str, extra: Value) -> anyhow::Result<RunRecord> {
        let _g = self.lock().await;
        let mut rec = self.load_run(run_id).await?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let st = rec.run["status"].as_str().unwrap_or_default().to_string();
        if TERMINAL.contains(&st.as_str()) || st == "needs_attention" {
            return Ok(rec);
        }
        rec.run["budget_usage"]["spend_halted"] = json!(true);
        let mut rec = self.save(rec).await?;
        let v = rec.run["version"].as_i64().unwrap_or(0);
        let mut att = json!({ "id": new_id("att"), "object": "attention_request", "run_id": run_id, "status": "open",
            "reason": reason, "title": title, "detail": detail, "created_at": now(), "resolution": null });
        if let (Some(a), Some(e)) = (att.as_object_mut(), extra.as_object()) {
            a.extend(e.clone());
        }
        rec.attention.push(att.clone());
        self.emit(run_id, v, "attention.requested", json!({ "data": { "attention": att } })).await?;
        self.transition(rec, "needs_attention", Some(title)).await
    }

    /// Latest snapshot of every run billed to `org`, newest first.
    pub async fn runs_of_org(&self, org: &str) -> anyhow::Result<Vec<RunRecord>> {
        let g = self.index().await?;
        let mut v: Vec<&(RunRecord, u64)> = g.runs.values().filter(|(r, _)| super::guard::run_org(&r.task_ir) == org).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(v.into_iter().map(|(r, _)| r.clone()).collect())
    }

    /// Runs of `org` (other than `except`) that started within the last hour.
    pub async fn org_runs_started_last_hour(&self, org: &str, except: &str) -> anyhow::Result<usize> {
        let g = self.index().await?;
        Ok(super::safety::runs_started_last_hour(g.runs.values().map(|(r, _)| (&r.run, &r.task_ir)), org, except))
    }

    // ── campaigns / replays: snapshot records keyed by id ────────────────────

    pub async fn save_object(&self, ty: &str, id: &str, owner: &str, obj: &Value) -> anyhow::Result<()> {
        self.append(ty, None, json!({ "id": id, "owner": owner, "object": obj })).await
    }

    pub async fn load_object(&self, ty: &str, id: &str, owner: &str) -> anyhow::Result<Option<Value>> {
        Ok(self.index().await?.objects.get(&(ty.to_string(), id.to_string(), owner.to_string())).map(|(o, _)| o.clone()))
    }

    pub async fn list_objects(&self, ty: &str, owner: &str) -> anyhow::Result<Vec<Value>> {
        let g = self.index().await?;
        let mut v: Vec<(&(String, String, String), &(Value, u64))> = g.objects.iter().filter(|(k, _)| k.0 == ty && k.2 == owner).collect();
        v.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
        Ok(v.into_iter().map(|(_, (o, _))| o.clone()).collect())
    }
}
