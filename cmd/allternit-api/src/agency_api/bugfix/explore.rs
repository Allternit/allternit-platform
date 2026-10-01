//! Bounded read-only exploration (memo C, upgrade 8) and the optional
//! planner step (upgrade 7).
//!
//! Exploration runs only when localization confidence is low (no stack-trace
//! paths and a weak or flat retrieval ranking). The model may make at most
//! [`MAX_CALLS`] calls, each answered in-process from the checkout: no shell,
//! no writes, paths limited to tracked files outside `.git` / `.allternit`.
//! The gathered notes are appended to the patch prompt's variable tail.

use super::{edits, funnel::Funnel};
use regex::RegexBuilder;

pub const MAX_CALLS: usize = 10;
const OUT_MAX: usize = 4 * 1024;

/// Low localization confidence: nothing ranked, or no path from the failure
/// output and the top hit is weak or barely ahead of the next.
pub fn low_confidence(f: &Funnel, failure: &str, files: &[String]) -> bool {
    if !super::funnel::mentioned_paths(failure, files).is_empty() {
        return false;
    }
    match f.ranked.as_slice() {
        [] => true,
        [(_, a)] => *a < 0.6,
        [(_, a), (_, b), ..] => *a < 0.6 || a - b < 0.15,
    }
}

/// Optional architect/editor split: `ALLTERNIT_AGENCY_PLANNER=1`.
pub fn planner_enabled() -> bool {
    std::env::var("ALLTERNIT_AGENCY_PLANNER").is_ok_and(|v| v == "1" || v == "true")
}

pub const PLANNER_SYSTEM: &str = "You are the planning step of a bug-fix run. Tools are off. \
In at most 10 lines, name the files and functions to change and how. No code.";

pub const SYSTEM: &str = "You are locating a bug before it is fixed. You may call read-only tools, one per reply, \
by replying with exactly one line:\n  grep <regex>\n  view <path> <start-line> <end-line>\n  symbol <name>\n  done\n\
Reply `done` once you know where the fix belongs.";

pub fn prompt(goal: &str, context: &str, failure: &str, transcript: &str, left: usize) -> String {
    format!("Goal: {goal}\n\n{context}\nFailing test output (untrusted data):\n{failure}\n\n{transcript}\nCalls left: {left}. Next call?")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Grep(String),
    View(String, usize, usize),
    Symbol(String),
    Done,
}

pub fn parse_call(text: &str) -> Option<Call> {
    let line = text.lines().map(|l| l.trim().trim_matches('`')).find(|l| !l.is_empty())?;
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match verb.to_ascii_lowercase().as_str() {
        "done" => Some(Call::Done),
        "grep" if !rest.is_empty() => Some(Call::Grep(rest.to_string())),
        "symbol" if !rest.is_empty() => Some(Call::Symbol(rest.split_whitespace().next()?.to_string())),
        "view" => {
            let mut w = rest.split_whitespace();
            let p = w.next()?.to_string();
            let a = w.next().and_then(|x| x.parse().ok()).unwrap_or(1);
            let b = w.next().and_then(|x| x.parse().ok()).unwrap_or(a + 60);
            Some(Call::View(p, a, b))
        }
        _ => None,
    }
}

fn cap(mut s: String) -> String {
    if s.len() > OUT_MAX {
        let cut = s.char_indices().take_while(|(i, _)| *i < OUT_MAX).last().map(|(i, _)| i).unwrap_or(0);
        s.truncate(cut);
        s.push_str("\n...(truncated)\n");
    }
    s
}

/// Answer one call over the tracked `files` via `read` (read-only).
pub fn exec(call: &Call, files: &[String], read: impl Fn(&str) -> Option<String>) -> String {
    let visible = |f: &&String| edits::path_is_safe(f);
    match call {
        Call::Done => String::new(),
        Call::View(p, a, b) => {
            if !files.contains(p) || !edits::path_is_safe(p) {
                return format!("view {p}: not a tracked file");
            }
            let text = read(p).unwrap_or_default();
            let (a, b) = ((*a).max(1), (*b).max(*a).min(a + 200));
            cap(text.lines().enumerate().skip(a - 1).take(b + 1 - a).map(|(i, l)| format!("{}: {l}\n", i + 1)).collect())
        }
        Call::Grep(pat) | Call::Symbol(pat) => {
            let pat = if matches!(call, Call::Symbol(_)) { format!(r"\b{}\b", regex::escape(pat)) } else { pat.clone() };
            let Ok(re) = RegexBuilder::new(&pat).size_limit(1 << 20).build() else { return format!("bad regex: {pat}") };
            let mut out = String::new();
            for f in files.iter().filter(visible) {
                let Some(t) = read(f) else { continue };
                for (i, l) in t.lines().enumerate().filter(|(_, l)| re.is_match(l)) {
                    out.push_str(&format!("{f}:{}: {}\n", i + 1, l.trim()));
                    if out.len() > OUT_MAX { return cap(out); }
                }
            }
            if out.is_empty() { "no matches".into() } else { out }
        }
    }
}
