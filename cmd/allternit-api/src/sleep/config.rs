//! Sleep settings and thresholds (D9, D15).
//!
//! Every switch and threshold has a code default here. `sleep_config` rows
//! overlay it: the global row (`user_id = ''`) first, then the user's own row.
//! A row's `source` says who set it: `default`, `fitted` (the fitter, step 3.4)
//! or `owner` (the user, through `PUT /memory/sleep/settings` or `/config`).
//! Writers of `fitted` rows must not replace an `owner` row.
//!
//! Every stage flag is off by default and the daily spend cap is 0, so nothing
//! runs and nothing is spent until a stage is built and switched on.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use super::types::Stage;

/// Master switch.
pub const KEY_ENABLED: &str = "enabled";
/// Daily spend cap in USD on the user's own quota. 0 = no spend.
pub const KEY_DAILY_SPEND_CAP: &str = "daily_spend_cap_usd";

/// The `sleep_config` key for a stage flag.
pub fn stage_key(stage: Stage) -> String {
    format!("stage.{}", stage.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DefaultValue {
    Bool(bool),
    Num(f64),
    Int(i64),
}

impl DefaultValue {
    pub fn to_json(self) -> Value {
        match self {
            DefaultValue::Bool(b) => Value::Bool(b),
            DefaultValue::Num(n) => serde_json::json!(n),
            DefaultValue::Int(n) => serde_json::json!(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// Switches and the spend cap: `/memory/sleep/settings`.
    Setting,
    /// Tuning numbers: `/memory/sleep/config`.
    Threshold,
}

/// One known key.
#[derive(Debug, Clone, Copy)]
pub struct Def {
    pub key: &'static str,
    pub default: DefaultValue,
    pub min: f64,
    pub max: f64,
    pub group: Group,
    pub about: &'static str,
}

const fn flag(key: &'static str, about: &'static str) -> Def {
    Def { key, default: DefaultValue::Bool(false), min: 0.0, max: 1.0, group: Group::Setting, about }
}

const fn num(key: &'static str, default: f64, min: f64, max: f64, about: &'static str) -> Def {
    Def { key, default: DefaultValue::Num(default), min, max, group: Group::Threshold, about }
}

const fn int(key: &'static str, default: i64, min: f64, max: f64, about: &'static str) -> Def {
    Def { key, default: DefaultValue::Int(default), min, max, group: Group::Threshold, about }
}

/// Every key Sleep reads. Stage keys must match `Stage::ALL`.
pub const DEFS: &[Def] = &[
    flag(KEY_ENABLED, "Master switch for Sleep."),
    flag("stage.labels_live", "Answer-used labels on the incumbent recall (3.1)."),
    flag("stage.strengths", "Storage and retrieval strengths (3.2)."),
    flag("stage.pages", "Topic pages, Track 2 (4.1-4.3)."),
    flag("stage.router", "Router out of shadow, context and load_page (4.4)."),
    flag("stage.orchestrator", "Sleep cycles from the unit queue (5.1)."),
    flag("stage.disputes", "Dispute records and the arbiter (6.1)."),
    flag("stage.checks", "Spaced checks on old memories (6.3)."),
    flag("stage.intentions", "Prospective memory (6.4)."),
    flag("stage.rem", "REM and the hypothesis queue (5.4)."),
    flag("stage.track1", "Track 1 adapters (Phase 7, needs migration B)."),
    Def {
        key: KEY_DAILY_SPEND_CAP,
        default: DefaultValue::Num(0.0),
        min: 0.0,
        max: 1000.0,
        group: Group::Setting,
        about: "Most Sleep may spend per day on your own quota, in USD. 0 = none.",
    },
    int("idle_ttl_s", 300, 60.0, 86_400.0, "Idle seconds before a nap; the runtime sends its real cache TTL."),
    num("nap_after_ttl_multiple", 1.0, 0.5, 100.0, "An idle signal starts a nap once the session has been idle this many times the cache TTL; below that it starts a daydream."),
    int("unit_max_attempts", 3, 1.0, 10.0, "Attempts before a unit moves to the next cycle."),
    int("unit_lease_s", 900, 30.0, 86_400.0, "Lease length for a running unit."),
    int("worker_poll_s", 30, 5.0, 3600.0, "Seconds between Sleep worker checks of the unit queue (global row only)."),
    int("cycle_max_stages", 8, 1.0, 50.0, "Per-cycle stop rule: most stages one cycle runs; the rest waits for the next signal."),
    int("cycle_max_minutes", 30, 1.0, 1440.0, "Per-cycle stop rule: no new stage starts after this many minutes."),
    num("dream_cost_estimate_usd", 0.05, 0.0, 100.0, "Spend charged against the daily cap for one Dream run."),
    num("curation_cost_estimate_usd", 0.05, 0.0, 100.0, "Spend charged against the daily cap for one weekly curation run."),
    num("debt_scale", 50.0, 1.0, 100_000.0, "New memories since the last finished nap that make 1.0 of sleep debt."),
    num("debt_pause_threshold", 1.0, 0.0, 1000.0, "At or above this sleep debt, memory decay pauses until a nap finishes."),
    num("page_budget_share", 0.05, 0.0, 0.5, "Share of the model's context window for pages per message."),
    num("router_min_confidence", 0.5, 0.0, 1.0, "Below this the router falls back to embedding top-k."),
    num("pull_cost_multiple_max", 2.0, 1.0, 100.0, "Gate G2: a mid-session pull may cost at most this multiple of preload."),
    num("dispute_ask_margin", 0.15, 0.0, 1.0, "Arbiter margin under which a high-stakes dispute asks the user."),
    num("dispute_ask_stakes", 0.7, 0.0, 1.0, "Stakes at or above which a low-margin dispute asks the user."),
    num("rem_min_keep_rate", 0.25, 0.0, 1.0, "REM turns itself off under this keep rate."),
    int("fit_min_labeled_events", 200, 1.0, 1_000_000.0, "Labelled events needed before fitted values replace defaults."),
    int("check_router_sample", 200, 0.0, 100_000.0, "Fixed router sample size in the bounded check."),
    int("check_neighbours", 5, 0.0, 100.0, "Neighbours checked per changed memory."),
    num("strength_half_life_days", 30.0, 1.0, 3650.0, "Days for retrieval strength to halve at storage strength 0; well-learned memories fade slower."),
    num("strength_spacing_credit", 0.2, 0.0, 5.0, "Storage strength a fully spaced recall earns."),
    num("strength_difficulty_weight", 1.0, 0.0, 10.0, "Extra credit for a recall made while retrieval strength was low."),
    num("strength_min_spacing_days", 1.0, 0.01, 365.0, "Days since the last recall that earn full spacing credit."),
    num("strength_instant_credit_cap", 0.02, 0.0, 1.0, "Most a recall under the minimum spacing can earn."),
    num("credence_user_said", 0.9, 0.0, 1.0, "Write-time credence of a fact the user said."),
    num("credence_tool_observed", 0.8, 0.0, 1.0, "Write-time credence of a fact a tool reported."),
    num("credence_imported", 0.6, 0.0, 1.0, "Write-time credence of an imported fact."),
    num("credence_model_inferred", 0.5, 0.0, 1.0, "Write-time credence of a fact the model inferred."),
    num("credence_dream_generated", 0.3, 0.0, 1.0, "Write-time credence of a fact a dream produced."),
    num("attention_base", 0.1, 0.0, 1.0, "Attention every new fact starts with."),
    num("attention_w_remember", 0.35, 0.0, 1.0, "Attention added when the user flagged it (remember this)."),
    num("attention_w_self", 0.2, 0.0, 1.0, "Attention added for a first-person statement about the user."),
    num("attention_w_repeat", 0.15, 0.0, 1.0, "Attention added when a near-duplicate is already stored."),
    num("attention_w_surprise", 0.2, 0.0, 1.0, "Attention added when the fact updates an existing one."),
    num("attention_w_stakes", 0.2, 0.0, 1.0, "Attention added for money, health, legal or deadline facts."),
    num("attention_repeat_jaccard", 0.6, 0.0, 1.0, "Word overlap at or above which two facts count as a repeat."),
];

pub fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Default,
    Fitted,
    Owner,
}

impl Source {
    fn parse(s: &str) -> Option<Source> {
        match s {
            "default" => Some(Source::Default),
            "fitted" => Some(Source::Fitted),
            "owner" => Some(Source::Owner),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub value: Value,
    pub source: Source,
    pub default: Value,
    pub updated_at: Option<String>,
    pub group: Group,
    pub about: &'static str,
}

/// The resolved settings and thresholds for one user.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub entries: BTreeMap<&'static str, Entry>,
}

impl Resolved {
    pub fn bool(&self, key: &str) -> bool {
        self.entries.get(key).and_then(|e| e.value.as_bool()).unwrap_or(false)
    }

    pub fn number(&self, key: &str) -> f64 {
        self.entries.get(key).and_then(|e| e.value.as_f64()).unwrap_or(0.0)
    }

    pub fn enabled(&self) -> bool {
        self.bool(KEY_ENABLED)
    }

    /// A stage runs only when the master switch and its own flag are both on.
    pub fn stage_on(&self, stage: Stage) -> bool {
        self.enabled() && self.bool(&stage_key(stage))
    }

    /// Each stage's own flag (not combined with the master switch).
    pub fn stage_flags(&self) -> BTreeMap<String, bool> {
        Stage::ALL.iter().map(|s| (s.as_str().to_string(), self.bool(&stage_key(*s)))).collect()
    }

    pub fn daily_spend_cap_usd(&self) -> f64 {
        self.number(KEY_DAILY_SPEND_CAP)
    }

    pub fn group(&self, group: Group) -> BTreeMap<&'static str, Entry> {
        self.entries.iter().filter(|(_, e)| e.group == group).map(|(k, e)| (*k, e.clone())).collect()
    }
}

#[derive(Debug)]
pub enum ConfigError {
    UnknownKey(String),
    WrongGroup(String),
    BadValue { key: String, why: String },
    Db(rusqlite::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::UnknownKey(k) => write!(f, "unknown key: {k}"),
            ConfigError::WrongGroup(k) => write!(f, "{k} is not set through this route"),
            ConfigError::BadValue { key, why } => write!(f, "bad value for {key}: {why}"),
            ConfigError::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl From<rusqlite::Error> for ConfigError {
    fn from(e: rusqlite::Error) -> Self {
        ConfigError::Db(e)
    }
}

/// Checks a value against its key's kind and range.
pub fn validate(d: &Def, value: &Value) -> Result<Value, ConfigError> {
    let bad = |why: &str| ConfigError::BadValue { key: d.key.to_string(), why: why.to_string() };
    match d.default {
        DefaultValue::Bool(_) => value.as_bool().map(Value::Bool).ok_or_else(|| bad("expected true or false")),
        DefaultValue::Num(_) | DefaultValue::Int(_) => {
            let n = value.as_f64().ok_or_else(|| bad("expected a number"))?;
            if !n.is_finite() || n < d.min || n > d.max {
                return Err(bad(&format!("must be between {} and {}", d.min, d.max)));
            }
            if matches!(d.default, DefaultValue::Int(_)) {
                if n.fract() != 0.0 {
                    return Err(bad("expected a whole number"));
                }
                return Ok(serde_json::json!(n as i64));
            }
            Ok(serde_json::json!(n))
        }
    }
}

/// Defaults overlaid with the global row, then the user's row.
pub fn load(conn: &Connection, user_id: &str) -> rusqlite::Result<Resolved> {
    let mut entries: BTreeMap<&'static str, Entry> = DEFS
        .iter()
        .map(|d| {
            (
                d.key,
                Entry {
                    value: d.default.to_json(),
                    source: Source::Default,
                    default: d.default.to_json(),
                    updated_at: None,
                    group: d.group,
                    about: d.about,
                },
            )
        })
        .collect();

    // '' sorts first, so the user's own row is applied last and wins.
    let mut stmt = conn.prepare(
        "SELECT key, value_json, source, updated_at FROM sleep_config
         WHERE user_id = '' OR user_id = ?1 ORDER BY user_id ASC",
    )?;
    let rows = stmt.query_map(params![user_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (key, value_json, source, updated_at) = row?;
        let Some(d) = def(&key) else { continue };
        let parsed = serde_json::from_str::<Value>(&value_json)
            .ok()
            .and_then(|v| validate(d, &v).ok());
        let (Some(value), Some(source)) = (parsed, Source::parse(&source)) else {
            tracing::warn!(key = %key, "sleep_config row ignored: bad value or source");
            continue;
        };
        if let Some(e) = entries.get_mut(d.key) {
            e.value = value;
            e.source = source;
            e.updated_at = updated_at;
        }
    }
    Ok(Resolved { entries })
}

/// Writes owner values for one user, all or nothing. Every key must exist and
/// belong to `group`.
pub fn set_owner(
    conn: &Connection,
    user_id: &str,
    group: Group,
    values: &[(String, Value)],
) -> Result<(), ConfigError> {
    let mut checked = Vec::with_capacity(values.len());
    for (key, value) in values {
        let d = def(key).ok_or_else(|| ConfigError::UnknownKey(key.clone()))?;
        if d.group != group {
            return Err(ConfigError::WrongGroup(key.clone()));
        }
        checked.push((d.key, validate(d, value)?));
    }
    let tx = conn.unchecked_transaction()?;
    for (key, value) in checked {
        tx.execute(
            "INSERT INTO sleep_config (user_id, key, value_json, source, updated_at)
             VALUES (?1, ?2, ?3, 'owner', CURRENT_TIMESTAMP)
             ON CONFLICT(user_id, key) DO UPDATE SET
                 value_json = excluded.value_json,
                 source = 'owner',
                 updated_at = CURRENT_TIMESTAMP",
            params![user_id, key, value.to_string()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Writes fitted values for one user (step 3.4). Keys must exist and be
/// numeric; values are clamped to the key's range. A row the user set
/// (`source = 'owner'`) is never replaced. Returns the keys actually written.
pub fn set_fitted(
    conn: &Connection,
    user_id: &str,
    values: &[(&str, f64)],
) -> Result<Vec<String>, ConfigError> {
    let mut checked = Vec::with_capacity(values.len());
    for (key, value) in values {
        let d = def(key).ok_or_else(|| ConfigError::UnknownKey((*key).to_string()))?;
        if !matches!(d.default, DefaultValue::Num(_)) || !value.is_finite() {
            return Err(ConfigError::BadValue { key: d.key.to_string(), why: "fitted values are finite numbers".into() });
        }
        checked.push((d.key, validate(d, &serde_json::json!(value.clamp(d.min, d.max)))?));
    }
    let tx = conn.unchecked_transaction()?;
    let mut written = Vec::new();
    for (key, value) in checked {
        let n = tx.execute(
            "INSERT INTO sleep_config (user_id, key, value_json, source, updated_at)
             VALUES (?1, ?2, ?3, 'fitted', CURRENT_TIMESTAMP)
             ON CONFLICT(user_id, key) DO UPDATE SET
                 value_json = excluded.value_json,
                 source = 'fitted',
                 updated_at = CURRENT_TIMESTAMP
             WHERE sleep_config.source <> 'owner'",
            params![user_id, key, value.to_string()],
        )?;
        if n > 0 {
            written.push(key.to_string());
        }
    }
    tx.commit()?;
    Ok(written)
}
