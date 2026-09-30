//! Spending guard for the Agency executor (Q11, server side).
//!
//! Everything here is read from the environment each time a run is admitted,
//! so an operator can tighten a cap with a restart and nothing else:
//!
//! * `ALLTERNIT_AGENCY_EXECUTE_ORGS`: comma-separated org ids allowed to
//!   execute. Empty or unset means NO org may execute, even with
//!   `ALLTERNIT_AGENCY_EXECUTE=1`. `*` allows every org (dev only).
//! * `ALLTERNIT_AGENCY_DAILY_BUDGET`: global per-UTC-day cap across all runs.
//! * `ALLTERNIT_AGENCY_ORG_DAILY_BUDGET`: the same cap, per org.
//!   Both take `tokens=<n>,usd=<x>` (either part may be left out; a bare
//!   number means USD; `off` removes the cap). Checked before every effect and
//!   model call; when either is reached the run parks with the attention
//!   request "budget cap reached" and spend stops.
//! * `ALLTERNIT_AGENCY_MAX_CONCURRENT` / `ALLTERNIT_AGENCY_ORG_MAX_CONCURRENT`:
//!   executing runs at once, globally and per org. Extra runs stay `waiting`
//!   and start when a slot frees.
//!
//! The org of a run is the creator's organization id, else tenant id, else
//! `user:<user id>` (recorded in the TaskIR as `org_id` at creation).

use serde_json::Value;
use std::collections::HashMap;

pub const ORGS_ENV: &str = "ALLTERNIT_AGENCY_EXECUTE_ORGS";
pub const DAILY_ENV: &str = "ALLTERNIT_AGENCY_DAILY_BUDGET";
pub const ORG_DAILY_ENV: &str = "ALLTERNIT_AGENCY_ORG_DAILY_BUDGET";
pub const MAX_CONC_ENV: &str = "ALLTERNIT_AGENCY_MAX_CONCURRENT";
pub const ORG_MAX_CONC_ENV: &str = "ALLTERNIT_AGENCY_ORG_MAX_CONCURRENT";

pub const DEFAULT_DAILY: Cap = Cap { tokens: Some(2_000_000), usd: Some(20.0) };
pub const DEFAULT_ORG_DAILY: Cap = Cap { tokens: Some(500_000), usd: Some(5.0) };
pub const DEFAULT_MAX_CONCURRENT: usize = 2;
pub const DEFAULT_ORG_MAX_CONCURRENT: usize = 1;

pub const CAP_REASON: &str = "budget_cap_reached";
pub const CAP_TITLE: &str = "budget cap reached";
/// Agent rules `approvals.spend_over_usd`: attention raised once per run.
pub const SPEND_REASON: &str = "spend_over_usd";
pub const SPEND_TITLE: &str = "spend threshold reached";

/// A daily cap. `None` in a dimension means that dimension is uncapped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cap {
    pub tokens: Option<u64>,
    pub usd: Option<f64>,
}

impl Cap {
    pub const OFF: Cap = Cap { tokens: None, usd: None };

    /// `tokens=200000,usd=5` | `5` (USD) | `off`. Unparseable input fails
    /// closed to a zero cap, never to "uncapped".
    pub fn parse(s: &str) -> Cap {
        let s = s.trim();
        if s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("unlimited") {
            return Cap::OFF;
        }
        if let Ok(usd) = s.parse::<f64>() {
            return Cap { tokens: None, usd: Some(usd.max(0.0)) };
        }
        let mut cap = Cap::OFF;
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("tokens", v)) => match v.parse::<u64>() {
                    Ok(n) => cap.tokens = Some(n),
                    Err(_) => return Cap { tokens: Some(0), usd: Some(0.0) },
                },
                Some(("usd", v)) => match v.parse::<f64>() {
                    Ok(x) => cap.usd = Some(x.max(0.0)),
                    Err(_) => return Cap { tokens: Some(0), usd: Some(0.0) },
                },
                _ => return Cap { tokens: Some(0), usd: Some(0.0) },
            }
        }
        cap
    }

    /// The dimension that is reached (`spent >= cap`), if any.
    pub fn reached(&self, spent: &Spend) -> Option<&'static str> {
        if self.tokens.is_some_and(|t| spent.tokens >= t) {
            return Some("tokens");
        }
        if self.usd.is_some_and(|u| spent.usd >= u) {
            return Some("usd");
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spend {
    pub tokens: u64,
    pub usd: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// `None` = every org (only `*`); empty = none.
    pub orgs: Option<Vec<String>>,
    pub daily: Cap,
    pub org_daily: Cap,
    pub max_concurrent: usize,
    pub org_max_concurrent: usize,
}

impl Limits {
    /// No allowlist, caps or concurrency limits: tests that call
    /// `executor::start` directly.
    pub fn unlimited() -> Self {
        Limits { orgs: None, daily: Cap::OFF, org_daily: Cap::OFF, max_concurrent: usize::MAX, org_max_concurrent: usize::MAX }
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let orgs_raw = get(ORGS_ENV).unwrap_or_default();
        let orgs: Vec<String> = orgs_raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        let conc = |k: &str, d: usize| get(k).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(d);
        Limits {
            orgs: if orgs.iter().any(|o| o == "*") { None } else { Some(orgs) },
            daily: get(DAILY_ENV).map(|v| Cap::parse(&v)).unwrap_or(DEFAULT_DAILY),
            org_daily: get(ORG_DAILY_ENV).map(|v| Cap::parse(&v)).unwrap_or(DEFAULT_ORG_DAILY),
            max_concurrent: conc(MAX_CONC_ENV, DEFAULT_MAX_CONCURRENT),
            org_max_concurrent: conc(ORG_MAX_CONC_ENV, DEFAULT_ORG_MAX_CONCURRENT),
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Tighten with a run's `task_ir.rules` (Agent rules): the stricter of the
    /// env ceiling and the rule wins; a rule never raises a limit.
    pub fn tightened(&self, rules: &Value) -> Limits {
        let mut l = self.clone();
        if let Some(u) = rules["daily_usd"].as_f64().filter(|u| *u >= 0.0) {
            l.org_daily.usd = Some(l.org_daily.usd.map_or(u, |c| c.min(u)));
        }
        if let Some(n) = rules["max_concurrent"].as_f64().filter(|n| *n >= 0.0) {
            l.org_max_concurrent = l.org_max_concurrent.min((n as usize).max(1));
        }
        l
    }

    pub fn org_allowed(&self, org: &str) -> bool {
        match &self.orgs {
            None => true,
            Some(list) => !org.is_empty() && list.iter().any(|o| o == org),
        }
    }

    /// May one more run of `org` start, given the runs executing now
    /// (run id → org)?
    pub fn admits(&self, active: &HashMap<String, String>, org: &str) -> bool {
        active.len() < self.max_concurrent && active.values().filter(|o| *o == org).count() < self.org_max_concurrent
    }

    /// Which daily cap is reached: `Some(("global"|"org", dimension))`.
    pub fn daily_reached(&self, global: &Spend, org: &Spend) -> Option<(&'static str, &'static str)> {
        self.daily.reached(global).map(|d| ("global", d)).or_else(|| self.org_daily.reached(org).map(|d| ("org", d)))
    }
}

/// The org a run is billed to (see module docs).
pub fn org_of(user: &crate::auth::AuthUser) -> String {
    user.organization_id.clone().filter(|s| !s.is_empty())
        .or_else(|| user.tenant_id.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| format!("user:{}", user.user_id))
}

pub fn run_org(task_ir: &Value) -> String {
    task_ir["org_id"].as_str().unwrap_or_default().to_string()
}

/// UTC day key (`YYYY-MM-DD`) of an RFC 3339 timestamp.
pub fn day_of(ts: &str) -> &str {
    ts.get(..10).unwrap_or_default()
}

pub fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Sum today's spend (runs created today, UTC) globally and for `org`.
pub fn spend_today<'a>(runs: impl IntoIterator<Item = (&'a Value, &'a Value)>, org: &str) -> (Spend, Spend) {
    let day = today();
    let (mut g, mut o) = (Spend::default(), Spend::default());
    for (run, task_ir) in runs {
        if day_of(run["created_at"].as_str().unwrap_or_default()) != day {
            continue;
        }
        let u = &run["budget_usage"];
        let s = Spend { tokens: u["tokens"].as_u64().unwrap_or(0), usd: u["cost_usd"].as_f64().unwrap_or(0.0) };
        g.tokens += s.tokens;
        g.usd += s.usd;
        if run_org(task_ir) == org {
            o.tokens += s.tokens;
            o.usd += s.usd;
        }
    }
    (g, o)
}
