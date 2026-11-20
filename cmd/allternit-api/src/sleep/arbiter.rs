//! The dispute arbiter (D10, step 6.1): picks a side of a contradiction with
//! no model call. Same inputs, same verdict.
//!
//! Inputs, in order:
//! 1. **Scope.** Both sides carry a context qualifier ("at work", "at home",
//!    "on weekends", ...) and the qualifiers differ, or the two facts belong to
//!    different agents: both can be true, so the pick is `both_scoped` and
//!    nothing is asked.
//! 2. **Support** for each side, a weighted sum of numbers in 0..1:
//!    `credence` (from step 3.3: what you said beats what was inferred),
//!    `recency` (`0.5 ± 0.5 · clamp(age gap / dispute_recency_days)`, the newer
//!    side up) and, only when a goal text is given, `goal` (each side's share
//!    of the words it has in common with the goal). Weights are config:
//!    `dispute_w_credence`, `dispute_w_recency`, `dispute_w_goal`.
//!    `margin = |support_a − support_b| / Σ weights used`, so it is 0..1. The
//!    side with more support is picked; an exact tie picks side b (the newer
//!    write).
//! 3. **Stakes** (0..1) from money, health and legal words in either side.
//!    Stakes never change the pick; they decide whether to ask: a margin
//!    under `dispute_ask_margin` with stakes at or above `dispute_ask_stakes`
//!    gives `needs_user` and no pick.
//!
//! No goal store exists yet, so callers pass `None` today and the goal input
//! drops out of the sum (the reason never cites it).

use std::collections::HashSet;

use super::config::Resolved;
use super::types::DisputeChoice;

/// One side as the arbiter sees it.
#[derive(Debug, Clone)]
pub struct Side<'a> {
    pub text: &'a str,
    /// 0..1, already resolved by the caller (credence column, else source).
    pub credence: f64,
    /// `user_said`, `model_inferred`, ... (for the reason only).
    pub source_type: Option<&'a str>,
    /// When the side became true, as a Julian day (SQLite `julianday`).
    pub as_of_jd: Option<f64>,
    pub agent_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// `None` when the user must pick (`needs_user`).
    pub pick: Option<DisputeChoice>,
    pub margin: f64,
    pub stakes: f64,
    pub scope: Option<String>,
    /// One short sentence a user can read.
    pub reason: String,
    pub needs_user: bool,
}

/// Context qualifiers that make two facts true in different situations.
const SCOPES: &[&str] = &[
    "at work", "for work", "at the office", "at home", "for personal", "personally", "professionally",
    "on weekends", "on the weekend", "on weekdays", "during the week", "in the morning", "in the evening",
    "at night", "when traveling", "when travelling", "on vacation", "for side projects", "for clients",
    "for client work", "in winter", "in summer", "at school", "for school",
];

/// Stakes per category; the highest category in either side wins.
const STAKES: &[(&str, f64, &[&str])] = &[
    ("health", 0.9, &["allerg", "medic", "diagnos", "doctor", "surgery", "health", "prescription", "dose", "blood type", "pregnan", "insulin"]),
    ("legal", 0.8, &["lawsuit", "lawyer", "contract", "legal", "attorney", "visa", "custody", "court ", "court."]),
    ("money", 0.8, &["$", "salary", "invoice", "mortgage", "rent ", "rent.", "rental", "loan", "tax ", "tax.", "taxes", "debt", "budget", "bank", "account number", "routing number", "price", "paid", "owes", "owed"]),
    ("deadline", 0.6, &["deadline", "due date", "due by", "expires"]),
];
const BASE_STAKES: f64 = 0.2;

fn norm(text: &str) -> String {
    let t: String = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() || c == '$' { c } else { ' ' }).collect();
    format!(" {} ", t.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The context qualifier in a fact, if any.
pub fn scope_of(text: &str) -> Option<&'static str> {
    let t = norm(text);
    SCOPES.iter().copied().find(|s| t.contains(&format!(" {s} ")))
}

/// Stakes of one text and the category that set them. Markers match at the
/// start of a word ("rent" matches "rental", not "current").
pub fn stakes_of(text: &str) -> (f64, Option<&'static str>) {
    let t = format!(" {} ", text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "));
    let at_word_start = |w: &str| t.match_indices(w).any(|(i, _)| i > 0 && !t[..i].chars().last().is_some_and(char::is_alphanumeric));
    STAKES
        .iter()
        .filter(|(_, _, words)| words.iter().any(|w| at_word_start(w)))
        .map(|(name, s, _)| (*s, Some(*name)))
        .fold((BASE_STAKES, None), |best, x| if x.0 > best.0 { x } else { best })
}

fn words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

fn source_words(s: Option<&str>) -> &'static str {
    match s {
        Some("user_said") => "what you said",
        Some("tool_observed") => "a tool",
        Some("imported") => "an import",
        Some("dream_generated") => "a Dream",
        Some("model_inferred") => "the assistant's guess",
        _ => "an older note",
    }
}

/// Decide one dispute. `goal` is the user's current goal text, if known.
pub fn decide(cfg: &Resolved, a: &Side<'_>, b: &Side<'_>, goal: Option<&str>) -> Verdict {
    let (sa, ca) = stakes_of(a.text);
    let (sb, cb) = stakes_of(b.text);
    let (stakes, category) = if sb > sa { (sb, cb) } else { (sa, ca) };

    // 1. Scope: both true in different contexts.
    let scoped = match (scope_of(a.text), scope_of(b.text)) {
        (Some(x), Some(y)) if x != y => Some(format!("a: {x}; b: {y}")),
        _ => match (a.agent_id, b.agent_id) {
            (Some(x), Some(y)) if x != y => Some(format!("a: agent {x}; b: agent {y}")),
            _ => None,
        },
    };
    if let Some(scope) = scoped {
        return Verdict {
            pick: Some(DisputeChoice::BothScoped),
            margin: 1.0,
            stakes,
            scope: Some(scope),
            reason: "Both can be true: they apply in different situations.".into(),
            needs_user: false,
        };
    }

    // 2. Support.
    let w_c = cfg.number("dispute_w_credence").max(0.0);
    let w_r = cfg.number("dispute_w_recency").max(0.0);
    let days = cfg.number("dispute_recency_days").max(1e-6);
    let gap = match (a.as_of_jd, b.as_of_jd) {
        (Some(x), Some(y)) => y - x,
        _ => 0.0,
    };
    let r_b = 0.5 + 0.5 * (gap / days).clamp(-1.0, 1.0);
    let r_a = 1.0 - r_b;
    let (mut g_a, mut g_b, mut w_g) = (0.0, 0.0, 0.0);
    if let Some(goal) = goal {
        let gw = words(goal);
        let (oa, ob) = (words(a.text).intersection(&gw).count() as f64, words(b.text).intersection(&gw).count() as f64);
        if oa + ob > 0.0 {
            (g_a, g_b, w_g) = (oa / (oa + ob), ob / (oa + ob), cfg.number("dispute_w_goal").max(0.0));
        }
    }
    let total = w_c + w_r + w_g;
    let parts = [
        ("credence", w_c * (a.credence.clamp(0.0, 1.0) - b.credence.clamp(0.0, 1.0))),
        ("recency", w_r * (r_a - r_b)),
        ("goal", w_g * (g_a - g_b)),
    ];
    let diff: f64 = parts.iter().map(|(_, d)| d).sum();
    let margin = if total > 0.0 { (diff.abs() / total).clamp(0.0, 1.0) } else { 0.0 };
    let tie = diff.abs() < 1e-9;
    let pick = if diff > 0.0 && !tie { DisputeChoice::A } else { DisputeChoice::B };

    // 3. Ask on a close call with high stakes.
    if margin < cfg.number("dispute_ask_margin") && stakes >= cfg.number("dispute_ask_stakes") {
        return Verdict {
            pick: None,
            margin,
            stakes,
            scope: None,
            reason: format!("These are too close to call and they touch {}, so you decide.", category.unwrap_or("something important")),
            needs_user: true,
        };
    }

    let reason = if tie {
        "Both are equally supported, so the newer one is used.".to_string()
    } else {
        // The input that pushed hardest towards the pick.
        let sign = if pick == DisputeChoice::A { 1.0 } else { -1.0 };
        let top = parts.iter().map(|(n, d)| (*n, d * sign)).fold(("", f64::MIN), |best, x| if x.1 > best.1 { x } else { best });
        let (win, lose) = if pick == DisputeChoice::A { (a, b) } else { (b, a) };
        match top.0 {
            "credence" if win.source_type != lose.source_type => {
                format!("It comes from {}, the other from {}.", source_words(win.source_type), source_words(lose.source_type))
            }
            "credence" => "It is the more trusted of the two.".to_string(),
            "goal" => "It fits your current goal better.".to_string(),
            _ => "It is the more recent of the two.".to_string(),
        }
    };
    Verdict { pick: Some(pick), margin, stakes, scope: None, reason, needs_user: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn cfg() -> Resolved {
        let db = DbHandle::new_memory().unwrap();
        super::super::config::load(&db.connect().unwrap(), "u1").unwrap()
    }

    fn side<'a>(text: &'a str, credence: f64, src: &'a str, jd: f64) -> Side<'a> {
        Side { text, credence, source_type: Some(src), as_of_jd: Some(jd), agent_id: None }
    }

    #[test]
    fn what_you_said_beats_an_inference_and_the_reason_says_so() {
        let v = decide(&cfg(), &side("User drinks coffee.", 0.9, "user_said", 100.0), &side("User drinks tea.", 0.5, "model_inferred", 100.0), None);
        assert_eq!(v.pick, Some(DisputeChoice::A));
        assert!(v.margin > 0.15 && v.margin <= 1.0, "{v:?}");
        assert!(v.reason.contains("what you said"), "{v:?}");
    }

    #[test]
    fn recency_breaks_equal_credence_and_a_tie_picks_the_newer_write() {
        let v = decide(&cfg(), &side("User lives in Austin.", 0.9, "user_said", 100.0), &side("User lives in Denver.", 0.9, "user_said", 160.0), None);
        assert_eq!((v.pick, v.reason.as_str()), (Some(DisputeChoice::B), "It is the more recent of the two."));
        let t = decide(&cfg(), &side("User lives in Austin.", 0.9, "user_said", 100.0), &side("User lives in Denver.", 0.9, "user_said", 100.0), None);
        assert_eq!((t.pick, t.margin), (Some(DisputeChoice::B), 0.0));
    }

    #[test]
    fn different_contexts_are_both_scoped() {
        let v = decide(&cfg(), &side("User uses a Mac at work.", 0.9, "user_said", 1.0), &side("User uses Linux at home.", 0.9, "user_said", 2.0), None);
        assert_eq!((v.pick, v.needs_user), (Some(DisputeChoice::BothScoped), false));
        assert_eq!(v.scope.as_deref(), Some("a: at work; b: at home"));
    }

    #[test]
    fn a_close_call_with_high_stakes_asks_and_picks_nothing() {
        let v = decide(&cfg(), &side("User is allergic to penicillin.", 0.9, "user_said", 5.0), &side("User is not allergic to penicillin.", 0.9, "user_said", 5.0), None);
        assert_eq!((v.pick, v.needs_user), (None, true));
        assert!(v.stakes >= 0.7 && v.reason.contains("health"), "{v:?}");
        // The same close call with low stakes is decided.
        let low = decide(&cfg(), &side("User likes jazz.", 0.9, "user_said", 5.0), &side("User dislikes jazz.", 0.9, "user_said", 5.0), None);
        assert_eq!((low.pick.is_some(), low.needs_user), (true, false));
    }

    #[test]
    fn stakes_match_whole_word_starts() {
        assert_eq!(stakes_of("User's current job is at Acme.").0, BASE_STAKES);
        assert_eq!(stakes_of("Rent is $1,200 a month.").1, Some("money"));
        assert_eq!(stakes_of("User takes medication daily.").1, Some("health"));
    }

    #[test]
    fn a_goal_only_counts_when_given() {
        let a = side("User's main project is Atlas.", 0.9, "user_said", 5.0);
        let b = side("User's main project is Borealis.", 0.9, "user_said", 5.0);
        assert_eq!(decide(&cfg(), &a, &b, Some("ship the Atlas launch")).pick, Some(DisputeChoice::A));
        assert_eq!(decide(&cfg(), &a, &b, None).reason, "Both are equally supported, so the newer one is used.");
    }
}
