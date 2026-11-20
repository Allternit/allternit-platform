//! Write-time memory scores (step 3.3). No model call, no network.
//!
//! Each new fact gets `attention` (how much it deserves keeping), `credence`
//! (how likely it is true), `flashbulb` (the user flagged it) and
//! `source_type`. The rule that matters: emphasis raises priority, never
//! truth. "Remember this" sets `flashbulb` and lifts `attention`; `credence`
//! comes from the source alone (user_said > tool_observed > imported >
//! model_inferred > dream_generated).
//!
//! Gate: the Sleep master switch (`Resolved::enabled`). No stage flag fits
//! (`strengths` is step 3.2 and a different job), so with Sleep off every
//! score column stays NULL, exactly as before.
//!
//! The "goal link" signal is not scored yet: no intention table exists until
//! stage `intentions` (6.4). The "use" signal is a hook ([`apply_use`]) that
//! step 3.1 labels will call later; nothing uses it at write time.

use std::collections::HashSet;

use rusqlite::{params, Connection};

use super::config::{self, Resolved};
use super::types::SourceType;

/// Inputs to [`score`], all explicit so the logic is testable without a DB.
#[derive(Debug, Clone, Copy)]
pub struct Signals {
    pub source: SourceType,
    pub flagged: bool,
    pub self_disclosure: bool,
    pub repeat: bool,
    pub surprise: bool,
    pub stakes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    pub attention: f64,
    pub credence: f64,
    pub flashbulb: bool,
    pub source_type: SourceType,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Credence for a source, from config (defaults in `config::DEFS`).
pub fn credence_for(cfg: &Resolved, source: SourceType) -> f64 {
    let key = match source {
        SourceType::UserSaid => "credence_user_said",
        SourceType::ToolObserved => "credence_tool_observed",
        SourceType::Imported => "credence_imported",
        SourceType::ModelInferred => "credence_model_inferred",
        SourceType::DreamGenerated => "credence_dream_generated",
    };
    clamp01(cfg.number(key))
}

/// Pure scoring. `flagged` never touches `credence`.
pub fn score(cfg: &Resolved, s: Signals) -> Score {
    let mut a = cfg.number("attention_base");
    if s.flagged {
        a += cfg.number("attention_w_remember");
    }
    if s.self_disclosure {
        a += cfg.number("attention_w_self");
    }
    if s.repeat {
        a += cfg.number("attention_w_repeat");
    }
    if s.surprise {
        a += cfg.number("attention_w_surprise");
    }
    if s.stakes {
        a += cfg.number("attention_w_stakes");
    }
    Score { attention: clamp01(a), credence: credence_for(cfg, s.source), flashbulb: s.flagged, source_type: s.source }
}

/// Source type from the observation kind the fact was extracted from.
/// `turn_user` / `explicit_memory` are the user's own words; `turn_assistant`
/// and `turn_system` are the model's; `turn_tool` is a tool report;
/// `dream_extraction` is a Sleep/dream write; `import*` kinds are imports.
/// An unknown kind is treated as `model_inferred` (never more trusted).
pub fn source_for_kind(kind: &str) -> SourceType {
    match kind {
        "turn_user" | "explicit_memory" => SourceType::UserSaid,
        "turn_tool" => SourceType::ToolObserved,
        "dream_extraction" => SourceType::DreamGenerated,
        k if k.starts_with("import") => SourceType::Imported,
        _ => SourceType::ModelInferred,
    }
}

const REMEMBER_MARKERS: &[&str] = &["remember", "don't forget", "dont forget", "do not forget", "note that", "important:"];

pub fn has_remember_marker(text: &str) -> bool {
    let t = text.to_lowercase();
    REMEMBER_MARKERS.iter().any(|m| t.contains(m))
}

const SELF_MARKERS: &[&str] = &["i am ", "i'm ", "i live", "i work", "my ", "i was ", "i have ", "we are ", "our "];

pub fn is_self_disclosure(text: &str) -> bool {
    let t = format!(" {}", text.to_lowercase());
    SELF_MARKERS.iter().any(|m| t.contains(&format!(" {m}")) || t.trim_start().starts_with(m))
}

const STAKES_MARKERS: &[&str] = &[
    "$", "salary", "invoice", "mortgage", "rent", "loan", "tax", "debt", "budget", "bank", "account number",
    "allerg", "medic", "diagnos", "doctor", "surgery", "health", "prescription",
    "lawsuit", "lawyer", "contract", "legal", "attorney", "visa", "custody",
    "deadline", "due date", "due by", "password", "passcode", "routing number",
];

/// Stakes keywords only raise attention. The text is scored, never stored here.
pub fn has_stakes(text: &str) -> bool {
    let t = text.to_lowercase();
    STAKES_MARKERS.iter().any(|m| t.contains(m))
}

fn tokens(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_string)
        .collect()
}

/// Normalized word-set overlap, 0..1.
pub fn jaccard(a: &str, b: &str) -> f64 {
    let (ta, tb) = (tokens(a), tokens(b));
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count() as f64;
    inter / ta.union(&tb).count() as f64
}

/// True when any of `others` is a near-duplicate of `fact`.
pub fn is_repeat(fact: &str, others: &[String], threshold: f64) -> bool {
    others.iter().any(|o| jaccard(fact, o) >= threshold)
}

/// Hook for step 3.1: answer-used labels raise attention later. Not called at
/// write time. Returns the new attention.
pub fn apply_use(attention: f64, used: bool, step: f64) -> f64 {
    if used { clamp01(attention + step) } else { attention }
}

fn load_cfg(conn: &Connection, user_id: &str) -> Option<Resolved> {
    config::load(conn, user_id).ok().filter(|c| c.enabled())
}

/// Score one just-written fact and write the four columns. Does nothing (and
/// returns Ok) when Sleep is off, so the columns stay NULL. `observation_id`
/// names the turn the fact came from; it supplies the source kind and the
/// text the markers are read from (the fact text is used if it is missing).
pub fn score_written_fact(conn: &Connection, user_id: &str, fact_id: &str, fact: &str, observation_id: &str) -> rusqlite::Result<()> {
    let Some(cfg) = load_cfg(conn, user_id) else { return Ok(()) };
    let obs: Option<(String, String)> = conn
        .query_row(
            "SELECT kind, content FROM memory_observations WHERE id = ?1 AND user_id = ?2",
            params![observation_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (kind, turn) = obs.unwrap_or_else(|| (String::new(), fact.to_string()));
    let source = source_for_kind(&kind);
    // An explicit memory is the user flagging the fact, whatever its words.
    let flagged = kind == "explicit_memory" || has_remember_marker(&turn);
    let others: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT fact FROM memory_facts WHERE user_id = ?1 AND id != ?2 AND valid_until IS NULL
             ORDER BY valid_from DESC LIMIT 300",
        )?;
        let rows = stmt.query_map(params![user_id, fact_id], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let sc = score(
        &cfg,
        Signals {
            source,
            flagged,
            self_disclosure: is_self_disclosure(fact) || (is_self_disclosure(&turn) && source == SourceType::UserSaid),
            repeat: is_repeat(fact, &others, cfg.number("attention_repeat_jaccard")),
            surprise: false,
            stakes: has_stakes(fact),
        },
    );
    conn.execute(
        "UPDATE memory_facts SET attention = ?3, credence = ?4, flashbulb = ?5, source_type = ?6
         WHERE id = ?1 AND user_id = ?2",
        params![fact_id, user_id, sc.attention, sc.credence, sc.flashbulb as i64, sc.source_type.as_str()],
    )?;
    Ok(())
}

/// A fact that updates an existing one is a surprise: raise its attention.
/// Only touches facts already scored (Sleep on).
pub fn note_surprise(conn: &Connection, user_id: &str, fact_id: &str) -> rusqlite::Result<()> {
    let Some(cfg) = load_cfg(conn, user_id) else { return Ok(()) };
    conn.execute(
        "UPDATE memory_facts SET attention = MIN(1.0, attention + ?3) WHERE id = ?1 AND user_id = ?2 AND attention IS NOT NULL",
        params![fact_id, user_id, cfg.number("attention_w_surprise")],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::memory_kernel_service as kernel;

    fn cfg_default() -> Resolved {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        config::load(&conn, "u1").unwrap()
    }

    fn sig(source: SourceType, flagged: bool) -> Signals {
        Signals { source, flagged, self_disclosure: false, repeat: false, surprise: false, stakes: false }
    }

    #[test]
    fn remember_flag_raises_priority_never_credence() {
        let cfg = cfg_default();
        let plain = score(&cfg, sig(SourceType::UserSaid, false));
        let flagged = score(&cfg, sig(SourceType::UserSaid, true));
        assert_eq!(plain.credence, flagged.credence);
        assert!(!plain.flashbulb && flagged.flashbulb);
        assert!(flagged.attention > plain.attention);
        assert!(has_remember_marker("Remember: I take 5mg daily") && !has_remember_marker("I take 5mg daily"));
    }

    #[test]
    fn credence_follows_source_order() {
        let cfg = cfg_default();
        let c = |s| score(&cfg, sig(s, false)).credence;
        assert!(c(SourceType::UserSaid) > c(SourceType::ToolObserved));
        assert!(c(SourceType::ToolObserved) > c(SourceType::Imported));
        assert!(c(SourceType::Imported) > c(SourceType::ModelInferred));
        assert!(c(SourceType::ModelInferred) > c(SourceType::DreamGenerated));
    }

    #[test]
    fn signals_add_and_attention_is_clamped() {
        let cfg = cfg_default();
        let base = score(&cfg, sig(SourceType::UserSaid, false)).attention;
        let all = score(&cfg, Signals { source: SourceType::UserSaid, flagged: true, self_disclosure: true, repeat: true, surprise: true, stakes: true });
        assert!(all.attention > base && all.attention <= 1.0);
        assert!(is_self_disclosure("I live in Austin") && !is_self_disclosure("The sky is blue"));
        assert!(has_stakes("Rent is $1,200 due by Friday") && !has_stakes("Likes tea"));
        assert!(jaccard("User lives in Austin", "user lives in austin texas") >= 0.6);
        assert!(jaccard("User lives in Austin", "Uses Linear daily") < 0.2);
    }

    #[test]
    fn source_comes_from_observation_kind() {
        assert_eq!(source_for_kind("turn_user"), SourceType::UserSaid);
        assert_eq!(source_for_kind("explicit_memory"), SourceType::UserSaid);
        assert_eq!(source_for_kind("turn_tool"), SourceType::ToolObserved);
        assert_eq!(source_for_kind("turn_assistant"), SourceType::ModelInferred);
        assert_eq!(source_for_kind("dream_extraction"), SourceType::DreamGenerated);
        assert_eq!(source_for_kind("import_chatgpt"), SourceType::Imported);
    }

    fn enable(conn: &Connection) {
        conn.execute("INSERT INTO sleep_config (user_id, key, value_json, source) VALUES ('u1', 'enabled', 'true', 'owner')", []).unwrap();
    }

    type Row = (Option<f64>, Option<f64>, Option<i64>, Option<String>);
    fn row(conn: &Connection, id: &str) -> Row {
        conn.query_row("SELECT attention, credence, flashbulb, source_type FROM memory_facts WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
    }

    #[test]
    fn write_path_scores_when_sleep_on_and_leaves_null_when_off() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        let plain_obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "I work in Rust.", Some("user")).unwrap();
        let flag_obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "Remember this: I live in Austin.", Some("user")).unwrap();
        let tool_obs = kernel::record_observation(&db, "u1", None, None, "turn_tool", "x", Some("tool")).unwrap();

        // Off: nothing is scored.
        let off = kernel::persist_facts(&db, "u1", None, &plain_obs, &["I work in Rust.".into()]).unwrap().remove(0);
        assert_eq!(row(&conn, &off.id), (None, None, None, None));

        enable(&conn);
        let plain = kernel::persist_facts(&db, "u1", None, &plain_obs, &["I work in Go.".into()]).unwrap().remove(0);
        let flag = kernel::persist_facts(&db, "u1", None, &flag_obs, &["I live in Austin.".into()]).unwrap().remove(0);
        let tool = kernel::persist_facts(&db, "u1", None, &tool_obs, &["Build 42 passed.".into()]).unwrap().remove(0);
        let (pa, pc, pf, ps) = row(&conn, &plain.id);
        let (fa, fc, ff, _) = row(&conn, &flag.id);
        let (_, tc, tf, ts) = row(&conn, &tool.id);
        assert_eq!((pf, ff, tf), (Some(0), Some(1), Some(0)));
        assert_eq!(pc, fc, "emphasis must not change credence");
        assert!(fa > pa);
        assert_eq!(ps.as_deref(), Some("user_said"));
        assert_eq!(ts.as_deref(), Some("tool_observed"));
        assert!(tc < pc);
    }
}
