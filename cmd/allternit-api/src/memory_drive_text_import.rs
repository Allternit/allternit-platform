//! Import memories from another assistant (ChatGPT's or Claude's memory
//! list, pasted or uploaded) into the personal drive: preview first, then one
//! commit. Accepts plain lines or bullets, numbered lists, or JSON (an array
//! of strings, or objects with `content`/`memory`/`text`).
use serde::Serialize;

use crate::db::DbHandle;
use crate::memory_drive_service::{Result, ServiceError};

pub const MAX_ITEMS: usize = 500;
const MAX_LEN: usize = 1000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Skipped {
    pub text: String,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub from: String,
    pub path: String,
    pub entries: Vec<String>,
    pub skipped: Vec<Skipped>,
}

pub fn source_label(from: &str) -> Result<(&'static str, &'static str)> {
    Ok(match from {
        "chatgpt" => ("chatgpt:memory-import", "imports/chatgpt.md"),
        "claude" => ("claude:memory-import", "imports/claude.md"),
        "other" => ("import:pasted", "imports/other.md"),
        _ => return Err(ServiceError::Provenance("from must be chatgpt, claude or other".into())),
    })
}

fn json_items(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| json_items(x, out)),
        serde_json::Value::Object(o) => {
            if let Some(t) = ["content", "memory", "text", "value"].iter().find_map(|k| o.get(*k).and_then(|x| x.as_str())) {
                out.push(t.to_string());
            } else if let Some(list) = ["memories", "items", "data"].iter().find_map(|k| o.get(*k)) {
                json_items(list, out);
            }
        }
        _ => {}
    }
}

/// Split pasted or uploaded text into candidate memories.
pub fn split(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let mut out = Vec::new();
            json_items(&v, &mut out);
            return out;
        }
    }
    static NUM: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let num = NUM.get_or_init(|| regex::Regex::new(r"^\d{1,4}[.)]\s+").expect("constant regex"));
    trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```"))
        .map(|l| {
            let l = l.trim_start_matches(['-', '*', '•', '·']).trim();
            num.replace(l, "").trim().to_string()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn plan(db: &DbHandle, owner: &str, text: &str, from: &str) -> Result<Plan> {
    let (_, path) = source_label(from)?;
    let items = split(text);
    if items.len() > MAX_ITEMS {
        return Err(ServiceError::Provenance(format!("Import up to {MAX_ITEMS} memories at a time.")));
    }
    let conn = db.connect()?;
    let mut seen = std::collections::BTreeSet::new();
    let (mut entries, mut skipped) = (Vec::new(), Vec::new());
    for raw in items {
        let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let reason = if t.chars().count() > MAX_LEN {
            Some("too long")
        } else if crate::memory_kernel_service::mentions_secret(&t) || crate::memory_drive::scan_secrets(&t).is_err() {
            Some("looks like a password or key")
        } else if !seen.insert(t.to_lowercase()) {
            Some("listed twice")
        } else if conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_facts WHERE user_id=?1 AND lower(fact)=?2 AND valid_until IS NULL)",
            rusqlite::params![owner, t.to_lowercase()],
            |r| r.get::<_, bool>(0),
        )? {
            Some("already remembered")
        } else if regex::Regex::new(r" \[[A-Za-z_][A-Za-z0-9_-]*:").expect("constant regex").is_match(&t) {
            Some("contains [key: …] text")
        } else {
            None
        };
        match reason {
            Some(reason) => skipped.push(Skipped { text: t.chars().take(200).collect(), reason }),
            None => entries.push(t),
        }
    }
    Ok(Plan { from: from.to_string(), path: path.to_string(), entries, skipped })
}

/// Apply as one commit. Returns (revision, imported count).
pub fn apply(db: &DbHandle, owner: &str, text: &str, from: &str) -> Result<(Option<String>, usize)> {
    let (source, _) = source_label(from)?;
    let p = plan(db, owner, text, from)?;
    let adds: Vec<_> = p
        .entries
        .iter()
        .map(|t| crate::memory_drive_writer::NewFact {
            text: t.clone(),
            source: Some(source.to_string()),
            path: Some(p.path.clone()),
            ..Default::default()
        })
        .collect();
    let label = match from { "chatgpt" => "ChatGPT", "claude" => "Claude", _ => "another assistant" };
    let out = crate::memory_drive_writer::commit_facts(db, owner, &adds, &[], &format!("Import memory from {label}"))?
        .ok_or(ServiceError::NotFound)?;
    Ok((out.revision, out.fact_ids.iter().filter(|f| f.is_some()).count()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_lists_bullets_numbers_and_json() {
        assert_eq!(split("- Likes tea\n* Lives in Saint Paul\n1. Uses Rust\n# Heading\n\n• Has a dog"), vec!["Likes tea", "Lives in Saint Paul", "Uses Rust", "Has a dog"]);
        assert_eq!(split(r#"["A","B"]"#), vec!["A", "B"]);
        assert_eq!(split(r#"{"memories":[{"content":"A"},{"memory":"B"}]}"#), vec!["A", "B"]);
    }
}
