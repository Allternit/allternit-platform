//! Agency production safety (WP-P1; W6 of the 2026-10-01 plan, research memo A).
//!
//! The executor (`executor.rs`) and the template executor (`template_exec.rs`)
//! call into this module; WP-X1 / WP-B1 build on the same seams. Everything
//! durable lives in the V210 SQLite tables (see the migration).
//!
//! 1. **Two-phase fence.** Each drive of a run takes a fencing token
//!    ([`acquire_fence`]: the run's epoch + 1). Every side effect is
//!    [`prepare`]d (journal row claimed under the current epoch), executed,
//!    then [`commit`]ted with a compare-and-set on the epoch. A stale or
//!    duplicate worker (older epoch) is refused at prepare, at commit and at
//!    every admission check, and stops without touching the run.
//! 2. **Idempotency / replay.** Effect keys are stable across re-drives
//!    (`<run>:<node>:<tool>:<n>`, n = occurrence of that node+tool in the
//!    drive). A committed key is served from the journal and never re-applied;
//!    the same key with different arguments is a replay divergence (fail
//!    closed). Model outputs that steer later effects are journaled too
//!    ([`journal_value`] / [`journaled_value`]), so a resumed run replays the
//!    same proposals instead of re-calling the model (memo A pitfall:
//!    "re-running LLM calls on replay"). A row left `prepared` by a dead
//!    worker is retried only when the effect is retry-safe; otherwise the run
//!    parks with `effect_outcome_unknown`. Failed rows carry an error class
//!    (`retryable` / `non_retryable`, Temporal-style); non-retryable failures
//!    are not retried on a re-drive.
//! 3. **Caps + stuck detection.** [`RunCaps`]: steps, wall time (attention
//!    wait excluded) and spend per run, server defaults so nothing is
//!    unbounded, tightened by org policy. [`StuckDetector`]: OpenHands-style
//!    repetition heuristics (same action+observation ×4, same action error ×3,
//!    A/B ping-pong ≥6 cycles, N steps with no new observation). Either one
//!    halts spend first, then parks the run with an attention item
//!    (`run_cap_reached` / `run_stuck`); a rejection ends the run.
//! 4. **Prod lanes.** [`Limits`](super::guard::Limits) gains a per-org run
//!    rate (`ALLTERNIT_AGENCY_ORG_RUNS_PER_HOUR`) next to the existing per-org
//!    concurrency and daily caps. Pool entries are classed into lanes
//!    (`api` / `subscription` / `local`); with `ALLTERNIT_AGENCY_LANES=api_first`
//!    (default) subscription lanes are held back as a fallback, `api_only`
//!    drops them. A backend that fails is cooled down process-wide for
//!    `ALLTERNIT_AGENCY_LANE_COOLDOWN_SECS`. The lane is logged on every plan
//!    record.
//! 5. **Non-requester approval.** [`requires_non_requester`]: when the org's
//!    policy says so, an *approval* of a consequential attention item must
//!    come from an org member other than the run's requester (rejections are
//!    always allowed). Default: on for orgs with 2+ members, off for
//!    single-member orgs and personal (`user:`) orgs.
//!
//! `ALLTERNIT_AGENCY_EXECUTE` gating is unchanged: none of this runs unless the
//! executor is on.

use crate::db::DbHandle;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const RUN_MAX_STEPS_ENV: &str = "ALLTERNIT_AGENCY_RUN_MAX_STEPS";
pub const RUN_MAX_WALL_ENV: &str = "ALLTERNIT_AGENCY_RUN_MAX_WALL_SECS";
pub const RUN_MAX_USD_ENV: &str = "ALLTERNIT_AGENCY_RUN_MAX_USD";
pub const NO_PROGRESS_ENV: &str = "ALLTERNIT_AGENCY_NO_PROGRESS_STEPS";
pub const LANES_ENV: &str = "ALLTERNIT_AGENCY_LANES";
pub const SUBSCRIPTION_SOURCES_ENV: &str = "ALLTERNIT_AGENCY_SUBSCRIPTION_SOURCES";
pub const LANE_COOLDOWN_ENV: &str = "ALLTERNIT_AGENCY_LANE_COOLDOWN_SECS";

pub const DEFAULT_RUN_MAX_STEPS: u64 = 200;
pub const DEFAULT_RUN_MAX_WALL_SECS: u64 = 3600;
pub const DEFAULT_RUN_MAX_USD: f64 = 10.0;
pub const DEFAULT_NO_PROGRESS_STEPS: u32 = 40;
pub const DEFAULT_LANE_COOLDOWN_SECS: u64 = 300;

pub const RUN_CAP_REASON: &str = "run_cap_reached";
pub const STUCK_REASON: &str = "run_stuck";
pub const UNKNOWN_EFFECT_REASON: &str = "effect_outcome_unknown";

/// Attention reasons whose *approval* is consequential (spend continues or
/// a limit is lifted). An attention item can also opt in with
/// `"consequential": true`.
pub const CONSEQUENTIAL_REASONS: &[&str] = &[
    "budget_exhausted", super::guard::SPEND_REASON, RUN_CAP_REASON, STUCK_REASON, UNKNOWN_EFFECT_REASON,
];

fn now() -> String {
    super::store::now()
}

fn env_num<T: std::str::FromStr>(get: &dyn Fn(&str) -> Option<String>, k: &str) -> Option<Option<T>> {
    let v = get(k)?;
    let v = v.trim();
    if v.eq_ignore_ascii_case("off") {
        return Some(None);
    }
    v.parse::<T>().ok().map(Some)
}

fn conn(db: &DbHandle) -> anyhow::Result<rusqlite::Connection> {
    let c = db.connect()?;
    c.busy_timeout(Duration::from_secs(5))?;
    Ok(c)
}

// ── 1/2. fence + effect journal ─────────────────────────────────────────────

/// Take the fencing token for a new drive of `run_id`: epoch + 1. Every
/// earlier holder is stale from this point on.
pub fn acquire_fence(db: &DbHandle, run_id: &str, holder: &str) -> anyhow::Result<i64> {
    let mut c = conn(db)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cur: Option<i64> = tx.query_row("SELECT epoch FROM agency_run_fences WHERE run_id = ?1", params![run_id], |r| r.get(0)).optional()?;
    let epoch = cur.unwrap_or(0) + 1;
    tx.execute(
        "INSERT INTO agency_run_fences (run_id, epoch, holder, acquired_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(run_id) DO UPDATE SET epoch = excluded.epoch, holder = excluded.holder, acquired_at = excluded.acquired_at",
        params![run_id, epoch, holder, now()],
    )?;
    tx.commit()?;
    Ok(epoch)
}

/// Is `epoch` still the run's current fencing token?
pub fn fence_current(db: &DbHandle, run_id: &str, epoch: i64) -> anyhow::Result<bool> {
    let c = conn(db)?;
    let cur: Option<i64> = c.query_row("SELECT epoch FROM agency_run_fences WHERE run_id = ?1", params![run_id], |r| r.get(0)).optional()?;
    Ok(cur == Some(epoch))
}

fn fence_in(tx: &rusqlite::Transaction<'_>, run_id: &str) -> rusqlite::Result<Option<i64>> {
    tx.query_row("SELECT epoch FROM agency_run_fences WHERE run_id = ?1", params![run_id], |r| r.get(0)).optional()
}

/// Outcome of [`prepare`].
#[derive(Debug, Clone, PartialEq)]
pub enum Prepared {
    /// New claim: execute once under `chain_key`, then [`commit`].
    Fresh { chain_key: String },
    /// A previous holder claimed the key and died before committing. The
    /// caller first asks the chain whether `old_chain_key` committed; if not,
    /// it executes under `chain_key`.
    Takeover { old_chain_key: String, chain_key: String },
    /// Already committed: serve this result, do not execute.
    Committed(String),
    /// Committed earlier with different arguments: the replay diverged.
    Diverged,
    /// Failed earlier with a non-retryable error: do not retry.
    FailedPermanent(String),
    /// Claimed by a dead holder, and the effect is not retry-safe.
    Unknown,
    /// This worker's fencing token is no longer current.
    Stale,
}

/// Error class of a failed effect (memo A §2.4: retryable vs non-retryable).
pub fn classify_error(msg: &str) -> &'static str {
    let m = msg.to_lowercase();
    let permanent = ["permission denied", "not allowed", "refused", "policy", "invalid", "unauthorized", "forbidden",
        "not found", "inside the server", "no such file"];
    if permanent.iter().any(|k| m.contains(k)) { "non_retryable" } else { "retryable" }
}

/// Phase 1: claim `key` for this drive (fence-checked, in one transaction).
#[allow(clippy::too_many_arguments)]
pub fn prepare(db: &DbHandle, key: &str, run_id: &str, node: &str, tool: &str, args_hash: &str, epoch: i64, retry_safe: bool) -> anyhow::Result<Prepared> {
    let mut c = conn(db)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if fence_in(&tx, run_id)? != Some(epoch) {
        return Ok(Prepared::Stale);
    }
    let row: Option<(String, String, Option<String>, Option<String>, Option<String>, i64, Option<String>)> = tx
        .query_row(
            "SELECT status, args_hash, result, error_class, error, epoch, chain_key FROM agency_effect_journal WHERE idempotency_key = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()?;
    let chain_key = format!("{key}#e{epoch}");
    let ts = now();
    let out = match row {
        Some((status, h, result, _, _, _, _)) if status == "committed" => {
            return Ok(if h == args_hash { Prepared::Committed(result.unwrap_or_default()) } else { Prepared::Diverged });
        }
        Some((status, _, _, class, err, _, _)) if status == "failed" && class.as_deref() == Some("non_retryable") => {
            return Ok(Prepared::FailedPermanent(err.unwrap_or_default()));
        }
        Some((status, _, _, _, _, _, old)) if status == "prepared" => {
            if !retry_safe {
                return Ok(Prepared::Unknown);
            }
            Prepared::Takeover { old_chain_key: old.unwrap_or_else(|| chain_key.clone()), chain_key: chain_key.clone() }
        }
        _ => Prepared::Fresh { chain_key: chain_key.clone() },
    };
    tx.execute(
        "INSERT INTO agency_effect_journal (idempotency_key, run_id, node_id, tool, args_hash, epoch, chain_key, status, retry_safe, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'prepared', ?8, ?9, ?9)
         ON CONFLICT(idempotency_key) DO UPDATE SET args_hash = excluded.args_hash, epoch = excluded.epoch, chain_key = excluded.chain_key,
            status = 'prepared', retry_safe = excluded.retry_safe, error_class = NULL, error = NULL, updated_at = excluded.updated_at",
        params![key, run_id, node, tool, args_hash, epoch, chain_key, retry_safe as i64, ts],
    )?;
    tx.commit()?;
    Ok(out)
}

/// Phase 2: commit the result. Compare-and-set on the fencing token and on
/// this drive's claim; `false` = stale (another worker took the run over).
pub fn commit(db: &DbHandle, key: &str, run_id: &str, epoch: i64, result: &str) -> anyhow::Result<bool> {
    let mut c = conn(db)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if fence_in(&tx, run_id)? != Some(epoch) {
        return Ok(false);
    }
    let n = tx.execute(
        "UPDATE agency_effect_journal SET status = 'committed', result = ?1, updated_at = ?2
         WHERE idempotency_key = ?3 AND run_id = ?4 AND epoch = ?5 AND status = 'prepared'",
        params![result, now(), key, run_id, epoch],
    )?;
    tx.commit()?;
    Ok(n == 1)
}

/// Record a failed attempt with its error class (fence-checked like commit).
pub fn fail(db: &DbHandle, key: &str, run_id: &str, epoch: i64, error: &str) -> anyhow::Result<bool> {
    let c = conn(db)?;
    let n = c.execute(
        "UPDATE agency_effect_journal SET status = 'failed', error_class = ?1, error = ?2, updated_at = ?3
         WHERE idempotency_key = ?4 AND run_id = ?5 AND epoch = ?6 AND status = 'prepared'
           AND (SELECT epoch FROM agency_run_fences WHERE run_id = ?5) = ?6",
        params![classify_error(error), error, now(), key, run_id, epoch],
    )?;
    Ok(n == 1)
}

/// Journal a model output (or any value later steps depend on) in one step:
/// committed straight away under the fence. `false` = stale.
pub fn journal_value(db: &DbHandle, key: &str, run_id: &str, node: &str, kind: &str, epoch: i64, value: &Value) -> anyhow::Result<bool> {
    let mut c = conn(db)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if fence_in(&tx, run_id)? != Some(epoch) {
        return Ok(false);
    }
    let ts = now();
    tx.execute(
        "INSERT INTO agency_effect_journal (idempotency_key, run_id, node_id, tool, args_hash, epoch, status, result, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, '', ?5, 'committed', ?6, ?7, ?7)
         ON CONFLICT(idempotency_key) DO NOTHING",
        params![key, run_id, node, kind, epoch, value.to_string(), ts],
    )?;
    tx.commit()?;
    Ok(true)
}

/// A person resolved an `effect_outcome_unknown` item: the effect either
/// happened (commit it, so it is never re-applied) or did not (mark it a
/// retryable failure, so the next drive applies it once).
pub fn resolve_unknown(db: &DbHandle, key: &str, applied: bool) -> anyhow::Result<()> {
    let c = conn(db)?;
    if applied {
        c.execute("UPDATE agency_effect_journal SET status = 'committed', result = COALESCE(result, 'operator:applied'), updated_at = ?1
                   WHERE idempotency_key = ?2 AND status = 'prepared'", params![now(), key])?;
    } else {
        c.execute("UPDATE agency_effect_journal SET status = 'failed', error_class = 'retryable', error = 'operator: not applied', updated_at = ?1
                   WHERE idempotency_key = ?2 AND status = 'prepared'", params![now(), key])?;
    }
    Ok(())
}

/// A journaled value for `key`, if one was committed.
pub fn journaled_value(db: &DbHandle, key: &str) -> anyhow::Result<Option<Value>> {
    let c = conn(db)?;
    let r: Option<String> = c
        .query_row("SELECT result FROM agency_effect_journal WHERE idempotency_key = ?1 AND status = 'committed'", params![key], |r| r.get(0))
        .optional()?;
    Ok(r.and_then(|s| serde_json::from_str(&s).ok()))
}

/// Journal rows of one run (operator view / tests).
pub fn journal(db: &DbHandle, run_id: &str) -> anyhow::Result<Vec<Value>> {
    let c = conn(db)?;
    let mut q = c.prepare(
        "SELECT idempotency_key, node_id, tool, epoch, status, error_class, created_at, updated_at
         FROM agency_effect_journal WHERE run_id = ?1 ORDER BY created_at, idempotency_key",
    )?;
    let rows = q.query_map(params![run_id], |r| {
        Ok(json!({ "idempotency_key": r.get::<_, String>(0)?, "node_id": r.get::<_, Option<String>>(1)?, "tool": r.get::<_, String>(2)?,
            "epoch": r.get::<_, i64>(3)?, "status": r.get::<_, String>(4)?, "error_class": r.get::<_, Option<String>>(5)?,
            "created_at": r.get::<_, String>(6)?, "updated_at": r.get::<_, String>(7)? }))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ── org policy ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrgSafety {
    pub require_non_requester_approval: Option<bool>,
    pub runs_per_hour: Option<u64>,
    pub max_steps: Option<u64>,
    pub max_wall_secs: Option<u64>,
    pub max_usd: Option<f64>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
}

pub fn load_org(db: &DbHandle, org: &str) -> anyhow::Result<OrgSafety> {
    let c = conn(db)?;
    let r = c
        .query_row(
            "SELECT require_non_requester_approval, runs_per_hour, max_steps, max_wall_secs, max_usd, updated_by, updated_at
             FROM agency_org_safety WHERE org_id = ?1",
            params![org],
            |r| {
                Ok(OrgSafety {
                    require_non_requester_approval: r.get::<_, Option<i64>>(0)?.map(|v| v != 0),
                    runs_per_hour: r.get::<_, Option<i64>>(1)?.map(|v| v.max(0) as u64),
                    max_steps: r.get::<_, Option<i64>>(2)?.map(|v| v.max(0) as u64),
                    max_wall_secs: r.get::<_, Option<i64>>(3)?.map(|v| v.max(0) as u64),
                    max_usd: r.get(4)?,
                    updated_by: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            },
        )
        .optional()?;
    Ok(r.unwrap_or_default())
}

pub fn save_org(db: &DbHandle, org: &str, p: &OrgSafety, by: &str) -> anyhow::Result<()> {
    let c = conn(db)?;
    c.execute(
        "INSERT INTO agency_org_safety (org_id, require_non_requester_approval, runs_per_hour, max_steps, max_wall_secs, max_usd, updated_by, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(org_id) DO UPDATE SET require_non_requester_approval = excluded.require_non_requester_approval,
            runs_per_hour = excluded.runs_per_hour, max_steps = excluded.max_steps, max_wall_secs = excluded.max_wall_secs,
            max_usd = excluded.max_usd, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![org, p.require_non_requester_approval.map(i64::from), p.runs_per_hour.map(|v| v as i64), p.max_steps.map(|v| v as i64),
            p.max_wall_secs.map(|v| v as i64), p.max_usd, by, now()],
    )?;
    Ok(())
}

/// Members of an organization (0 for personal `user:` orgs and unknown ids).
pub fn org_member_count(db: &DbHandle, org: &str) -> usize {
    if org.is_empty() || org.starts_with("user:") {
        return 0;
    }
    conn(db)
        .and_then(|c| Ok(c.query_row("SELECT COUNT(*) FROM organization_members WHERE organization_id = ?1", params![org], |r| r.get::<_, i64>(0))?))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// Is `user` a member of `org`? (Personal orgs: only their own user.)
pub fn is_org_member(db: &DbHandle, org: &str, user: &str) -> bool {
    if let Some(u) = org.strip_prefix("user:") {
        return u == user;
    }
    conn(db)
        .and_then(|c| Ok(c.query_row("SELECT 1 FROM organization_members WHERE organization_id = ?1 AND user_id = ?2", params![org, user], |_| Ok(()))
            .optional()?))
        .map(|r| r.is_some())
        .unwrap_or(false)
}

/// Effective non-requester rule for an org: the stored policy, else on for
/// 2+ member orgs and off for single-member / personal orgs.
pub fn requires_non_requester(db: &DbHandle, org: &str) -> bool {
    match load_org(db, org).ok().and_then(|p| p.require_non_requester_approval) {
        Some(v) => v,
        None => org_member_count(db, org) >= 2,
    }
}

pub fn is_consequential(att: &Value) -> bool {
    att["consequential"] == true || att["reason"].as_str().is_some_and(|r| CONSEQUENTIAL_REASONS.contains(&r))
}

/// Public policy view (stored values + the effective result).
pub fn policy_view(db: &DbHandle, org: &str) -> Value {
    let p = load_org(db, org).unwrap_or_default();
    let caps = RunCaps::from_env().tightened(&p);
    json!({ "object": "agency_safety_policy", "org_id": org,
        "require_non_requester_approval": p.require_non_requester_approval,
        "effective_require_non_requester_approval": requires_non_requester(db, org),
        "runs_per_hour": p.runs_per_hour, "max_steps": p.max_steps, "max_wall_secs": p.max_wall_secs, "max_usd": p.max_usd,
        "effective_caps": caps.to_json(), "updated_by": p.updated_by, "updated_at": p.updated_at })
}

// ── 3. caps ─────────────────────────────────────────────────────────────────

/// Per-run hard caps beyond the request's own budget. Server defaults keep
/// every run bounded (memo A pitfall: unbounded defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct RunCaps {
    pub max_steps: Option<u64>,
    pub max_wall_secs: Option<u64>,
    pub max_usd: Option<f64>,
}

impl RunCaps {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let get: &dyn Fn(&str) -> Option<String> = &get;
        RunCaps {
            max_steps: env_num(get, RUN_MAX_STEPS_ENV).unwrap_or(Some(DEFAULT_RUN_MAX_STEPS)),
            max_wall_secs: env_num(get, RUN_MAX_WALL_ENV).unwrap_or(Some(DEFAULT_RUN_MAX_WALL_SECS)),
            max_usd: env_num(get, RUN_MAX_USD_ENV).unwrap_or(Some(DEFAULT_RUN_MAX_USD)),
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Org policy only tightens.
    pub fn tightened(&self, p: &OrgSafety) -> Self {
        fn min<T: PartialOrd + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) { (Some(x), Some(y)) => Some(if y < x { y } else { x }), (x, None) => x, (None, y) => y }
        }
        RunCaps { max_steps: min(self.max_steps, p.max_steps), max_wall_secs: min(self.max_wall_secs, p.max_wall_secs), max_usd: min(self.max_usd, p.max_usd) }
    }

    /// A per-run override an approver granted (`run.safety.caps`) replaces
    /// the defaults for the dimensions it names.
    pub fn with_override(&self, run: &Value) -> Self {
        let o = &run["safety"]["caps"];
        RunCaps {
            max_steps: o["max_steps"].as_u64().or(self.max_steps),
            max_wall_secs: o["max_wall_secs"].as_u64().or(self.max_wall_secs),
            max_usd: o["max_usd"].as_f64().or(self.max_usd),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "max_steps": self.max_steps, "max_wall_secs": self.max_wall_secs, "max_usd": self.max_usd })
    }

    /// Which cap the run has reached: `(dimension, detail)`.
    pub fn reached(&self, run: &Value, attention: &[Value]) -> Option<(&'static str, String)> {
        let u = &run["budget_usage"];
        let steps = u["steps"].as_u64().unwrap_or(0);
        if let Some(m) = self.max_steps.filter(|m| steps >= *m) {
            return Some(("max_steps", format!("{steps} steps used; the per-run cap is {m}.")));
        }
        let usd = u["cost_usd"].as_f64().unwrap_or(0.0);
        if let Some(m) = self.max_usd.filter(|m| usd >= *m) {
            return Some(("max_usd", format!("${usd:.2} spent; the per-run cap is ${m:.2}.")));
        }
        let wall = wall_secs(run, attention);
        if let Some(m) = self.max_wall_secs.filter(|m| wall >= *m) {
            return Some(("max_wall_secs", format!("{wall} s of wall time used (attention wait excluded); the per-run cap is {m} s.")));
        }
        None
    }
}

/// Wall time since the run first started executing, minus time spent
/// waiting on people (resolved attention).
pub fn wall_secs(run: &Value, attention: &[Value]) -> u64 {
    let Some(start) = run["safety"]["started_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) else { return 0 };
    let total = (chrono::Utc::now() - start.with_timezone(&chrono::Utc)).num_milliseconds().max(0) as u64;
    let waited = super::executor::attention_wait_ms(attention);
    total.saturating_sub(waited) / 1000
}

/// Stamp the first execution start (wall-time origin) and count drives.
pub fn note_drive(run: &mut Value, epoch: i64) {
    if !run["safety"].is_object() {
        run["safety"] = json!({});
    }
    if run["safety"]["started_at"].is_null() {
        run["safety"]["started_at"] = json!(now());
    }
    run["safety"]["fence_epoch"] = json!(epoch);
}

/// Runs of `org` that started executing within the last hour.
pub fn runs_started_last_hour<'a>(runs: impl IntoIterator<Item = (&'a Value, &'a Value)>, org: &str, except: &str) -> usize {
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
    runs.into_iter()
        .filter(|(run, ir)| super::guard::run_org(ir) == org && run["id"] != except)
        .filter_map(|(run, _)| run["safety"]["started_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()))
        .filter(|t| t.with_timezone(&chrono::Utc) > cutoff)
        .count()
}

// ── 3. stuck detection ──────────────────────────────────────────────────────

pub const REPEAT_LIMIT: usize = 4;
pub const ERROR_LIMIT: u32 = 3;
pub const PINGPONG_CYCLES: usize = 6;

/// OpenHands-style stuck heuristics over one drive's actions.
#[derive(Debug)]
pub struct StuckDetector {
    history: Vec<(String, String)>,
    seen: std::collections::HashSet<(String, String)>,
    errors: HashMap<String, u32>,
    since_progress: u32,
    no_progress_limit: u32,
}

impl Default for StuckDetector {
    fn default() -> Self {
        let n = std::env::var(NO_PROGRESS_ENV).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_NO_PROGRESS_STEPS);
        Self::with_limit(n)
    }
}

impl StuckDetector {
    pub fn with_limit(no_progress_limit: u32) -> Self {
        StuckDetector { history: vec![], seen: Default::default(), errors: HashMap::new(), since_progress: 0, no_progress_limit: no_progress_limit.max(1) }
    }

    /// One action and what it observed. Returns why the run is stuck, if it is.
    pub fn observe(&mut self, action: &str, observation: &str) -> Option<String> {
        let entry = (action.to_string(), observation.to_string());
        if self.seen.insert(entry.clone()) {
            self.since_progress = 0;
        } else {
            self.since_progress += 1;
        }
        self.history.push(entry);
        let h = &self.history;
        if h.len() >= REPEAT_LIMIT && h[h.len() - REPEAT_LIMIT..].iter().all(|e| *e == h[h.len() - 1]) {
            return Some(format!("the same action returned the same result {REPEAT_LIMIT} times in a row ({action})"));
        }
        let span = PINGPONG_CYCLES * 2;
        if h.len() >= span {
            let w = &h[h.len() - span..];
            let (a, b) = (&w[0], &w[1]);
            if a.0 != b.0 && w.iter().enumerate().all(|(i, e)| e == if i % 2 == 0 { a } else { b }) {
                return Some(format!("the run alternated between two actions for {PINGPONG_CYCLES} cycles ({} / {})", a.0, b.0));
            }
        }
        self.check_progress()
    }

    /// One failed action. Same action failing [`ERROR_LIMIT`] times = stuck.
    pub fn observe_error(&mut self, action: &str) -> Option<String> {
        let n = self.errors.entry(action.to_string()).or_insert(0);
        *n += 1;
        self.since_progress += 1;
        if *n >= ERROR_LIMIT {
            return Some(format!("the same action failed {n} times ({action})"));
        }
        self.check_progress()
    }

    /// A step that produced nothing new (e.g. a node closed as failed).
    pub fn observe_idle(&mut self) -> Option<String> {
        self.since_progress += 1;
        self.check_progress()
    }

    pub fn progressed(&mut self) {
        self.since_progress = 0;
    }

    fn check_progress(&self) -> Option<String> {
        (self.since_progress >= self.no_progress_limit)
            .then(|| format!("{} steps in a row produced nothing new", self.since_progress))
    }
}

// ── 4. prod lanes ───────────────────────────────────────────────────────────

use allternit_factory_engine::kernel::router::{PoolEntry, Residency, StaticModelPool};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LaneMode {
    ApiFirst,
    ApiOnly,
    Any,
}

pub fn lane_mode() -> LaneMode {
    match std::env::var(LANES_ENV).unwrap_or_default().trim() {
        "api_only" => LaneMode::ApiOnly,
        "any" => LaneMode::Any,
        _ => LaneMode::ApiFirst,
    }
}

/// `api` | `subscription` | `local`. The pool's `x-lane` wins; else local
/// residency; else the `x-model_ref` source listed in
/// `ALLTERNIT_AGENCY_SUBSCRIPTION_SOURCES` (operator config, no names in code).
pub fn lane_of(e: &PoolEntry) -> &'static str {
    let ext = e.extensions.as_ref();
    match ext.and_then(|x| x.get("x-lane")).and_then(Value::as_str) {
        Some("subscription") => return "subscription",
        Some("local") => return "local",
        Some("api") => return "api",
        _ => {}
    }
    if e.residency != Residency::Remote {
        return "local";
    }
    let src = ext.and_then(|x| x.get("x-model_ref")).and_then(Value::as_str).and_then(|r| r.split_once('/')).map(|(p, _)| p.to_string());
    let subs = std::env::var(SUBSCRIPTION_SOURCES_ENV).unwrap_or_default();
    if src.is_some_and(|s| subs.split(',').map(str::trim).any(|x| !x.is_empty() && x == s)) {
        return "subscription";
    }
    "api"
}

pub fn lane_for(pool: Option<&StaticModelPool>, backend: &str) -> Option<&'static str> {
    pool?.entries.iter().find(|e| e.backend_id == backend).map(lane_of)
}

static COOLDOWN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn cooldown_secs() -> u64 {
    std::env::var(LANE_COOLDOWN_ENV).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_LANE_COOLDOWN_SECS)
}

/// A backend failed: skip it in every run for the cooldown window.
pub fn cool_down(backend: &str) {
    let until = Instant::now() + Duration::from_secs(cooldown_secs());
    COOLDOWN.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(backend.to_string(), until);
}

pub fn cooling(backend: &str) -> bool {
    let mut g = COOLDOWN.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    match m.get(backend) {
        Some(t) if *t > Instant::now() => true,
        Some(_) => { m.remove(backend); false }
        None => false,
    }
}

/// Split the pool for prod: cooled-down backends out; under `api_first`
/// subscription lanes are held back as the fallback (used only when no other
/// lane routes), under `api_only` they are dropped.
pub fn split_lanes(pool: StaticModelPool, mode: LaneMode) -> (StaticModelPool, Vec<PoolEntry>) {
    let (mut primary, mut fallback) = (vec![], vec![]);
    for e in pool.entries.into_iter().filter(|e| !cooling(&e.backend_id)) {
        match (mode, lane_of(&e)) {
            (LaneMode::Any, _) | (_, "api" | "local") => primary.push(e),
            (LaneMode::ApiFirst, _) => fallback.push(e),
            (LaneMode::ApiOnly, _) => {}
        }
    }
    if primary.is_empty() {
        // No API/local lane at all: the fallback is all there is.
        return (StaticModelPool { entries: fallback }, vec![]);
    }
    (StaticModelPool { entries: primary }, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> DbHandle {
        DbHandle::new_memory().unwrap()
    }

    #[test]
    fn agency_safety_fence_refuses_stale_worker_commit() {
        let d = db();
        let e1 = acquire_fence(&d, "run_a", "w1").unwrap();
        assert!(matches!(prepare(&d, "run_a:N14:fs:1", "run_a", "N14", "fs", "h1", e1, true).unwrap(), Prepared::Fresh { .. }));
        // A second worker takes the run over before the first commits.
        let e2 = acquire_fence(&d, "run_a", "w2").unwrap();
        assert!(e2 > e1);
        assert!(!commit(&d, "run_a:N14:fs:1", "run_a", e1, "r1").unwrap(), "stale commit refused");
        assert_eq!(prepare(&d, "run_a:N15:t:1", "run_a", "N15", "t", "h", e1, true).unwrap(), Prepared::Stale);
        assert!(!fence_current(&d, "run_a", e1).unwrap());
        // The current holder retries the dead claim (retry-safe) and commits.
        match prepare(&d, "run_a:N14:fs:1", "run_a", "N14", "fs", "h1", e2, true).unwrap() {
            Prepared::Takeover { old_chain_key, chain_key } => {
                assert!(old_chain_key.ends_with(&format!("#e{e1}")));
                assert!(chain_key.ends_with(&format!("#e{e2}")));
            }
            p => panic!("{p:?}"),
        }
        assert!(commit(&d, "run_a:N14:fs:1", "run_a", e2, "r2").unwrap());
    }

    #[test]
    fn agency_safety_committed_effect_is_never_reapplied() {
        let d = db();
        let e1 = acquire_fence(&d, "run_b", "w").unwrap();
        prepare(&d, "k:N02:clone:1", "run_b", "N02", "clone", "h", e1, true).unwrap();
        assert!(commit(&d, "k:N02:clone:1", "run_b", e1, "workspace:ready").unwrap());
        let e2 = acquire_fence(&d, "run_b", "w").unwrap();
        assert_eq!(prepare(&d, "k:N02:clone:1", "run_b", "N02", "clone", "h", e2, true).unwrap(), Prepared::Committed("workspace:ready".into()));
        assert_eq!(prepare(&d, "k:N02:clone:1", "run_b", "N02", "clone", "other", e2, true).unwrap(), Prepared::Diverged);
        assert_eq!(journal(&d, "run_b").unwrap()[0]["status"], "committed");
    }

    #[test]
    fn agency_safety_unknown_outcome_and_error_classes() {
        let d = db();
        let e1 = acquire_fence(&d, "run_c", "w").unwrap();
        prepare(&d, "c:N30:push:1", "run_c", "N30", "push", "h", e1, false).unwrap();
        let e2 = acquire_fence(&d, "run_c", "w").unwrap();
        assert_eq!(prepare(&d, "c:N30:push:1", "run_c", "N30", "push", "h", e2, false).unwrap(), Prepared::Unknown);
        prepare(&d, "c:N31:t:1", "run_c", "N31", "t", "h", e2, true).unwrap();
        assert!(fail(&d, "c:N31:t:1", "run_c", e2, "gate refused tool.x: policy").unwrap());
        assert!(matches!(prepare(&d, "c:N31:t:1", "run_c", "N31", "t", "h", e2, true).unwrap(), Prepared::FailedPermanent(_)));
        prepare(&d, "c:N32:t:1", "run_c", "N32", "t", "h", e2, true).unwrap();
        assert!(fail(&d, "c:N32:t:1", "run_c", e2, "connection reset").unwrap());
        assert!(matches!(prepare(&d, "c:N32:t:1", "run_c", "N32", "t", "h", e2, true).unwrap(), Prepared::Fresh { .. }), "retryable failure is retried");
        assert_eq!(classify_error("timed out"), "retryable");
    }

    #[test]
    fn agency_safety_journaled_model_output_replays() {
        let d = db();
        let e = acquire_fence(&d, "run_d", "w").unwrap();
        let v = json!({ "path": "a.js", "content": "x" });
        assert!(journal_value(&d, "run_d:N11:model.propose:1", "run_d", "N11", "model.propose", e, &v).unwrap());
        assert_eq!(journaled_value(&d, "run_d:N11:model.propose:1").unwrap(), Some(v.clone()));
        // A later (or stale) write never replaces the journaled output.
        let e2 = acquire_fence(&d, "run_d", "w").unwrap();
        journal_value(&d, "run_d:N11:model.propose:1", "run_d", "N11", "model.propose", e2, &json!({"path": "b"})).unwrap();
        assert_eq!(journaled_value(&d, "run_d:N11:model.propose:1").unwrap(), Some(v));
        assert!(!journal_value(&d, "run_d:N17:model.propose:2", "run_d", "N17", "model.propose", e, &json!({})).unwrap(), "stale");
    }

    #[test]
    fn agency_safety_caps_default_bounded_and_tightened_by_org() {
        let c = RunCaps::from_lookup(|_| None);
        assert_eq!(c.max_steps, Some(DEFAULT_RUN_MAX_STEPS));
        assert_eq!(c.max_wall_secs, Some(DEFAULT_RUN_MAX_WALL_SECS));
        let off = RunCaps::from_lookup(|k| (k == RUN_MAX_USD_ENV).then(|| "off".to_string()));
        assert_eq!(off.max_usd, None);
        let t = c.tightened(&OrgSafety { max_steps: Some(5), max_usd: Some(100.0), ..Default::default() });
        assert_eq!(t.max_steps, Some(5));
        assert_eq!(t.max_usd, Some(DEFAULT_RUN_MAX_USD), "org policy never raises a ceiling");
        let run = json!({ "budget_usage": { "steps": 5, "cost_usd": 0.1 } });
        assert_eq!(t.reached(&run, &[]).unwrap().0, "max_steps");
        let mut run = json!({ "budget_usage": { "steps": 1 } });
        note_drive(&mut run, 1);
        assert!(t.reached(&run, &[]).is_none());
        run["safety"]["started_at"] = json!((chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339());
        assert_eq!(t.reached(&run, &[]).unwrap().0, "max_wall_secs");
        let raised = t.with_override(&json!({ "safety": { "caps": { "max_wall_secs": 100000 } } }));
        assert!(raised.reached(&run, &[]).is_none());
    }

    #[test]
    fn agency_safety_stuck_detector_heuristics() {
        let mut s = StuckDetector::with_limit(100);
        for i in 0..3 { assert!(s.observe("N15:test", "FAIL").is_none(), "{i}"); }
        assert!(s.observe("N15:test", "FAIL").unwrap().contains("same result"));
        let mut s = StuckDetector::with_limit(100);
        assert!(s.observe_error("N02:clone").is_none());
        assert!(s.observe_error("N02:clone").is_none());
        assert!(s.observe_error("N02:clone").is_some());
        let mut s = StuckDetector::with_limit(100);
        let mut hit = None;
        for i in 0..PINGPONG_CYCLES * 2 {
            hit = if i % 2 == 0 { s.observe("A", "x") } else { s.observe("B", "y") };
        }
        assert!(hit.unwrap().contains("alternated"));
        let mut s = StuckDetector::with_limit(3);
        assert!(s.observe("A", "1").is_none());
        assert!(s.observe_idle().is_none());
        assert!(s.observe_idle().is_none());
        assert!(s.observe_idle().unwrap().contains("nothing new"));
        // A normal bug-fix shape (distinct observations) is never stuck.
        let mut s = StuckDetector::with_limit(5);
        for (a, o) in [("t", "FAIL:1"), ("w", "p1"), ("t", "FAIL:2"), ("w", "p2"), ("t", "PASS")] {
            assert!(s.observe(a, o).is_none());
        }
    }

    #[test]
    fn agency_safety_non_requester_default_by_membership() {
        let d = db();
        let c = d.connect().unwrap();
        c.execute("INSERT OR IGNORE INTO organizations (id, name) VALUES ('org_1', 'O')", []).unwrap();
        for u in ["ua", "ub"] {
            c.execute("INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)", params![u, format!("{u}@t.local")]).unwrap();
        }
        c.execute("INSERT INTO organization_members (id, organization_id, user_id, role) VALUES ('m1','org_1','ua','owner')", []).unwrap();
        assert!(!requires_non_requester(&d, "org_1"), "single-member org: off by default");
        assert!(!requires_non_requester(&d, "user:ua"));
        c.execute("INSERT INTO organization_members (id, organization_id, user_id, role) VALUES ('m2','org_1','ub','member')", []).unwrap();
        assert!(requires_non_requester(&d, "org_1"), "2+ members: on by default");
        save_org(&d, "org_1", &OrgSafety { require_non_requester_approval: Some(false), ..Default::default() }, "ua").unwrap();
        assert!(!requires_non_requester(&d, "org_1"), "explicit policy wins");
        assert!(is_org_member(&d, "org_1", "ub") && !is_org_member(&d, "org_1", "uc"));
        assert!(is_consequential(&json!({ "reason": "spend_over_usd" })));
        assert!(!is_consequential(&json!({ "reason": "template_attention" })));
    }

    fn entry(id: &str, lane: Option<&str>, src: &str) -> PoolEntry {
        let mut e = crate::agency_api::executor::tests_support::entry(id);
        let mut ext = serde_json::Map::new();
        ext.insert("x-model_ref".into(), json!(format!("{src}/m")));
        if let Some(l) = lane { ext.insert("x-lane".into(), json!(l)); }
        e.extensions = Some(ext);
        e
    }

    #[test]
    fn agency_safety_lanes_api_first_and_cooldown() {
        let pool = StaticModelPool { entries: vec![entry("b_api", None, "p1"), entry("b_sub", Some("subscription"), "p2")] };
        let (p, fb) = split_lanes(pool.clone(), LaneMode::ApiFirst);
        assert_eq!(p.entries.iter().map(|e| e.backend_id.as_str()).collect::<Vec<_>>(), ["b_api"]);
        assert_eq!(fb.len(), 1);
        let (p, fb) = split_lanes(pool.clone(), LaneMode::ApiOnly);
        assert_eq!((p.entries.len(), fb.len()), (1, 0));
        let only_sub = StaticModelPool { entries: vec![entry("b_sub2", Some("subscription"), "p2")] };
        assert_eq!(split_lanes(only_sub, LaneMode::ApiFirst).0.entries.len(), 1, "fallback is all there is");
        cool_down("b_api_cool");
        let pool = StaticModelPool { entries: vec![entry("b_api_cool", None, "p1")] };
        assert!(split_lanes(pool, LaneMode::Any).0.entries.is_empty(), "cooled backend skipped");
        assert_eq!(lane_of(&entry("x", None, "p1")), "api");
    }
}
