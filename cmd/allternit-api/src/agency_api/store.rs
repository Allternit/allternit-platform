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

use allternit_commrails::core::types::{Actor, ActorType, AllternitEvent, EventScope, LedgerQuery};
use allternit_commrails::ledger::Ledger;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

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

#[derive(Clone)]
pub struct AgencyStore {
    ledger: Arc<Ledger>,
}

fn scope(run_id: &str) -> EventScope {
    EventScope { run_id: Some(run_id.to_string()), ..Default::default() }
}

impl AgencyStore {
    pub fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    async fn append(&self, ty: &str, run_id: Option<&str>, payload: Value) -> anyhow::Result<()> {
        self.ledger
            .append(AllternitEvent {
                event_id: String::new(),
                ts: String::new(),
                actor: Actor { r#type: ActorType::Gate, id: "agency-api".into() },
                scope: run_id.map(scope),
                r#type: ty.into(),
                payload,
                provenance: None,
            })
            .await?;
        Ok(())
    }

    async fn of_type(&self, ty: &str) -> anyhow::Result<Vec<AllternitEvent>> {
        self.ledger.query(LedgerQuery { r#type: Some(ty.into()), ..Default::default() }).await
    }

    /// Append a raw ledger event (e.g. `JudgePolicySet` for the run's DAG).
    pub async fn append_raw(&self, ty: &str, run_id: &str, payload: Value) -> anyhow::Result<()> {
        self.append(ty, Some(run_id), payload).await
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
        Ok(self.of_type(EV_RUN_STATE).await?.iter().rev().find(|e| e.payload["run_id"] == run_id).map(Self::record_from))
    }

    /// Latest snapshot per run for `owner`, newest first.
    pub async fn list_runs(&self, owner: &str) -> anyhow::Result<Vec<RunRecord>> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in self.of_type(EV_RUN_STATE).await?.iter().rev() {
            let id = e.payload["run_id"].as_str().unwrap_or_default().to_string();
            if e.payload["owner"] == owner && seen.insert(id) {
                out.push(Self::record_from(e));
            }
        }
        Ok(out)
    }

    pub async fn find_by_idempotency(&self, owner: &str, key: &str) -> anyhow::Result<Option<RunRecord>> {
        let Some(e) = self.of_type(EV_RUN_STATE).await?.into_iter().find(|e| e.payload["owner"] == owner && e.payload["idempotency_key"] == key) else {
            return Ok(None);
        };
        let id = e.payload["run_id"].as_str().unwrap_or_default().to_string();
        self.load_run(&id).await
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
        Ok(self
            .ledger
            .query(LedgerQuery { r#type: Some(EV_RUN_EVENT.into()), scope: Some(scope(run_id)), ..Default::default() })
            .await?
            .into_iter()
            .filter(|e| e.payload["run_id"] == run_id)
            .map(|e| e.payload["event"].clone())
            .collect())
    }

    /// Append one public event; `extra` is merged at top level (`data`, …).
    pub async fn emit(&self, run_id: &str, run_version: i64, ty: &str, extra: Value) -> anyhow::Result<Value> {
        let seq = self.events(run_id).await?.len() as i64 + 1;
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
        let _g = self.lock().await;
        let mut rec = self.load_run(run_id).await?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let u = &mut rec.run["budget_usage"];
        u["seconds"] = json!(u["seconds"].as_f64().unwrap_or(0.0) + seconds);
        u["cost_usd"] = json!(u["cost_usd"].as_f64().unwrap_or(0.0) + cost_usd);
        u["steps"] = json!(u["steps"].as_i64().unwrap_or(0) + steps);
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

    // ── campaigns / replays: snapshot records keyed by id ────────────────────

    pub async fn save_object(&self, ty: &str, id: &str, owner: &str, obj: &Value) -> anyhow::Result<()> {
        self.append(ty, None, json!({ "id": id, "owner": owner, "object": obj })).await
    }

    pub async fn load_object(&self, ty: &str, id: &str, owner: &str) -> anyhow::Result<Option<Value>> {
        Ok(self.of_type(ty).await?.iter().rev().find(|e| e.payload["id"] == id && e.payload["owner"] == owner).map(|e| e.payload["object"].clone()))
    }

    pub async fn list_objects(&self, ty: &str, owner: &str) -> anyhow::Result<Vec<Value>> {
        let mut seen = std::collections::HashSet::new();
        Ok(self
            .of_type(ty)
            .await?
            .iter()
            .rev()
            .filter(|e| e.payload["owner"] == owner && seen.insert(e.payload["id"].as_str().unwrap_or_default().to_string()))
            .map(|e| e.payload["object"].clone())
            .collect())
    }
}
