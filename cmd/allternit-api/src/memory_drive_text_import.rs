//! Import memories from another assistant into the personal drive: preview
//! first, then one commit. Text comes pasted, uploaded, or as the answer to
//! the export prompt we show for each assistant. Accepts plain lines or
//! bullets, numbered lists, or JSON (an array of strings, or objects with
//! `content`/`memory`/`text`). The candidate checks here are shared by every
//! import source (see `memory_drive_agent_import`).
use std::collections::BTreeSet;

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

/// One memory to import and where it came from (a provenance label).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub from: String,
    pub path: String,
    pub entries: Vec<String>,
    pub skipped: Vec<Skipped>,
    /// Candidates not checked because this run already has `MAX_ITEMS`
    /// entries. Importing again picks them up.
    pub remaining: usize,
}

/// An assistant we can import from: its id, how its memories are labelled
/// and filed, and its display name.
struct Assistant {
    id: &'static str,
    source: &'static str,
    path: &'static str,
    name: &'static str,
}

const ASSISTANTS: &[Assistant] = &[
    Assistant { id: "chatgpt", source: "chatgpt:memory-import", path: "imports/chatgpt.md", name: "ChatGPT" },
    Assistant { id: "claude", source: "claude:memory-import", path: "imports/claude.md", name: "Claude" },
    Assistant { id: "gemini", source: "gemini:memory-import", path: "imports/gemini.md", name: "Gemini" },
    Assistant { id: "grok", source: "grok:memory-import", path: "imports/grok.md", name: "Grok" },
    Assistant { id: "copilot", source: "copilot:memory-import", path: "imports/copilot.md", name: "Copilot" },
    Assistant { id: "perplexity", source: "perplexity:memory-import", path: "imports/perplexity.md", name: "Perplexity" },
    Assistant { id: "meta", source: "meta-ai:memory-import", path: "imports/meta-ai.md", name: "Meta AI" },
    Assistant { id: "kimi", source: "kimi:memory-import", path: "imports/kimi.md", name: "Kimi" },
    Assistant { id: "mistral", source: "le-chat:memory-import", path: "imports/le-chat.md", name: "Le Chat" },
    Assistant { id: "mem0", source: "mem0:memory-import", path: "imports/mem0.md", name: "Mem0" },
    Assistant { id: "other", source: "import:pasted", path: "imports/other.md", name: "another assistant" },
];

/// Keep a provenance label inside the characters `validate_source` accepts.
pub fn provenance_label(scheme: &str, rest: &str) -> String {
    let clean: String = rest
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._~/@=+-".contains(c) { c } else { '-' })
        .take(400)
        .collect();
    format!("{scheme}:{clean}")
}

/// The scheme memories from `from` carry (`chatgpt`, `meta-ai`, `import`, …).
pub fn scheme(from: &str) -> Result<&'static str> {
    assistant(from).map(|a| a.source.split(':').next().unwrap_or("import"))
}

/// Display name of an assistant, for commit messages.
pub fn display_name(from: &str) -> Result<&'static str> {
    assistant(from).map(|a| a.name)
}

fn assistant(from: &str) -> Result<&'static Assistant> {
    ASSISTANTS.iter().find(|a| a.id == from).ok_or_else(|| {
        let ids = ASSISTANTS.iter().map(|a| a.id).collect::<Vec<_>>().join(", ");
        ServiceError::Provenance(format!("from must be one of: {ids}"))
    })
}

pub fn source_label(from: &str) -> Result<(&'static str, &'static str)> {
    assistant(from).map(|a| (a.source, a.path))
}

fn json_items(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| json_items(x, out)),
        serde_json::Value::Object(o) => {
            if let Some(t) = ["content", "memory", "text", "value"].iter().find_map(|k| o.get(*k).and_then(|x| x.as_str())) {
                out.push(t.to_string());
            } else if let Some(list) = ["memories", "results", "items", "data"].iter().find_map(|k| o.get(*k)) {
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

/// The accepted candidates of a run, with their sources.
pub struct Checked {
    pub entries: Vec<Candidate>,
    pub skipped: Vec<Skipped>,
    pub remaining: usize,
}

/// Check candidates in order and keep up to `MAX_ITEMS`: skip text that is
/// too long, looks like a secret, repeats, is already remembered, or carries
/// `[key: …]` metadata.
pub fn check(db: &DbHandle, owner: &str, items: Vec<Candidate>) -> Result<Checked> {
    check_with(db, owner, items, false)
}

/// [`check`] for imports that run by themselves (agent folder and vendor
/// syncs): also skip anything the person deleted or forgot, so a sync never
/// brings it back. A person's own import may bring it back on purpose.
pub fn check_auto(db: &DbHandle, owner: &str, items: Vec<Candidate>) -> Result<Checked> {
    check_with(db, owner, items, true)
}

fn check_with(db: &DbHandle, owner: &str, items: Vec<Candidate>, skip_removed: bool) -> Result<Checked> {
    static META: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let meta = META.get_or_init(|| regex::Regex::new(r" \[[A-Za-z_][A-Za-z0-9_-]*:").expect("constant regex"));
    let conn = db.connect()?;
    let mut seen = BTreeSet::new();
    let (mut entries, mut skipped) = (Vec::new(), Vec::new());
    let total = items.len();
    let mut examined = 0;
    for raw in items {
        if entries.len() >= MAX_ITEMS {
            break;
        }
        examined += 1;
        let t = raw.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            continue;
        }
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
        } else if skip_removed
            && conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_facts WHERE user_id=?1 AND lower(fact)=?2 AND valid_until IS NOT NULL)",
                rusqlite::params![owner, t.to_lowercase()],
                |r| r.get::<_, bool>(0),
            )?
        {
            Some("you removed it before")
        } else if meta.is_match(&t) {
            Some("contains [key: …] text")
        } else {
            None
        };
        match reason {
            Some(reason) => skipped.push(Skipped { text: t.chars().take(200).collect(), reason }),
            None => entries.push(Candidate { text: t, source: raw.source }),
        }
    }
    Ok(Checked { entries, skipped, remaining: total - examined })
}

/// Commit checked entries to `path` as one change, keeping only `selected`
/// texts when given. Returns (revision, imported count).
pub fn commit(
    db: &DbHandle,
    owner: &str,
    entries: Vec<Candidate>,
    selected: Option<&[String]>,
    path: &str,
    message: &str,
) -> Result<(Option<String>, usize)> {
    let keep: Option<BTreeSet<&str>> = selected.map(|s| s.iter().map(String::as_str).collect());
    let adds: Vec<_> = entries
        .into_iter()
        .filter(|c| keep.as_ref().map_or(true, |k| k.contains(c.text.as_str())))
        .map(|c| crate::memory_drive_writer::NewFact {
            text: c.text,
            source: Some(c.source),
            path: Some(path.to_string()),
            ..Default::default()
        })
        .collect();
    if adds.is_empty() {
        return Err(ServiceError::Provenance("Choose at least one memory to import.".into()));
    }
    let out = crate::memory_drive_writer::commit_facts(db, owner, &adds, &[], message)?.ok_or(ServiceError::NotFound)?;
    Ok((out.revision, out.fact_ids.iter().filter(|f| f.is_some()).count()))
}

fn candidates(text: &str, from: &str) -> Result<Vec<Candidate>> {
    let a = assistant(from)?;
    let items = split(text);
    if items.len() > MAX_ITEMS {
        return Err(ServiceError::Provenance(format!("Import up to {MAX_ITEMS} memories at a time.")));
    }
    Ok(items.into_iter().map(|text| Candidate { text, source: a.source.to_string() }).collect())
}

pub fn plan(db: &DbHandle, owner: &str, text: &str, from: &str) -> Result<Plan> {
    let a = assistant(from)?;
    let c = check(db, owner, candidates(text, from)?)?;
    Ok(Plan {
        from: from.to_string(),
        path: a.path.to_string(),
        entries: c.entries.into_iter().map(|e| e.text).collect(),
        skipped: c.skipped,
        remaining: c.remaining,
    })
}

/// Apply as one commit. Returns (revision, imported count).
pub fn apply(db: &DbHandle, owner: &str, text: &str, from: &str, selected: Option<&[String]>) -> Result<(Option<String>, usize)> {
    let a = assistant(from)?;
    let c = check(db, owner, candidates(text, from)?)?;
    commit(db, owner, c.entries, selected, a.path, &format!("Import memory from {}", a.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_lists_bullets_numbers_and_json() {
        assert_eq!(split("- Likes tea\n* Lives in Saint Paul\n1. Uses Rust\n# Heading\n\n• Has a dog"), vec!["Likes tea", "Lives in Saint Paul", "Uses Rust", "Has a dog"]);
        assert_eq!(split(r#"["A","B"]"#), vec!["A", "B"]);
        assert_eq!(split(r#"{"memories":[{"content":"A"},{"memory":"B"}]}"#), vec!["A", "B"]);
        // Mem0's list and export shape.
        assert_eq!(split(r#"{"results":[{"id":"1","memory":"Likes tea","user_id":"u"}]}"#), vec!["Likes tea"]);
    }

    #[test]
    fn every_assistant_has_a_valid_label_and_path() {
        for a in ASSISTANTS {
            crate::memory_drive::validate_source(a.source).expect(a.id);
            assert!(a.path.starts_with("imports/") && a.path.ends_with(".md"), "{}", a.id);
        }
        assert!(source_label("myspace").is_err());
    }
}
