//! `GET /v1/descent`: is work moving down the tiers? (Agency Descent, Phase 1)
//!
//! Over the caller's decisions in a window, per decision kind: how many were
//! answered by each tier (`head` trained/lookup heads, `local` scorer,
//! `open` open-weight decision model, `vendor`, `oracle` planner model), the
//! cheap share (head + local + open), how often the router escalated, the
//! mean latency and the oracle's spend. `daily` shows the cheap share per
//! day, the number that should rise as heads are trained and promoted.

use super::super::{ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Extension, Json,
};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const TIERS: &[&str] = &["head", "local", "open", "vendor", "oracle", "none"];
const CHEAP: &[&str] = &["head", "local", "open"];

/// The tier a serving backend belongs to.
pub fn tier(backend: &str) -> &'static str {
    match backend {
        b if b.starts_with("head") => "head",
        "local" => "local",
        "open" => "open",
        "vendor" | "typesafe" => "vendor",
        "oracle" => "oracle",
        _ => "none",
    }
}

#[derive(Default)]
struct Acc {
    n: u64,
    tiers: BTreeMap<&'static str, u64>,
    escalated: u64,
    latency_ms: f64,
    oracle_cost_usd: f64,
}

impl Acc {
    fn add(&mut self, backend: &str, latency_ms: f64, attempts: &Value) {
        self.n += 1;
        *self.tiers.entry(tier(backend)).or_default() += 1;
        self.latency_ms += latency_ms;
        let tried: Vec<&Value> = attempts.as_array().map(|a| a.iter().filter(|x| x.get("skipped").is_none()).collect()).unwrap_or_default();
        if tried.len() > 1 {
            self.escalated += 1;
        }
        self.oracle_cost_usd += tried
            .iter()
            .filter(|x| x["backend"] == "oracle")
            .filter_map(|x| x["detail"]["cost_usd"].as_f64())
            .sum::<f64>();
    }
    fn share(&self, tiers: &[&str]) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        round4(tiers.iter().map(|t| self.tiers.get(t).copied().unwrap_or(0)).sum::<u64>() as f64 / self.n as f64)
    }
    fn json(&self) -> Value {
        let tiers: serde_json::Map<String, Value> = TIERS.iter().map(|t| (t.to_string(), json!(self.tiers.get(t).copied().unwrap_or(0)))).collect();
        json!({
            "decisions": self.n,
            "tiers": tiers,
            "cheap_share": self.share(CHEAP),
            "oracle_share": self.share(&["oracle"]),
            "escalation_rate": if self.n == 0 { 0.0 } else { round4(self.escalated as f64 / self.n as f64) },
            "avg_latency_ms": if self.n == 0 { 0.0 } else { (self.latency_ms / self.n as f64 * 100.0).round() / 100.0 },
            "oracle_cost_usd": (self.oracle_cost_usd * 1e6).round() / 1e6,
        })
    }
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// The descent report for `owner` over decisions created at or after `since`.
pub fn report(conn: &Connection, owner: &str, since: &str) -> rusqlite::Result<Value> {
    let mut stmt = conn.prepare(
        "SELECT kind, backend, latency_ms, attempts_json, substr(created_at, 1, 10)
         FROM decisions WHERE owner = ?1 AND created_at >= ?2 ORDER BY created_at",
    )?;
    let mut total = Acc::default();
    let mut kinds: BTreeMap<String, Acc> = BTreeMap::new();
    let mut days: BTreeMap<String, Acc> = BTreeMap::new();
    let rows = stmt.query_map(params![owner, since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
    })?;
    for row in rows {
        let (kind, backend, latency, attempts, day) = row?;
        let attempts: Value = serde_json::from_str(&attempts).unwrap_or(Value::Null);
        total.add(&backend, latency, &attempts);
        kinds.entry(kind).or_default().add(&backend, latency, &attempts);
        days.entry(day).or_default().add(&backend, latency, &attempts);
    }
    Ok(json!({
        "object": "descent",
        "since": since,
        "total": total.json(),
        "kinds": kinds.iter().map(|(k, a)| { let mut v = a.json(); v["kind"] = json!(k); v }).collect::<Vec<_>>(),
        "daily": days.iter().map(|(d, a)| json!({ "date": d, "decisions": a.n, "cheap_share": a.share(CHEAP) })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
pub struct DescentQuery {
    #[serde(default)]
    days: Option<i64>,
}

pub async fn get_descent(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Query(q): Query<DescentQuery>,
) -> ApiResult {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    let mut v = report(&conn, &user.user_id, &since).map_err(|e| ApiError::internal(e, &rid))?;
    v["window_days"] = json!(days);
    Ok(Json(v).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    #[test]
    fn tiers_shares_escalations_and_daily_trend() {
        let db = DbHandle::new(std::env::temp_dir().join(format!("allternit-descent-{}.db", uuid::Uuid::new_v4()))).unwrap();
        let conn = db.connect().unwrap();
        let add = |id: &str, owner: &str, day: &str, kind: &str, backend: &str, attempts: Value| {
            conn.execute(
                "INSERT INTO decisions (id, owner, created_at, kind, context_hash, options_json, probs_json, confidence, backend, latency_ms, attempts_json)
                 VALUES (?1, ?2, ?3, ?4, 'h', '[]', '{}', 0.9, ?5, 10.0, ?6)",
                params![id, owner, format!("{day}T12:00:00.000Z"), kind, backend, attempts.to_string()],
            )
            .unwrap();
        };
        let one = |b: &str| json!([{ "backend": b }]);
        // Day 1: the oracle does most of the work (one escalation from local).
        add("1", "u", "2026-10-01", "route", "oracle", json!([{ "backend": "local" }, { "backend": "oracle", "detail": { "cost_usd": 0.002 } }]));
        add("2", "u", "2026-10-01", "route", "oracle", json!([{ "backend": "oracle", "detail": { "cost_usd": 0.003 } }]));
        add("3", "u", "2026-10-01", "verify", "local", one("local"));
        // Day 2: heads and the open model take over.
        add("4", "u", "2026-10-02", "route", "head.lookup", one("head.lookup"));
        add("5", "u", "2026-10-02", "route", "open", json!([{ "backend": "head.lookup", "skipped": "x" }, { "backend": "open" }]));
        // Another owner, and an old row, stay out.
        add("6", "other", "2026-10-02", "route", "oracle", one("oracle"));
        add("7", "u", "2026-09-01", "route", "oracle", one("oracle"));

        let r = report(&conn, "u", "2026-09-30T00:00:00.000Z").unwrap();
        assert_eq!(r["total"]["decisions"], 5);
        assert_eq!(r["total"]["tiers"]["oracle"], 2);
        assert_eq!(r["total"]["tiers"]["head"], 1);
        assert_eq!(r["total"]["cheap_share"], 0.6);
        assert_eq!(r["total"]["escalation_rate"], 0.2);
        assert_eq!(r["total"]["oracle_cost_usd"], 0.005);
        let route = r["kinds"].as_array().unwrap().iter().find(|k| k["kind"] == "route").unwrap();
        assert_eq!((route["decisions"].as_u64(), route["oracle_share"].as_f64()), (Some(4), Some(0.5)));
        let daily: Vec<(String, f64)> = r["daily"].as_array().unwrap().iter()
            .map(|d| (d["date"].as_str().unwrap().to_string(), d["cheap_share"].as_f64().unwrap())).collect();
        assert_eq!(daily, vec![("2026-10-01".into(), 0.3333), ("2026-10-02".into(), 1.0)]);
    }
}
