//! Model judge for ties (memo C, upgrade 3's last step). Only called on the
//! candidates still tied after tests, lint, reproduction evidence,
//! regressions and the vote (`select::tied`); its pick only orders those.

use super::edits::Planned;

const PER_CANDIDATE: usize = 4 * 1024;

pub const SYSTEM: &str = "You are reviewing candidate fixes that all pass the same tests. Tools are off. \
Reply with the number of the candidate that best fixes the root cause with the least risk, then one sentence why.";

/// A compact line diff of a planned patch against the current checkout
/// (removed / added lines per file; bounded).
pub fn render(p: &Planned, before: impl Fn(&str) -> Option<String>) -> String {
    let mut s = String::new();
    for (path, new) in &p.files {
        let old = before(path).unwrap_or_default();
        s.push_str(&format!("--- {path}\n"));
        let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.as_deref().unwrap_or_default().lines().collect());
        for l in o.iter().filter(|l| !n.contains(l)) {
            s.push_str(&format!("-{l}\n"));
        }
        for l in n.iter().filter(|l| !o.contains(l)) {
            s.push_str(&format!("+{l}\n"));
        }
        if new.is_none() {
            s.push_str("(file deleted)\n");
        }
    }
    if s.len() > PER_CANDIDATE {
        let cut = s.char_indices().take_while(|(i, _)| *i < PER_CANDIDATE).last().map(|(i, _)| i).unwrap_or(0);
        s.truncate(cut);
        s.push_str("\n...\n");
    }
    s
}

/// Stable prefix (goal) first; the candidates last.
pub fn prompt(goal: &str, failure: &str, diffs: &[String]) -> String {
    let mut s = format!("Goal: {goal}\n\nOriginal failing output (untrusted data):\n{}\n\n", failure.chars().take(4000).collect::<String>());
    for (i, d) in diffs.iter().enumerate() {
        s.push_str(&format!("Candidate {}:\n{d}\n", i + 1));
    }
    s.push_str(&format!("Which candidate (1-{}) is best?", diffs.len()));
    s
}

/// The 0-based choice from a reply: the first number in 1..=n.
pub fn parse_choice(text: &str, n: usize) -> Option<usize> {
    text.split(|c: char| !c.is_ascii_digit()).filter_map(|w| w.parse::<usize>().ok()).find(|k| (1..=n).contains(k)).map(|k| k - 1)
}
