//! Prospective memory (step 6.4, stage `intentions`): "remind me to ..." kept
//! as an intention and brought back at the right moment. No model call, no
//! scheduler, no network.
//!
//! **Capture.** Every user turn that is recorded (`record_observation`, kind
//! `turn_user`) passes through [`on_turn`]. Two kinds of cue:
//!
//! - *Event* ("remind me when I mention Acme", "next time we talk about X,
//!   ...", "when I open the billing project ..."): `cue` is the cue words,
//!   matched later as a run of whole words in a turn. The row expires
//!   `intention_event_expiry_days` after it was made.
//! - *Time* ("remind me tomorrow at 9", "on Friday", "in two weeks", "at
//!   5pm", "on 2026-12-01"): `due_at` is parsed from the words, in UTC; a day
//!   with no clock means 09:00. A date phrase this parser cannot place
//!   exactly ("next month", "on March 3rd", "soon") is kept as an *event* cue
//!   on those words and reported as approximate.
//!
//! **Fire.** The same hook runs first on each turn, before it captures: open
//! rows past `expires_at` become `expired`; an event row whose cue words are
//! in the turn, or a time row with `due_at <= now`, becomes `fired`. A time
//! cue therefore fires on the first turn at or after its due time.
//!
//! **Deliver.** [`deliver`] (called by `GET /memory/sleep/context`) returns the
//! fired rows once and marks them `done`, so the runtime preloads each
//! reminder on its next request and never twice.
//!
//! With the stage off nothing is captured, fired or delivered.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::config;
use super::strength::fmt_ts;
use super::types::Stage;

const MAX_CONTENT_CHARS: usize = 300;
const MAX_CUE_WORDS: usize = 6;

/// A cue found in a user turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub content: String,
    /// `event` or `time`.
    pub cue_kind: &'static str,
    pub cue: String,
    pub due_at: Option<DateTime<Utc>>,
    /// A date phrase kept as an event cue because it could not be placed.
    pub approximate: bool,
}

const EVENT_LEADS: &[&str] = &[
    "next time we talk about ", "next time we discuss ", "next time i mention ", "next time i bring up ",
    "next time i talk about ", "when i mention ", "when i bring up ", "when we talk about ",
    "when i talk about ", "when we discuss ", "when i open ", "when i start ", "when i switch to ",
];
const CUE_ENDS: &[&str] = &[",", ".", ";", "?", "!", "\n", " remind", " to ", " that ", " tell ", " ask ", " and ", " so "];
const CUE_SUFFIXES: &[&str] = &[" project", " channel", " page", " folder", " workspace", " chat", " repo", " topic"];
const CUE_STOP: &[&str] = &["it", "this", "that", "them", "something", "anything", "everything", "again", "me", "us"];
const UNSURE: &[&str] = &[
    "later", "soon", "eventually", "weekend", "morning", "evening", "tonight", "month", "year", "january", "february",
    "march", "april", "june", "july", "august", "september", "october", "november", "december",
];

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect()
}

fn word_start(low: &str, idx: usize) -> bool {
    idx == 0 || !low[..idx].chars().next_back().is_some_and(char::is_alphanumeric)
}

fn trim_edges(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ",.;:!?\"'".contains(c))
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_CONTENT_CHARS).collect()
}

fn strip_lead<'a>(s: &'a str, leads: &[&str]) -> &'a str {
    let mut s = trim_edges(s);
    loop {
        let low = s.to_ascii_lowercase();
        match leads.iter().copied().find(|l| low.starts_with(l)) {
            Some(l) => s = trim_edges(&s[l.len()..]),
            None => return s,
        }
    }
}

const REMIND_LEADS: &[&str] = &["remind me to ", "remind me that ", "remind me ", "to ", "that ", "tell me ", "then "];

/// Find a cue in `text` (ASCII-lowercase indexes stay aligned with `text`).
pub fn parse(text: &str, now: DateTime<Utc>) -> Option<Parsed> {
    let text = text.trim();
    let low = text.to_ascii_lowercase();
    if let Some(p) = parse_event(text, &low) {
        return p;
    }
    parse_time(text, &low, now)
}

/// `Some(None)`: an event lead was found but the cue was unusable.
fn parse_event(text: &str, low: &str) -> Option<Option<Parsed>> {
    let (idx, lead) = EVENT_LEADS
        .iter()
        .filter_map(|l| low.find(l).filter(|i| word_start(low, *i)).map(|i| (i, *l)))
        .min_by_key(|(i, _)| *i)?;
    let cue_start = idx + lead.len();
    let rest = &low[cue_start..];
    let cue_len = CUE_ENDS.iter().filter_map(|e| rest.find(e)).min().unwrap_or(rest.len());
    let mut cue = trim_edges(&rest[..cue_len]).to_string();
    for p in ["the ", "a ", "my ", "our "] {
        if let Some(r) = cue.strip_prefix(p) {
            cue = r.to_string();
        }
    }
    for s in CUE_SUFFIXES {
        if let Some(r) = cue.strip_suffix(s) {
            cue = r.to_string();
        }
    }
    let words = tokens(&cue);
    if words.is_empty() || words.len() > MAX_CUE_WORDS || words.iter().all(|w| CUE_STOP.contains(&w.as_str())) {
        return Some(None);
    }
    let after = strip_lead(&text[cue_start + cue_len..], REMIND_LEADS);
    let before = strip_lead(&text[..idx], REMIND_LEADS);
    let content = if after.chars().count() >= 3 {
        after
    } else if before.chars().count() >= 3 {
        before
    } else {
        text
    };
    Some(Some(Parsed { content: clip(content), cue_kind: "event", cue: words.join(" "), due_at: None, approximate: false }))
}

fn parse_time(text: &str, low: &str, now: DateTime<Utc>) -> Option<Parsed> {
    let at = low.find("remind me").filter(|i| word_start(low, *i) && !low[*i + 9..].starts_with(|c: char| c.is_alphanumeric()))? + "remind me".len();
    let after = &low[at..];
    let tail = text[at..].trim();
    let tl = tail.to_ascii_lowercase();
    let content = if tl.starts_with("to ") {
        &tail[3..]
    } else if let Some(i) = tl.find(" to ") {
        &tail[i + 4..]
    } else {
        text
    };
    let content = trim_edges(content);
    let content = if content.chars().count() >= 3 { content } else { text };
    match find_time(after, now) {
        Some(Hit::At(due)) => Some(Parsed { content: clip(content), cue_kind: "time", cue: "time".into(), due_at: Some(due), approximate: false }),
        Some(Hit::Unsure(phrase)) => Some(Parsed { content: clip(content), cue_kind: "event", cue: phrase, due_at: None, approximate: true }),
        None => None,
    }
}

enum Hit {
    At(DateTime<Utc>),
    Unsure(String),
}

fn weekday(w: &str) -> Option<Weekday> {
    Some(match w {
        "monday" | "mon" => Weekday::Mon,
        "tuesday" | "tue" | "tues" => Weekday::Tue,
        "wednesday" | "wed" => Weekday::Wed,
        "thursday" | "thu" | "thur" | "thurs" => Weekday::Thu,
        "friday" | "fri" => Weekday::Fri,
        "saturday" | "sat" => Weekday::Sat,
        "sunday" | "sun" => Weekday::Sun,
        _ => return None,
    })
}

fn number(t: &str) -> Option<i64> {
    if let Ok(n) = t.parse::<i64>() {
        return Some(n);
    }
    ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"]
        .iter()
        .position(|w| *w == t)
        .map(|n| n as i64)
        .or(if t == "a" || t == "an" { Some(1) } else { None })
}

/// `(hour, minute)` at `toks[j]`, only when written as a clock time: after
/// "at", with am/pm, or as `H:MM`.
fn clock(toks: &[String], mut j: usize) -> Option<(u32, u32)> {
    let had_at = toks.get(j).is_some_and(|t| t == "at");
    if had_at {
        j += 1;
    }
    let t = toks.get(j)?;
    if t == "noon" {
        return Some((12, 0));
    }
    let end = t.find(|c: char| !(c.is_ascii_digit() || c == ':')).unwrap_or(t.len());
    let (num, mut suf) = t.split_at(end);
    let (h, m) = match num.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (num.parse::<u32>().ok()?, 0),
    };
    if suf.is_empty() {
        if let Some(n) = toks.get(j + 1).filter(|n| *n == "am" || *n == "pm") {
            suf = n;
        }
    }
    if !(had_at || !suf.is_empty() || num.contains(':')) {
        return None;
    }
    let h = match suf {
        "pm" if h < 12 => h + 12,
        "am" if h == 12 => 0,
        "am" | "pm" | "" => h,
        _ => return None,
    };
    (h < 24 && m < 60).then_some((h, m))
}

fn find_time(after: &str, now: DateTime<Utc>) -> Option<Hit> {
    let toks: Vec<String> = after
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '-'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let today = now.date_naive();
    let on = |date: NaiveDate, c: Option<(u32, u32)>| {
        let (h, m) = c.unwrap_or((9, 0));
        date.and_hms_opt(h, m, 0).map(|n| Hit::At(n.and_utc()))
    };
    for i in 0..toks.len() {
        let t = toks[i].as_str();
        if t == "tomorrow" {
            return on(today + Duration::days(1), clock(&toks, i + 1));
        }
        if t == "in" {
            if let (Some(n), Some(unit)) = (toks.get(i + 1).and_then(|n| number(n)), toks.get(i + 2)) {
                let dur = match unit.trim_end_matches('s') {
                    "minute" | "min" => Duration::minutes(n),
                    "hour" | "hr" => Duration::hours(n),
                    "day" => Duration::days(n),
                    "week" => Duration::weeks(n),
                    "month" => Duration::days(30 * n),
                    _ => continue,
                };
                return Some(Hit::At(now + dur));
            }
        }
        if t == "next" && toks.get(i + 1).is_some_and(|n| n == "week") {
            return on(today + Duration::days(7), clock(&toks, i + 2));
        }
        if let Some(w) = weekday(t) {
            let mut d = today + Duration::days(1);
            while d.weekday() != w {
                d += Duration::days(1);
            }
            return on(d, clock(&toks, i + 1));
        }
        if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
            return on(d, clock(&toks, i + 1));
        }
    }
    for i in 0..toks.len() {
        if let Some((h, m)) = clock(&toks, i) {
            let mut due = today.and_hms_opt(h, m, 0)?.and_utc();
            if due <= now {
                due += Duration::days(1);
            }
            return Some(Hit::At(due));
        }
    }
    let unsure = toks.iter().any(|t| {
        UNSURE.contains(&t.as_str()) || (t.len() > 2 && t[..t.len() - 2].chars().all(|c| c.is_ascii_digit()) && ["st", "nd", "rd", "th"].contains(&&t[t.len() - 2..]))
    });
    unsure.then(|| {
        let end = after.find(|c: char| ".;!?\n".contains(c)).unwrap_or(after.len());
        let phrase: String = trim_edges(&after[..end]).chars().take(60).collect();
        Hit::Unsure(tokens(&phrase).join(" "))
    })
}

/// Stage gate and the per-turn work: fire what this turn triggers, then
/// capture any new intention in it (so a turn never fires its own cue).
pub fn on_turn(conn: &Connection, user: &str, observation_id: &str, text: &str, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let cfg = config::load(conn, user)?;
    if !cfg.stage_on(Stage::Intentions) {
        return Ok(());
    }
    fire(conn, user, text, now)?;
    capture(conn, user, observation_id, text, now, cfg.number("intention_event_expiry_days"))?;
    Ok(())
}

/// Expire, then fire the rows this turn triggers. Returns how many fired.
pub fn fire(conn: &Connection, user: &str, text: &str, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    let now_s = fmt_ts(now);
    expire(conn, user, now)?;
    let mut fired = conn.execute(
        "UPDATE memory_intentions SET state = 'fired', fired_at = ?2
         WHERE user_id = ?1 AND state = 'open' AND cue_kind = 'time' AND due_at IS NOT NULL AND due_at <= ?2",
        params![user, now_s],
    )?;
    let turn = tokens(text);
    let mut stmt = conn.prepare("SELECT id, cue FROM memory_intentions WHERE user_id = ?1 AND state = 'open' AND cue_kind = 'event'")?;
    let open = stmt.query_map(params![user], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, cue) in open {
        let words = tokens(&cue.unwrap_or_default());
        if !words.is_empty() && turn.windows(words.len()).any(|w| w == words.as_slice()) {
            fired += conn.execute("UPDATE memory_intentions SET state = 'fired', fired_at = ?2 WHERE id = ?1 AND state = 'open'", params![id, now_s])?;
        }
    }
    Ok(fired)
}

/// Open rows past `expires_at` become `expired`.
pub fn expire(conn: &Connection, user: &str, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_intentions SET state = 'expired'
         WHERE user_id = ?1 AND state = 'open' AND expires_at IS NOT NULL AND expires_at < ?2",
        params![user, fmt_ts(now)],
    )
}

/// Write the intention in `text`, if any. A repeat of the same open
/// intention is not stored twice.
pub fn capture(conn: &Connection, user: &str, observation_id: &str, text: &str, now: DateTime<Utc>, expiry_days: f64) -> rusqlite::Result<Option<String>> {
    let Some(p) = parse(text, now) else { return Ok(None) };
    let dup: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memory_intentions WHERE user_id = ?1 AND state = 'open' AND cue_kind = ?2 AND cue IS ?3 AND content = ?4 AND due_at IS ?5)",
        params![user, p.cue_kind, p.cue, p.content, p.due_at.map(fmt_ts)],
        |r| r.get(0),
    )?;
    if dup {
        return Ok(None);
    }
    let expires = (p.cue_kind == "event").then(|| fmt_ts(now + Duration::days(expiry_days.max(1.0) as i64)));
    let id = format!("int_{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO memory_intentions (id, user_id, content, cue_kind, cue, due_at, expires_at, state, source_observation_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open', ?8, ?9)",
        params![id, user, p.content, p.cue_kind, p.cue, p.due_at.map(fmt_ts), expires, observation_id, fmt_ts(now)],
    )?;
    Ok(Some(id))
}

/// The fired rows, each returned once (then `done`), and how many are still
/// open. Expired rows are dropped first.
pub fn deliver(conn: &Connection, user: &str, now: DateTime<Utc>) -> rusqlite::Result<(Vec<Value>, i64)> {
    expire(conn, user, now)?;
    let mut stmt = conn.prepare(
        "SELECT id, content, cue_kind, cue, due_at, fired_at FROM memory_intentions
         WHERE user_id = ?1 AND state = 'fired' ORDER BY fired_at ASC, created_at ASC LIMIT 50",
    )?;
    let rows = stmt
        .query_map(params![user], |r| {
            Ok((
                r.get::<_, String>(0)?,
                json!({
                    "id": r.get::<_, String>(0)?,
                    "content": r.get::<_, String>(1)?,
                    "cue_kind": r.get::<_, String>(2)?,
                    "cue": r.get::<_, Option<String>>(3)?,
                    "due_at": r.get::<_, Option<String>>(4)?,
                    "fired_at": r.get::<_, Option<String>>(5)?,
                }),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (id, v) in rows {
        // The state check makes a second reader get nothing.
        if conn.execute("UPDATE memory_intentions SET state = 'done' WHERE id = ?1 AND state = 'fired'", params![id])? == 1 {
            out.push(v);
        }
    }
    let open: i64 = conn.query_row("SELECT COUNT(*) FROM memory_intentions WHERE user_id = ?1 AND state = 'open'", params![user], |r| r.get(0))?;
    Ok((out, open))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use super::super::config::{self, Group};
    use super::super::strength::parse_ts;

    fn db() -> DbHandle {
        let dir = std::env::temp_dir().join(format!("sleep-intentions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        DbHandle::new(dir.join("t.db")).unwrap()
    }

    /// 2026-01-01 is a Thursday.
    fn t0() -> DateTime<Utc> {
        parse_ts("2026-01-01 12:00:00").unwrap()
    }

    fn set(db: &DbHandle, stage: bool) {
        let conn = db.connect().unwrap();
        let v = vec![("enabled".to_string(), json!(true)), ("stage.intentions".to_string(), json!(stage))];
        config::set_owner(&conn, "u1", Group::Setting, &v).unwrap();
    }

    fn turn(conn: &Connection, text: &str, now: DateTime<Utc>) {
        on_turn(conn, "u1", "obs_1", text, now).unwrap();
    }

    fn state_of(conn: &Connection) -> String {
        conn.query_row("SELECT state FROM memory_intentions WHERE user_id = 'u1'", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn remind_me_when_i_mention_acme_preloads_on_a_later_acme_turn() {
        let db = db();
        set(&db, true);
        let conn = db.connect().unwrap();
        turn(&conn, "Remind me when I mention Acme, to ask about the renewal", t0());
        assert_eq!(deliver(&conn, "u1", t0()).unwrap().0.len(), 0, "the turn that made it does not fire it");
        turn(&conn, "what is the weather", t0() + Duration::minutes(5));
        assert_eq!(deliver(&conn, "u1", t0() + Duration::minutes(5)).unwrap().0.len(), 0);
        turn(&conn, "Acme sent a new invoice", t0() + Duration::minutes(10));
        let (got, open) = deliver(&conn, "u1", t0() + Duration::minutes(10)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["content"], "ask about the renewal");
        assert_eq!(got[0]["cue"], "acme");
        assert_eq!(open, 0);
    }

    #[test]
    fn a_time_intention_fires_at_the_first_turn_after_its_due_time() {
        let db = db();
        set(&db, true);
        let conn = db.connect().unwrap();
        turn(&conn, "remind me tomorrow at 9 to call the bank", t0());
        let due: String = conn.query_row("SELECT due_at FROM memory_intentions", [], |r| r.get(0)).unwrap();
        assert_eq!(due, "2026-01-02 09:00:00");
        turn(&conn, "hello", parse_ts("2026-01-02 08:59:00").unwrap());
        assert_eq!(deliver(&conn, "u1", parse_ts("2026-01-02 08:59:00").unwrap()).unwrap().0.len(), 0);
        // Due time passes with no turn: nothing fires without one.
        assert_eq!(state_of(&conn), "open");
        turn(&conn, "hello again", parse_ts("2026-01-02 10:30:00").unwrap());
        let (got, _) = deliver(&conn, "u1", parse_ts("2026-01-02 10:30:00").unwrap()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["content"], "call the bank");
    }

    #[test]
    fn expired_intentions_drop() {
        let db = db();
        set(&db, true);
        let conn = db.connect().unwrap();
        turn(&conn, "next time we talk about Acme, mention the discount", t0());
        let late = t0() + Duration::days(31);
        turn(&conn, "Acme is back", late);
        assert_eq!(deliver(&conn, "u1", late).unwrap(), (vec![], 0));
        assert_eq!(state_of(&conn), "expired");
    }

    #[test]
    fn a_delivered_intention_is_not_delivered_twice() {
        let db = db();
        set(&db, true);
        let conn = db.connect().unwrap();
        turn(&conn, "when I open the billing project, remind me to send invoices", t0());
        turn(&conn, "opening billing now", t0() + Duration::minutes(1));
        assert_eq!(deliver(&conn, "u1", t0() + Duration::minutes(2)).unwrap().0.len(), 1);
        assert_eq!(deliver(&conn, "u1", t0() + Duration::minutes(3)).unwrap().0.len(), 0);
        turn(&conn, "billing again", t0() + Duration::minutes(4));
        assert_eq!(deliver(&conn, "u1", t0() + Duration::minutes(5)).unwrap().0.len(), 0);
        assert_eq!(state_of(&conn), "done");
    }

    #[test]
    fn stage_off_no_intentions_captured() {
        let db = db();
        set(&db, false);
        let conn = db.connect().unwrap();
        turn(&conn, "remind me when I mention Acme", t0());
        turn(&conn, "remind me tomorrow at 9", t0());
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM memory_intentions", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn parses_cues() {
        let p = parse("remind me on Friday to send the deck", t0()).unwrap();
        assert_eq!((p.cue_kind, p.due_at.map(fmt_ts).as_deref(), p.content.as_str()), ("time", Some("2026-01-02 09:00:00"), "send the deck"));
        let p = parse("Remind me in two weeks", t0()).unwrap();
        assert_eq!(p.due_at.map(fmt_ts).as_deref(), Some("2026-01-15 12:00:00"));
        let p = parse("remind me at 5pm to stretch", t0()).unwrap();
        assert_eq!(p.due_at.map(fmt_ts).as_deref(), Some("2026-01-01 17:00:00"));
        let p = parse("remind me next month to renew the domain", t0()).unwrap();
        assert!(p.approximate && p.cue_kind == "event" && p.cue == "next month to renew the domain");
        assert!(parse("next time we talk about it, hi", t0()).is_none());
        assert!(parse("I will remind myself later", t0()).is_none());
        let p = parse("remind me to send the deck when I mention Acme", t0()).unwrap();
        assert_eq!((p.cue.as_str(), p.content.as_str()), ("acme", "send the deck"));
    }
}
