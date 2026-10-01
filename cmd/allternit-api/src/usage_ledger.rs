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
//! - S1 decisions (`source = s1`, `tier = S1`, cost 0 but counted).
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
pub const LANES: &[&str] = &["api", "subscription-cli", "local"];

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
