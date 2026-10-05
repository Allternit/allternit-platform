//! WP-C3b: the `campaign:` effect connector (CAMPAIGN).
//!
//! Advances one cycle of a campaign created through `/v1/campaigns`: the
//! cycle is recorded on the campaign's own record (same store, same owner
//! scoping as the routes), the run joins `run_ids`, and the campaign's step
//! budget is charged.
//!
//! Q19: a cycle only runs on an explicit trigger. The executor passes
//! `triggered = true` only when the run's wake gate was opened by a person;
//! without it this refuses. Advancing a cycle never schedules the next one:
//! `scheduling` stays `disabled_pending_golive` and `next_wake` stays null.
//!
//! Replay-safe on its own as well as through P1's fenced path: a cycle
//! already recorded for this effect key is served, not applied twice.

use crate::agency_api::store::{AgencyStore, EV_CAMPAIGN_STATE};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

/// Prompt suffix for the node that proposes the campaign step (K03).
pub const STEP_HINT: &str = "\n\nDescribe the single next step of this campaign cycle in one short paragraph.";

/// Record one cycle of `campaign_id` for `run_id`. Returns the effect ref.
pub async fn advance(s: &AgencyStore, owner: &str, campaign_id: &str, run_id: &str, key: &str, step: &str, triggered: bool) -> Result<String> {
    if !triggered {
        bail!("campaign cycles only run on an explicit trigger (Q19): not allowed without one");
    }
    let mut c = s.load_object(EV_CAMPAIGN_STATE, campaign_id, owner).await?
        .ok_or_else(|| anyhow!("campaign {campaign_id} not found"))?;
    let digest = allternit_factory_engine::receipts::jcs::sha256_tagged(step.as_bytes());
    let cycles = c["cycles"].as_array().cloned().unwrap_or_default();
    if let Some(prev) = cycles.iter().find(|x| x["effect_key"] == key) {
        return Ok(format!("campaign:{campaign_id}:cycle:{}:{}", prev["cycle"], prev["step_digest"].as_str().unwrap_or_default()));
    }
    match c["status"].as_str() {
        Some("active") => {}
        other => bail!("campaign {campaign_id} is {} (not allowed to advance)", other.unwrap_or("unknown")),
    }
    let used = c["budget_usage"]["steps"].as_u64().unwrap_or(0);
    if c["budget_usage"]["spend_halted"] == true || c["budget"]["max_steps"].as_u64().is_some_and(|m| used >= m) {
        bail!("campaign {campaign_id} budget is exhausted (not allowed to advance)");
    }
    let n = cycles.len() as u64 + 1;
    let ts = crate::agency_api::store::now();
    let mut cycles = cycles;
    cycles.push(json!({ "cycle": n, "run_id": run_id, "effect_key": key, "step_digest": digest, "trigger": "explicit", "at": ts }));
    c["cycles"] = json!(cycles);
    c["budget_usage"]["steps"] = json!(used + 1);
    let mut ids = c["run_ids"].as_array().cloned().unwrap_or_default();
    if !ids.iter().any(|x| x == run_id) { ids.push(json!(run_id)); }
    c["run_ids"] = json!(ids);
    c["last_cycle_at"] = json!(ts);
    c["updated_at"] = json!(ts);
    // Never self-wakes (Q19).
    c["scheduling"] = json!("disabled_pending_golive");
    c["next_wake"] = Value::Null;
    s.save_object(EV_CAMPAIGN_STATE, campaign_id, owner, &c).await?;
    Ok(format!("campaign:{campaign_id}:cycle:{n}:{digest}"))
}
