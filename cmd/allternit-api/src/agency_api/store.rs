//! Durable Agency run/campaign/replay store on the Factory ledger.
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
pub const EV_REPLAY_BATCH_STATE: &str = "agency.replay_batch.state";
/// C2: a run's trace-monitor findings (keyed by run id).
pub const EV_FINDINGS_STATE: &str = "agency.run_findings.state";
/// C5: eval suites and eval runs.
pub const EV_EVAL_SUITE_STATE: &str = "agency.eval_suite.state";
pub const EV_EVAL_RUN_STATE: &str = "agency.eval_run.state";
/// C6: harness learning. Searches, proposals, the per-task-type graph
/// version registry and the audit trail (all owned by [`SYSTEM_OWNER`]).
pub const EV_HARNESS_SEARCH_STATE: &str = "agency.harness_search.state";
pub const EV_HARNESS_PROPOSAL_STATE: &str = "agency.harness_proposal.state";
pub const EV_HARNESS_VERSIONS_STATE: &str = "agency.harness_versions.state";
pub const EV_HARNESS_AUDIT_STATE: &str = "agency.harness_audit.state";
/// Owner of server-wide objects (harness versions apply to every user's runs).
pub const SYSTEM_OWNER: &str = "system";

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
            EV_CAMPAIGN_STATE | EV_REPLAY_STATE | EV_REPLAY_BATCH_STATE | EV_FINDINGS_STATE | EV_EVAL_SUITE_STATE | EV_EVAL_RUN_STATE
            | EV_HARNESS_SEARCH_STATE | EV_HARNESS_PROPOSAL_STATE | EV_HARNESS_VERSIONS_STATE | EV_HARNESS_AUDIT_STATE => {
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
    /// C2: called with the run id after every terminal transition is durable.
    on_terminal: std::sync::OnceLock<TerminalHook>,
}

/// Called after a run's terminal `run.status_changed` is durable (the trace
/// monitor). It must not block: it spawns its work.
pub type TerminalHook = Arc<dyn Fn(&str) + Send + Sync>;

/// One index per ledger: `store(st)` builds an `AgencyStore` per request, so
/// the index must outlive the handle.
static INDEXES: std::sync::Mutex<Option<HashMap<usize, Arc<Shared>>>> = std::sync::Mutex::new(None);

fn shared_for(ledger: &Arc<Ledger>) -> Arc<Shared> {
    let mut g = INDEXES.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .entry(Arc::as_ptr(ledger) as usize)
        .or_insert_with(|| Arc::new(Shared { _ledger: ledger.clone(), inner: RwLock::new(Inner::default()), scans: AtomicUsize::new(0), on_terminal: Default::default() }))
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

    /// Install the terminal hook once per ledger (later calls are no-ops).
    pub fn set_terminal_hook(&self, f: impl FnOnce() -> TerminalHook) {
        self.shared.on_terminal.get_or_init(f);
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
        if terminal {
            if let Some(h) = self.shared.on_terminal.get() {
                h(&rid);
            }
        }
        Ok(rec)
    }

    /// Attention `expires_at` (attention.mdx, "How it works" step 5): every
    /// open request whose deadline passed becomes `expired`. An `approval`
    /// (anything not `kind: informational`) resolves as a rejection (fail
    /// closed, `outcome: rejected`) and the run moves exactly as it would on a
    /// rejection answer ([`rejection_next`], the same mapping as
    /// `mod.rs resolve_attention_linked`); an `informational` one gets
    /// `outcome: none` and the run is left as it is. Requests without
    /// `expires_at` never expire. Run every 60 s by the executor's resume task;
    /// returns the runs to re-queue.
    pub async fn expire_attention(&self) -> anyhow::Result<Vec<String>> {
        let at = Utc::now();
        let mut requeue = Vec::new();
        let _g = self.lock().await;
        for listed in self.runs_with_status(&["needs_attention"]).await? {
            if !listed.attention.iter().any(|a| attention_expired(a, at)) {
                continue;
            }
            let id = listed.run["id"].as_str().unwrap_or_default().to_string();
            let Some(mut rec) = self.load_run(&id).await? else { continue };
            let due: Vec<Value> = rec.attention.iter().filter(|a| attention_expired(a, at)).cloned().collect();
            if due.is_empty() {
                continue;
            }
            let v = rec.run["version"].as_i64().unwrap_or(0);
            let mut next = None;
            let mut released = Vec::new();
            for a in &due {
                let rejected = a["kind"] != "informational";
                let resolution = json!({ "type": "expiry", "resolved_by": "system", "resolved_at": now(), "receipt_id": new_id("rcpt"),
                    "outcome": if rejected { "rejected" } else { "none" }, "requested_by": rec.owner, "self_approval": false });
                for x in rec.attention.iter_mut().filter(|x| x["id"] == a["id"]) {
                    x["status"] = json!("expired");
                    x["resolution"] = resolution.clone();
                }
                self.emit(&id, v, "attention.expired", json!({ "data": { "attention_id": a["id"], "resolution": resolution } })).await?;
                released.push((rec.owner.clone(), a["id"].as_str().unwrap_or_default().to_string(), resolution));
                if rejected && next.is_none() {
                    next = rejection_next(a["reason"].as_str().unwrap_or_default());
                }
            }
            match next {
                Some((to, why, again)) => {
                    self.transition(rec, to, Some(why)).await?;
                    if again {
                        requeue.push(id);
                    }
                }
                None => {
                    self.save(rec).await?;
                }
            }
            // A parent run parked on one of these (a child template's request)
            // gets the same outcome, as when a person answers on the child.
            for (owner, child_att, resolution) in released {
                for pid in self.release_parents(&owner, &child_att, &resolution, "expired").await? {
                    if !requeue.contains(&pid) {
                        requeue.push(pid);
                    }
                }
            }
        }
        Ok(requeue)
    }

    /// G2: a child template's request was settled on the child itself
    /// (answered, or expired unanswered): settle every open parent item linked
    /// to it with the same resolution (`status` `resolved` or `expired`) and
    /// move each parent with nothing else open back to `waiting` (its drive
    /// re-checks the child under the fence). Returns the parent runs to
    /// re-queue. The caller holds the write lock or runs under the answer path.
    pub async fn release_parents(&self, owner: &str, child_att: &str, resolution: &Value, status: &str) -> anyhow::Result<Vec<String>> {
        let event = if status == "expired" { "attention.expired" } else { "attention.resolved" };
        let mut requeue = Vec::new();
        for mut p in self.list_runs(owner).await? {
            if p.run["status"] != "needs_attention" {
                continue;
            }
            let mut hit = None;
            for a in p.attention.iter_mut().filter(|a| a["reason"] == super::effect_template::CHILD_ATTENTION_REASON && a["status"] == "open" && a["child_attention_id"] == child_att) {
                a["status"] = json!(status);
                a["resolution"] = resolution.clone();
                hit = a["id"].as_str().map(str::to_string);
            }
            let Some(pid) = hit else { continue };
            let run_id = p.run["id"].as_str().unwrap_or_default().to_string();
            let v = p.run["version"].as_i64().unwrap_or(0);
            self.emit(&run_id, v, event, json!({ "data": { "attention_id": pid, "resolution": resolution } })).await?;
            if p.attention.iter().any(|a| a["status"] == "open") {
                self.save(p).await?;
                continue;
            }
            self.transition(p, "waiting", Some("answered; queued to re-check the template run (committed steps replay from the journal)")).await?;
            requeue.push(run_id);
        }
        Ok(requeue)
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
        let att = new_attention(run_id, json!({
            "reason": "budget_exhausted", "title": format!("Budget exhausted ({dim}). Raise the budget or stop the run.")
        }));
        rec.attention.push(att.clone());
        self.emit(run_id, v, "attention.requested", json!({ "data": { "attention": att } })).await?;
        self.transition(rec, "needs_attention", Some("budget_exhausted")).await
    }

    /// Today's spend (UTC, by run creation day) globally and for `org`.
    pub async fn daily_spend(&self, org: &str) -> anyhow::Result<(super::guard::Spend, super::guard::Spend)> {
        let g = self.index().await?;
        Ok(super::guard::spend_today(g.runs.values().map(|(r, _)| (&r.run, &r.task_ir)), org))
    }

    /// [`daily_spend`] plus today's replay and eval live model calls (usage
    /// ledger, `source = replay`): what the daily caps count.
    pub async fn daily_spend_all(&self, db: &crate::db::DbHandle, org: &str) -> anyhow::Result<(super::guard::Spend, super::guard::Spend)> {
        let (g, o) = self.daily_spend(org).await?;
        let (rg, ro) = super::guard::replay_spend_today(&db.connect()?, org)?;
        Ok((super::guard::add(g, rg), super::guard::add(o, ro)))
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
        let att = new_attention(run_id, json!({
            "reason": super::guard::CAP_REASON, "title": super::guard::CAP_TITLE,
            "detail": format!("The {scope} daily {dimension} cap is reached. Spend stopped; approve after the cap resets or is raised to continue.")
        }));
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
        let att = new_attention(run_id, json!({
            "reason": super::guard::SPEND_REASON, "title": super::guard::SPEND_TITLE,
            "detail": format!("This run has spent ${spent:.2}, past the ${threshold:.2} approval threshold. Approve to continue or reject to stop.")
        }));
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
        let mut fields = json!({ "reason": reason, "title": title, "detail": detail });
        if let (Some(a), Some(e)) = (fields.as_object_mut(), extra.as_object()) {
            a.extend(e.clone());
        }
        let att = new_attention(run_id, fields);
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
        let mut fields = json!({ "reason": reason, "title": title, "detail": detail });
        if let (Some(a), Some(e)) = (fields.as_object_mut(), extra.as_object()) {
            a.extend(e.clone());
        }
        let att = new_attention(run_id, fields);
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

/// An open attention request past its `expires_at` (RFC 3339). No deadline,
/// an unparsable one, or a request no longer open never expires.
pub fn attention_expired(a: &Value, at: chrono::DateTime<Utc>) -> bool {
    a["status"] == "open"
        && a["expires_at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| t.with_timezone(&Utc) <= at)
}

/// Hours a person has to answer an attention request before it expires
/// (`approval`: as a rejection, fail closed). Most requests park a run that
/// holds a budget, a lock or a person's goal; without a deadline an unanswered
/// one parked the run forever and the expiry sweep never acted.
pub const DEFAULT_ATTENTION_TTL_HOURS: i64 = 72;

/// `ALLTERNIT_AGENCY_ATTENTION_TTL_HOURS` (0 = no default deadline), else
/// [`DEFAULT_ATTENTION_TTL_HOURS`].
pub fn attention_ttl_hours() -> i64 {
    std::env::var("ALLTERNIT_AGENCY_ATTENTION_TTL_HOURS").ok().and_then(|v| v.trim().parse::<i64>().ok()).filter(|h| *h >= 0).unwrap_or(DEFAULT_ATTENTION_TTL_HOURS)
}

/// A new open attention request on `run_id`: `fields` (reason, title,
/// detail, step fields) over the defaults. `kind` defaults to `approval`;
/// `on_expiry` follows `kind` (attention.mdx). `expires_at` defaults to
/// `at` + `ttl_hours` (none when 0); an `expires_at` in `fields` wins, `null`
/// included (a wake gate, or a parent copying its child's deadline).
pub fn open_attention(run_id: &str, fields: Value, at: chrono::DateTime<Utc>, ttl_hours: i64) -> Value {
    let deadline = (ttl_hours > 0).then(|| (at + chrono::Duration::hours(ttl_hours)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let mut att = json!({ "id": new_id("att"), "object": "attention_request", "run_id": run_id, "status": "open",
        "kind": "approval", "expires_at": deadline, "created_at": now(), "resolution": null });
    if let (Some(a), Some(f)) = (att.as_object_mut(), fields.as_object()) {
        a.extend(f.clone());
    }
    let on_expiry = if att["kind"] == "informational" { "none" } else { "reject" };
    att["on_expiry"] = json!(on_expiry);
    att
}

/// [`open_attention`] now, with the configured deadline.
pub fn new_attention(run_id: &str, fields: Value) -> Value {
    open_attention(run_id, fields, Utc::now(), attention_ttl_hours())
}

/// Where a run goes when an attention request of `reason` is rejected (by a
/// person or by expiry): `(status, why, re-queue)`, or `None` to leave it.
/// Mirrors the rejection arms of `mod.rs resolve_attention_linked`.
pub fn rejection_next(reason: &str) -> Option<(&'static str, &'static str, bool)> {
    match reason {
        "budget_exhausted" => Some(("failed", "budget exhausted; the request expired unanswered")),
        r if r == super::guard::SPEND_REASON || r == super::guard::CAP_REASON => Some(("failed", "attention expired unanswered; the run stopped (fail closed)")),
        r if matches!(r, super::safety::RUN_CAP_REASON | super::safety::STUCK_REASON | super::safety::UNKNOWN_EFFECT_REASON) => {
            Some(("failed", "attention expired unanswered; the run stopped (fail closed)"))
        }
        _ => None,
    }
    .map(|(to, why)| (to, why, false))
    .or_else(|| {
        (reason == super::executor::generic::TASK_WAIT_REASON || reason == super::effect_template::CHILD_ATTENTION_REASON)
            .then_some(("waiting", "attention expired unanswered (rejected); queued for execution", true))
    })
}

#[cfg(test)]
mod expiry_tests {
    use super::*;

    #[test]
    fn a_new_request_expires_by_default_unless_its_step_says_otherwise() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc);
        let a = open_attention("run_1", json!({ "reason": "spend_threshold", "title": "t" }), at, 72);
        assert_eq!(a["expires_at"], "2026-11-16T12:00:00Z");
        assert_eq!((a["kind"].as_str(), a["on_expiry"].as_str(), a["status"].as_str()), (Some("approval"), Some("reject"), Some("open")));
        assert!(!attention_expired(&a, at) && attention_expired(&a, at + chrono::Duration::hours(72)), "the sweep acts on the default deadline");
        // A template's own deadline, or an explicit null (wake gate), wins.
        let t = open_attention("run_1", json!({ "expires_at": "2026-11-13T13:00:00Z" }), at, 72);
        assert_eq!(t["expires_at"], "2026-11-13T13:00:00Z");
        assert!(open_attention("run_1", json!({ "expires_at": null }), at, 72)["expires_at"].is_null());
        assert!(open_attention("run_1", json!({}), at, 0)["expires_at"].is_null(), "TTL 0 = no default deadline");
        assert_eq!(open_attention("run_1", json!({ "kind": "informational" }), at, 72)["on_expiry"], "none");
    }

    #[test]
    fn only_open_requests_past_their_deadline_expire() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z").unwrap().with_timezone(&Utc);
        assert!(attention_expired(&json!({ "status": "open", "expires_at": "2026-11-12T11:59:59Z" }), at));
        assert!(attention_expired(&json!({ "status": "open", "expires_at": "2026-11-12T12:00:00Z" }), at));
        assert!(!attention_expired(&json!({ "status": "open", "expires_at": "2026-11-12T12:00:01Z" }), at));
        assert!(!attention_expired(&json!({ "status": "open", "expires_at": null }), at));
        assert!(!attention_expired(&json!({ "status": "open" }), at));
        assert!(!attention_expired(&json!({ "status": "open", "expires_at": "tomorrow" }), at));
        assert!(!attention_expired(&json!({ "status": "resolved", "expires_at": "2026-11-12T11:00:00Z" }), at));
    }

    #[test]
    fn an_expired_approval_moves_the_run_like_a_rejection() {
        assert_eq!(rejection_next("budget_exhausted").map(|n| (n.0, n.2)), Some(("failed", false)));
        assert_eq!(rejection_next(super::super::guard::SPEND_REASON).map(|n| n.0), Some("failed"));
        assert_eq!(rejection_next(super::super::guard::CAP_REASON).map(|n| n.0), Some("failed"));
        assert_eq!(rejection_next(super::super::safety::STUCK_REASON).map(|n| n.0), Some("failed"));
        assert_eq!(rejection_next(super::super::executor::generic::TASK_WAIT_REASON).map(|n| (n.0, n.2)), Some(("waiting", true)));
        assert_eq!(rejection_next(super::super::effect_template::CHILD_ATTENTION_REASON).map(|n| (n.0, n.2)), Some(("waiting", true)));
        assert_eq!(rejection_next("routing_policy"), None);
    }
}
