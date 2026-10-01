//! O15 (WP-C1): one cost ledger for every model call.
//!
//! `llm_usage_events` is the ledger (V25 + V27 + V147 + V207). Writers:
//! - the gateway (`llm_gateway::proxy::record_usage_event`, `source = gateway`),
//!   attributed from the request's cost-attribution `tags`
//!   (`surface` / `run_id` / `node_id` / `tier`);
//! - internal `gizzi_completion` calls (`source = internal`), attributed by
//!   the caller's [`LedgerCtx`] ([`scope`] for async callers, [`enter`] for
//!   the sync Agency/template executors that `block_on`);
//! - gizzi-code's own model calls, reported to `POST /usage/ledger`
//!   (`source = gizzi`, existing user auth, no new auth path);
//! - S1 decisions (`source = s1`, `tier = S1`, cost 0 but counted);
//! - Agent Gateway vendor turns (`source = vendor`, `lane = vendor`): the
//!   vendor bills the user's own subscription and reports no usage, so the
//!   row is cost 0 but counted, keyed by the turn's correlation id.
//!
//! Dedupe: gizzi serves the gateway's and `gizzi_completion`'s sessions too,
//! so its per-call reports for a session that allternit-api already metered
//! are dropped (both orderings: an allternit-api row deletes earlier gizzi
//! rows for the same `gizzi_session_id`; a later gizzi report is skipped).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::OnceLock;

use crate::db::DbHandle;

pub const SURFACES: &[&str] = &[
    "chat", "cowork", "code", "browser", "design", "bot", "agency", "template", "memory", "lessons", "s1",
    "batch", "api", "internal",
];
pub const TIERS: &[&str] = &["S0", "S1", "S2", "S3"];
pub const LANES: &[&str] = &["api", "subscription-cli", "local", "vendor"];

fn allowed(v: Option<&str>, set: &[&str]) -> Option<String> {
    v.map(str::trim).filter(|s| set.contains(s)).map(str::to_string)
}

/// Bounded free-text id (run/node ids): trimmed, non-empty, at most 128 chars.
fn id(v: Option<&str>) -> Option<String> {
    v.map(str::trim).filter(|s| !s.is_empty() && s.len() <= 128).map(str::to_string)
}

/// Who a model call is for. Unknown values are dropped, never stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LedgerCtx {
    pub surface: Option<String>,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub tier: Option<String>,
    pub lane: Option<String>,
    pub tenant_id: Option<String>,
    pub user_id: Option<String>,
}

impl LedgerCtx {
    pub fn surface(surface: &str) -> Self {
        Self { surface: allowed(Some(surface), SURFACES), ..Default::default() }
    }
    pub fn run(mut self, run_id: &str, node_id: Option<&str>) -> Self {
        self.run_id = id(Some(run_id));
        self.node_id = id(node_id);
        self
    }
    pub fn tier(mut self, tier: &str) -> Self {
        self.tier = allowed(Some(tier), TIERS);
        self
    }
    pub fn lane(mut self, lane: &str) -> Self {
        self.lane = allowed(Some(lane), LANES);
        self
    }
    pub fn tenant(mut self, tenant_id: Option<&str>, user_id: Option<&str>) -> Self {
        self.tenant_id = id(tenant_id);
        self.user_id = id(user_id);
        self
    }
}

tokio::task_local! {
    static TASK_CTX: LedgerCtx;
}
thread_local! {
    static THREAD_CTX: RefCell<Option<LedgerCtx>> = const { RefCell::new(None) };
}

/// Run `fut` with `ctx` as the ledger attribution of every model call in it.
pub async fn scope<F: std::future::Future>(ctx: LedgerCtx, fut: F) -> F::Output {
    TASK_CTX.scope(ctx, fut).await
}

/// Thread-scoped attribution for sync callers that `Handle::block_on` the
/// completion (the future then runs on this thread). Restored on drop.
#[must_use]
pub struct CtxGuard(Option<LedgerCtx>);
pub fn enter(ctx: LedgerCtx) -> CtxGuard {
    CtxGuard(THREAD_CTX.with(|c| c.replace(Some(ctx))))
}
impl Drop for CtxGuard {
    fn drop(&mut self) {
        let prev = self.0.take();
        THREAD_CTX.with(|c| *c.borrow_mut() = prev);
    }
}

/// The attribution in effect: task scope first, then thread scope.
pub fn current() -> Option<LedgerCtx> {
    TASK_CTX.try_with(Clone::clone).ok().or_else(|| THREAD_CTX.with(|c| c.borrow().clone()))
}

/// One ledger row (one model call or one S1 decision).
#[derive(Debug, Clone, Default)]
pub struct LedgerRow {
    pub source: &'static str,
    pub ctx: LedgerCtx,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_microdollars: i64,
    pub latency_ms: i64,
    pub status: String,
    pub gizzi_session_id: Option<String>,
    /// Dedupe key for reported rows (a re-delivery updates, never doubles).
    pub idempotency_key: Option<String>,
    pub decision_served_by_s1: bool,
    pub s1_incumbent_cost_microdollars: Option<i64>,
}

/// Write one row. `source = gizzi` rows for a session allternit-api already
/// metered are skipped (returns `Ok(None)`); other rows with a session id
/// delete earlier gizzi rows for it (see module docs).
pub fn insert(conn: &Connection, row: &LedgerRow) -> rusqlite::Result<Option<String>> {
    let sid = row.gizzi_session_id.as_deref().filter(|s| !s.is_empty());
    if let Some(sid) = sid {
        if row.source == "gizzi" {
            let metered: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM llm_usage_events WHERE gizzi_session_id = ?1 AND COALESCE(source, 'gateway') != 'gizzi' LIMIT 1",
                    params![sid],
                    |r| r.get(0),
                )
                .optional()?;
            if metered.is_some() {
                return Ok(None);
            }
        } else {
            drop_gizzi_duplicates(conn, sid)?;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let c = &row.ctx;
    conn.execute(
        "INSERT INTO llm_usage_events
            (id, user_id, tenant_id, provider_id, model_id, prompt_tokens, completion_tokens,
             reasoning_tokens, cached_tokens, cache_write_tokens, cost_microdollars, latency_ms,
             status, gizzi_session_id, idempotency_key, source, surface, run_id, node_id, tier,
             lane, decision_served_by_s1, s1_incumbent_cost_microdollars)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23)
         ON CONFLICT(idempotency_key) DO UPDATE SET
             prompt_tokens = excluded.prompt_tokens, completion_tokens = excluded.completion_tokens,
             reasoning_tokens = excluded.reasoning_tokens, cached_tokens = excluded.cached_tokens,
             cache_write_tokens = excluded.cache_write_tokens,
             cost_microdollars = excluded.cost_microdollars, latency_ms = excluded.latency_ms,
             status = excluded.status",
        params![
            id,
            c.user_id,
            c.tenant_id,
            row.provider_id,
            row.model_id,
            row.prompt_tokens.max(0),
            row.completion_tokens.max(0),
            row.reasoning_tokens.max(0),
            row.cache_read_tokens.max(0),
            row.cache_write_tokens.max(0),
            row.cost_microdollars.max(0),
            row.latency_ms.max(0),
            if row.status.is_empty() { "ok" } else { row.status.as_str() },
            sid,
            row.idempotency_key,
            row.source,
            allowed(c.surface.as_deref(), SURFACES),
            id_or_none(&c.run_id),
            id_or_none(&c.node_id),
            allowed(c.tier.as_deref(), TIERS),
            allowed(c.lane.as_deref(), LANES),
            row.decision_served_by_s1 as i64,
            row.s1_incumbent_cost_microdollars.map(|v| v.max(0)),
        ],
    )?;
    Ok(Some(id))
}

fn id_or_none(v: &Option<String>) -> Option<String> {
    id(v.as_deref())
}

fn drop_gizzi_duplicates(conn: &Connection, sid: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM llm_usage_events WHERE gizzi_session_id = ?1 AND source = 'gizzi'", params![sid])
}

/// Gateway rows: stamp the ledger keys on the row `record_usage_event` just
/// wrote. Attribution comes from the request's cost-attribution tags
/// (`surface`, `run_id`, `node_id`, `tier`); default surface is `batch` for
/// batch sub-requests, else `api`. The gateway lane is always `api`.
pub fn stamp_gateway_row(
    conn: &Connection,
    row_id: &str,
    tags_json: Option<&str>,
    batch_id: Option<&str>,
    cache_write_tokens: i64,
    gizzi_session_id: Option<&str>,
) -> rusqlite::Result<()> {
    let tags: Value = tags_json.and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
    let tag = |k: &str| tags.get(k).and_then(Value::as_str);
    let surface = allowed(tag("surface"), SURFACES)
        .unwrap_or_else(|| if batch_id.is_some() { "batch" } else { "api" }.to_string());
    conn.execute(
        "UPDATE llm_usage_events SET source = 'gateway', surface = ?2, run_id = ?3, node_id = ?4,
             tier = ?5, lane = 'api', cache_write_tokens = ?6
         WHERE id = ?1",
        params![row_id, surface, id(tag("run_id")), id(tag("node_id")), allowed(tag("tier"), TIERS), cache_write_tokens.max(0)],
    )?;
    if let Some(sid) = gizzi_session_id.filter(|s| !s.is_empty()) {
        drop_gizzi_duplicates(conn, sid)?;
    }
    Ok(())
}

static LEDGER_DB: OnceLock<DbHandle> = OnceLock::new();

/// Called once at startup with the app DB, so the free-function completion
/// helpers can write the ledger.
pub fn install(db: DbHandle) {
    let _ = LEDGER_DB.set(db);
}

/// Best-effort write to the installed DB (metering never fails a call).
pub fn record(row: LedgerRow) {
    let Some(db) = LEDGER_DB.get() else { return };
    if let Err(e) = db.connect().and_then(|c| insert(&c, &row)) {
        tracing::warn!(error = %e, source = row.source, "usage ledger write failed");
    }
}

/// Row for one internal `gizzi_completion` call, attributed by [`current`]
/// (no scope = surface `internal`, lane `api`, tier `S2`).
#[allow(clippy::too_many_arguments)]
pub fn internal_row(
    provider_id: &str,
    model_id: &str,
    session_id: Option<&str>,
    tokens_in: u64,
    tokens_out: u64,
    cache_read: u64,
    cache_write: u64,
    cost_usd: f64,
    latency_ms: u64,
    ok: bool,
) -> LedgerRow {
    let mut ctx = current().unwrap_or_default();
    ctx.surface = ctx.surface.or_else(|| Some("internal".into()));
    ctx.tier = ctx.tier.or_else(|| Some("S2".into()));
    ctx.lane = ctx.lane.or_else(|| Some("api".into()));
    LedgerRow {
        source: "internal",
        ctx,
        provider_id: Some(provider_id.to_string()),
        model_id: Some(model_id.to_string()),
        prompt_tokens: tokens_in as i64,
        completion_tokens: tokens_out as i64,
        cache_read_tokens: cache_read as i64,
        cache_write_tokens: cache_write as i64,
        cost_microdollars: (cost_usd * 1_000_000.0).round() as i64,
        latency_ms: latency_ms as i64,
        status: if ok { "ok" } else { "error" }.into(),
        gizzi_session_id: session_id.map(str::to_string),
        ..Default::default()
    }
}

/// Row for one Agent Gateway vendor turn: surface bot, tier S2, lane vendor,
/// model = the vendor, cost 0 (no usage is reported back). The correlation id
/// is the idempotency key, so a retried turn never counts twice.
pub fn vendor_turn_row(vendor: &str, correlation_id: &str, tenant_id: Option<&str>, user_id: &str, latency_ms: u64, ok: bool) -> LedgerRow {
    LedgerRow {
        source: "vendor",
        ctx: LedgerCtx::surface("bot").tier("S2").lane("vendor").tenant(tenant_id, Some(user_id)),
        provider_id: Some("vendor".into()),
        model_id: Some(vendor.to_string()),
        latency_ms: latency_ms as i64,
        status: if ok { "ok" } else { "error" }.into(),
        idempotency_key: Some(format!("vendor-turn:{correlation_id}")),
        ..Default::default()
    }
}

/// Row for one S1 decision: cost 0, tier S1, lane local, counted.
pub fn s1_decision_row(ctx: LedgerCtx, backend: &str, served: bool, latency_ms: u64, incumbent_cost_microdollars: Option<i64>) -> LedgerRow {
    let mut ctx = ctx;
    ctx.tier = Some("S1".into());
    ctx.lane = ctx.lane.or_else(|| Some("local".into()));
    ctx.surface = ctx.surface.or_else(|| Some("s1".into()));
    LedgerRow {
        source: "s1",
        ctx,
        provider_id: Some("s1".into()),
        model_id: Some(backend.to_string()),
        latency_ms: latency_ms as i64,
        status: "ok".into(),
        decision_served_by_s1: served,
        s1_incumbent_cost_microdollars: incumbent_cost_microdollars,
        ..Default::default()
    }
}

// ─── Reported rows (gizzi-code / S1 sidecar → POST /usage/ledger) ──────────

#[derive(Debug, Deserialize)]
pub struct ReportedCall {
    /// `gizzi` (a model call gizzi-code made) or `s1` (an S1 decision).
    #[serde(default)]
    pub kind: Option<String>,
    /// Reporter's own stable id for the call (dedupe on re-delivery).
    pub call_id: String,
    pub session_id: Option<String>,
    pub surface: Option<String>,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub tier: Option<String>,
    pub lane: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub latency_ms: i64,
    #[serde(default)]
    pub served_by_s1: bool,
    pub incumbent_cost_usd: Option<f64>,
}

/// Convert a reported call into a row owned by the authenticated user/org.
/// Cost is client-reported and non-negative; S1 rows are forced to cost 0.
pub fn reported_row(call: &ReportedCall, tenant_id: Option<&str>, user_id: &str) -> Option<LedgerRow> {
    let call_id = id(Some(&call.call_id))?;
    let s1 = call.kind.as_deref() == Some("s1");
    let ctx = LedgerCtx {
        surface: allowed(call.surface.as_deref(), SURFACES),
        run_id: id(call.run_id.as_deref()),
        node_id: id(call.node_id.as_deref()),
        tier: allowed(call.tier.as_deref(), TIERS),
        lane: allowed(call.lane.as_deref(), LANES),
        tenant_id: id(tenant_id),
        user_id: id(Some(user_id)),
    };
    let usd = |v: f64| if v.is_finite() { (v.max(0.0) * 1_000_000.0).round() as i64 } else { 0 };
    let backend = call.model_id.as_deref().unwrap_or("s1");
    let mut row = if s1 {
        s1_decision_row(ctx, backend, call.served_by_s1, call.latency_ms.max(0) as u64, call.incumbent_cost_usd.map(usd))
    } else {
        let mut ctx = ctx;
        ctx.surface = ctx.surface.or_else(|| Some("internal".into()));
        ctx.tier = ctx.tier.or_else(|| Some("S2".into()));
        LedgerRow {
            source: "gizzi",
            ctx,
            provider_id: call.provider_id.clone(),
            model_id: call.model_id.clone(),
            prompt_tokens: call.input_tokens,
            completion_tokens: call.output_tokens,
            reasoning_tokens: call.reasoning_tokens,
            cache_read_tokens: call.cache_read_tokens,
            cache_write_tokens: call.cache_write_tokens,
            cost_microdollars: usd(call.cost_usd),
            latency_ms: call.latency_ms,
            status: "ok".into(),
            gizzi_session_id: id(call.session_id.as_deref()),
            ..Default::default()
        }
    };
    // Namespaced per user so one reporter cannot overwrite another's rows.
    row.idempotency_key = Some(format!("ledger:{user_id}:{}:{call_id}", if s1 { "s1" } else { "gizzi" }));
    Some(row)
}

// ─── Summary (GET /usage/summary?group_by=…) ───────────────────────────────

/// Group-by dimension → SQL expression (whitelist; never interpolate input).
fn dimension(name: &str) -> Option<&'static str> {
    Some(match name {
        "surface" => "COALESCE(surface, 'api')",
        "tier" => "COALESCE(tier, 'S2')",
        "model" => "COALESCE(provider_id, '?') || '/' || COALESCE(model_id, '?')",
        "lane" => "COALESCE(lane, 'api')",
        _ => return None,
    })
}

/// Parse `surface,tier` style group-by lists (1..=4 known dimensions).
pub fn parse_group_by(raw: &str) -> Result<Vec<String>, String> {
    let dims: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if dims.is_empty() || dims.len() > 4 {
        return Err("group_by takes 1 to 4 of: surface, tier, model, lane".into());
    }
    for d in &dims {
        if dimension(d).is_none() {
            return Err(format!("unknown group_by dimension '{d}' (surface, tier, model, lane)"));
        }
    }
    Ok(dims)
}

/// Cache hit rate = cache reads / all input tokens (uncached input + reads +
/// writes). `None` when there was no input.
fn hit_rate(prompt: i64, read: i64, write: i64) -> Option<f64> {
    let all = prompt + read + write;
    (all > 0).then(|| read as f64 / all as f64)
}

/// Ledger summary for one tenant and period: groups by the requested
/// dimensions, plus totals, cache hit rate, and S1 decision counts with
/// estimated savings (incumbent cost of decisions S1 actually served).
/// Cost reads `COALESCE(recomputed_cost_microdollars, cost_microdollars)`.
pub fn summary(conn: &Connection, tenant_id: &str, start: &str, end: &str, group_by: &[String]) -> rusqlite::Result<Value> {
    let exprs: Vec<&str> = group_by.iter().filter_map(|d| dimension(d)).collect();
    let keys = exprs.iter().enumerate().map(|(i, e)| format!("{e} AS k{i}")).collect::<Vec<_>>().join(", ");
    let group = (0..exprs.len()).map(|i| format!("k{i}")).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT {keys}, COUNT(*), SUM(prompt_tokens), SUM(completion_tokens), SUM(reasoning_tokens),
                SUM(cached_tokens), SUM(cache_write_tokens),
                SUM(COALESCE(recomputed_cost_microdollars, cost_microdollars)), SUM(latency_ms)
         FROM llm_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3 AND status != 'in_progress'
         GROUP BY {group} ORDER BY {cost_col} DESC",
        // 1-based position of the cost column: the keys, then 7 aggregates.
        cost_col = exprs.len() + 7
    );
    let mut stmt = conn.prepare(&sql)?;
    let n = exprs.len();
    let mut totals = [0i64; 8];
    let groups = stmt
        .query_map(params![tenant_id, start, end], |r| {
            let mut key = serde_json::Map::new();
            for (i, d) in group_by.iter().enumerate() {
                key.insert(d.clone(), json!(r.get::<_, String>(i)?));
            }
            let v: Vec<i64> = (0..8).map(|j| r.get::<_, Option<i64>>(n + j).map(|x| x.unwrap_or(0))).collect::<Result<_, _>>()?;
            Ok((key, v))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(key, v)| {
            for (t, x) in totals.iter_mut().zip(&v) {
                *t += x;
            }
            json!({
                "key": key, "calls": v[0], "prompt_tokens": v[1], "completion_tokens": v[2],
                "reasoning_tokens": v[3], "cache_read_tokens": v[4], "cache_write_tokens": v[5],
                "cost_microdollars": v[6], "latency_ms_total": v[7],
                "cache_hit_rate": hit_rate(v[1], v[4], v[5]),
            })
        })
        .collect::<Vec<_>>();
    let (decisions, served, savings): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(decision_served_by_s1), 0),
                COALESCE(SUM(CASE WHEN decision_served_by_s1 = 1 THEN s1_incumbent_cost_microdollars END), 0)
         FROM llm_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3 AND tier = 'S1'",
        params![tenant_id, start, end],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(json!({
        "group_by": group_by,
        "groups": groups,
        "totals": {
            "calls": totals[0], "prompt_tokens": totals[1], "completion_tokens": totals[2],
            "reasoning_tokens": totals[3], "cache_read_tokens": totals[4], "cache_write_tokens": totals[5],
            "cost_microdollars": totals[6], "latency_ms_total": totals[7],
        },
        "cache_hit_rate": hit_rate(totals[1], totals[4], totals[5]),
        "s1": {
            "decisions": decisions,
            "served_by_s1": served,
            "estimated_savings_microdollars": savings,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (DbHandle, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbHandle::new(dir.path().join("ledger.db")).unwrap();
        (db, dir)
    }

    fn row_of(conn: &Connection, id: &str) -> (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64) {
        conn.query_row(
            "SELECT source, surface, run_id, node_id, tier, lane, cache_write_tokens, cost_microdollars FROM llm_usage_events WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn internal_rows_take_the_task_scope_then_default_to_internal() {
        let ctx = LedgerCtx::surface("memory").run("run_1", Some("N3")).tier("S2").tenant(Some("org-1"), Some("u1"));
        let row = scope(ctx, async { internal_row("p", "m", Some("ses_1"), 10, 4, 6, 2, 0.25, 30, true) }).await;
        assert_eq!(row.ctx.surface.as_deref(), Some("memory"));
        assert_eq!((row.ctx.run_id.as_deref(), row.ctx.node_id.as_deref()), (Some("run_1"), Some("N3")));
        assert_eq!(row.ctx.tenant_id.as_deref(), Some("org-1"));
        assert_eq!((row.cost_microdollars, row.cache_read_tokens, row.cache_write_tokens), (250_000, 6, 2));
        let bare = internal_row("p", "m", None, 1, 1, 0, 0, 0.0, 1, false);
        assert_eq!((bare.ctx.surface.as_deref(), bare.ctx.tier.as_deref(), bare.ctx.lane.as_deref()), (Some("internal"), Some("S2"), Some("api")));
        assert_eq!(bare.status, "error");
    }

    #[test]
    fn thread_scope_covers_block_on_callers_and_restores() {
        assert!(current().is_none());
        {
            let _outer = enter(LedgerCtx::surface("agency").run("run_9", Some("N11")));
            {
                let _inner = enter(LedgerCtx::surface("template"));
                assert_eq!(current().unwrap().surface.as_deref(), Some("template"));
            }
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let seen = rt.block_on(async { internal_row("p", "m", None, 0, 0, 0, 0, 0.0, 0, true) });
            assert_eq!(seen.ctx.surface.as_deref(), Some("agency"));
            assert_eq!(seen.ctx.node_id.as_deref(), Some("N11"));
        }
        assert!(current().is_none());
    }

    #[test]
    fn unknown_values_are_dropped_not_stored() {
        let ctx = LedgerCtx::surface("nope").tier("S9").lane("cloud");
        assert_eq!((ctx.surface, ctx.tier, ctx.lane), (None, None, None));
    }

    #[test]
    fn internal_and_s1_rows_persist_with_ledger_keys() {
        let (db, _d) = db();
        let conn = db.connect().unwrap();
        let ctx = LedgerCtx::surface("agency").run("run_1", Some("N11")).tier("S2").tenant(Some("org-1"), None);
        let id = insert(&conn, &LedgerRow { source: "internal", ctx: ctx.clone(), cache_write_tokens: 5, cost_microdollars: 7, status: "ok".into(), ..Default::default() })
            .unwrap()
            .unwrap();
        let r = row_of(&conn, &id);
        assert_eq!((r.0.as_str(), r.1.as_deref(), r.2.as_deref(), r.3.as_deref(), r.4.as_deref()), ("internal", Some("agency"), Some("run_1"), Some("N11"), Some("S2")));
        assert_eq!((r.6, r.7), (5, 7));
        let s1 = insert(&conn, &s1_decision_row(ctx, "laya_bundled", false, 12, None)).unwrap().unwrap();
        let r = row_of(&conn, &s1);
        assert_eq!((r.0.as_str(), r.4.as_deref(), r.5.as_deref(), r.7), ("s1", Some("S1"), Some("local"), 0));
    }

    #[test]
    fn vendor_turns_are_counted_once_at_zero_cost() {
        let (db, _d) = db();
        let conn = db.connect().unwrap();
        let id = insert(&conn, &vendor_turn_row("claude-code", "corr_1", Some("org-1"), "u1", 40, true)).unwrap().unwrap();
        let r = row_of(&conn, &id);
        assert_eq!((r.0.as_str(), r.1.as_deref(), r.4.as_deref(), r.5.as_deref(), r.7), ("vendor", Some("bot"), Some("S2"), Some("vendor"), 0));
        // A retried turn (same correlation id) updates the row, never doubles it.
        insert(&conn, &vendor_turn_row("claude-code", "corr_1", Some("org-1"), "u1", 55, false)).unwrap();
        let (n, status): (i64, String) = conn
            .query_row("SELECT COUNT(*), MAX(status) FROM llm_usage_events WHERE source = 'vendor'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((n, status.as_str()), (1, "error"));
    }

    #[test]
    fn gizzi_reports_dedupe_against_metered_sessions_in_both_orders() {
        let (db, _d) = db();
        let conn = db.connect().unwrap();
        let call = |sid: &str, call_id: &str| ReportedCall {
            kind: None, call_id: call_id.into(), session_id: Some(sid.into()), surface: Some("chat".into()), run_id: None,
            node_id: None, tier: None, lane: Some("subscription-cli".into()), provider_id: Some("p".into()), model_id: Some("m".into()),
            input_tokens: 10, output_tokens: 5, reasoning_tokens: 0, cache_read_tokens: 30, cache_write_tokens: 0, cost_usd: 0.01,
            latency_ms: 9, served_by_s1: false, incumbent_cost_usd: None,
        };
        let count = |c: &Connection| -> i64 { c.query_row("SELECT COUNT(*) FROM llm_usage_events", [], |r| r.get(0)).unwrap() };
        // Standalone gizzi call: recorded; a re-delivery updates, not doubles.
        let row = reported_row(&call("ses_a", "c1"), Some("org-1"), "u1").unwrap();
        assert!(insert(&conn, &row).unwrap().is_some());
        insert(&conn, &row).unwrap();
        assert_eq!(count(&conn), 1);
        // allternit-api then meters the same session → the gizzi row goes.
        insert(&conn, &internal_row("p", "m", Some("ses_a"), 10, 5, 30, 0, 0.01, 9, true)).unwrap();
        assert_eq!(conn.query_row("SELECT source FROM llm_usage_events", [], |r| r.get::<_, String>(0)).unwrap(), "internal");
        // A late gizzi report for a metered session is skipped.
        let late = reported_row(&call("ses_a", "c2"), Some("org-1"), "u1").unwrap();
        assert!(insert(&conn, &late).unwrap().is_none());
        assert_eq!(count(&conn), 1);
    }

    #[test]
    fn s1_reports_are_cost_zero_and_keys_are_namespaced_per_user() {
        let call = ReportedCall {
            kind: Some("s1".into()), call_id: "d1".into(), session_id: None, surface: Some("chat".into()), run_id: None, node_id: None,
            tier: Some("S3".into()), lane: None, provider_id: None, model_id: Some("laya_bundled".into()), input_tokens: 99,
            output_tokens: 99, reasoning_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 5.0, latency_ms: 4,
            served_by_s1: true, incumbent_cost_usd: Some(0.002),
        };
        let row = reported_row(&call, None, "u1").unwrap();
        assert_eq!((row.source, row.cost_microdollars, row.prompt_tokens), ("s1", 0, 0));
        assert_eq!(row.ctx.tier.as_deref(), Some("S1"));
        assert_eq!(row.s1_incumbent_cost_microdollars, Some(2_000));
        assert!(row.decision_served_by_s1);
        assert_eq!(row.idempotency_key.as_deref(), Some("ledger:u1:s1:d1"));
        assert_ne!(reported_row(&call, None, "u2").unwrap().idempotency_key, row.idempotency_key);
        assert!(reported_row(&ReportedCall { call_id: " ".into(), ..call }, None, "u1").is_none());
    }

    fn seed(conn: &Connection) {
        let ctx = |s: &str, t: &str, l: &str| LedgerCtx::surface(s).tier(t).lane(l).tenant(Some("org-1"), None);
        let llm = |ctx: LedgerCtx, model: &str, prompt: i64, read: i64, cost: i64| LedgerRow {
            source: "internal", ctx, provider_id: Some("prov".into()), model_id: Some(model.into()), prompt_tokens: prompt,
            completion_tokens: 1, cache_read_tokens: read, cost_microdollars: cost, status: "ok".into(), ..Default::default()
        };
        insert(conn, &llm(ctx("chat", "S2", "api"), "big", 100, 300, 1_000)).unwrap();
        insert(conn, &llm(ctx("chat", "S2", "subscription-cli"), "big", 100, 0, 500)).unwrap();
        insert(conn, &llm(ctx("memory", "S2", "api"), "small", 50, 50, 100)).unwrap();
        insert(conn, &s1_decision_row(ctx("agency", "S1", "local"), "laya", true, 5, Some(400))).unwrap();
        insert(conn, &s1_decision_row(ctx("agency", "S1", "local"), "laya", false, 5, Some(999))).unwrap();
        // Another tenant: never counted.
        insert(conn, &llm(LedgerCtx::surface("chat").tenant(Some("org-2"), None), "big", 1, 0, 9_999)).unwrap();
    }

    #[test]
    fn summary_groups_by_each_dimension_with_hit_rate_and_s1_savings() {
        let (db, _d) = db();
        let conn = db.connect().unwrap();
        seed(&conn);
        let (s, e) = ("2000-01-01", "2999-01-01");
        let by = |d: &str| summary(&conn, "org-1", s, e, &parse_group_by(d).unwrap()).unwrap();

        let surface = by("surface");
        let groups = surface["groups"].as_array().unwrap();
        assert_eq!(groups[0]["key"]["surface"], "chat");
        assert_eq!((groups[0]["calls"].as_i64(), groups[0]["cost_microdollars"].as_i64()), (Some(2), Some(1_500)));
        assert_eq!(surface["totals"]["cost_microdollars"], 1_600);
        assert_eq!(surface["totals"]["calls"], 5);
        // reads 350 / (prompt 250 + reads 350)
        let rate = surface["cache_hit_rate"].as_f64().unwrap();
        assert!((rate - 350.0 / 600.0).abs() < 1e-9, "{rate}");
        assert_eq!(surface["s1"]["decisions"], 2);
        assert_eq!(surface["s1"]["served_by_s1"], 1);
        assert_eq!(surface["s1"]["estimated_savings_microdollars"], 400);

        let tier_lane = by("tier,lane");
        let keys: Vec<_> = tier_lane["groups"].as_array().unwrap().iter().map(|g| g["key"].clone()).collect();
        assert!(keys.contains(&serde_json::json!({"tier": "S1", "lane": "local"})));
        assert!(keys.contains(&serde_json::json!({"tier": "S2", "lane": "subscription-cli"})));
        let model = by("model");
        assert_eq!(model["groups"][0]["key"]["model"], "prov/big");
        assert!(parse_group_by("surface,secret").is_err());
        assert!(parse_group_by("").is_err());
    }
}
