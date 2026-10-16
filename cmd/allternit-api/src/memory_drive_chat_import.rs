//! Find memories in chats exported from another assistant (a ChatGPT,
//! Claude, Gemini or Grok data export). The app opens the export in the
//! browser and sends only the person's own messages, a few conversations at
//! a time; this reads them with the memory model and returns facts, each
//! tied to the conversation it came from. Nothing is saved until the person
//! previews the facts and imports them through [`plan`] / [`apply`], which
//! run the same checks as every other import.
use serde::{Deserialize, Serialize};

use crate::db::DbHandle;
use crate::memory_drive_service::{Result, ServiceError};
use crate::memory_drive_text_import::{self as text_import, Candidate, Plan};

/// Conversations per request.
pub const MAX_CONVERSATIONS: usize = 20;
/// Characters of the person's messages per conversation and per request.
pub const MAX_CONVERSATION_CHARS: usize = 8_000;
pub const MAX_BATCH_CHARS: usize = 80_000;
/// Facts accepted per import request (preview or apply).
pub const MAX_FOUND: usize = 5_000;
const MAX_FACT_CHARS: usize = 300;

#[derive(Debug, Clone, Deserialize)]
pub struct Conversation {
    /// The export's own conversation id.
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    /// When it happened, as the export gives it.
    #[serde(default)]
    pub date: Option<String>,
    /// The person's own messages, oldest first.
    pub text: String,
}

/// One fact found in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub text: String,
    pub conversation: String,
}

pub const SYSTEM_PROMPT: &str = "You read a person's past chats with an AI assistant and write down what a new assistant \
should remember about them. You only output JSON.";

pub fn reply_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["memories"],
        "properties": { "memories": { "type": "array", "items": { "type": "object",
            "additionalProperties": false,
            "required": ["fact", "conversation"],
            "properties": {
                "fact": { "type": "string", "maxLength": MAX_FACT_CHARS },
                "conversation": { "type": "integer", "minimum": 1 }
            } } } }
    })
}

fn clip(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn build_prompt(conversations: &[Conversation]) -> String {
    let mut body = String::new();
    for (i, c) in conversations.iter().enumerate() {
        let title = c.title.as_deref().map(|t| clip(t.trim(), 120)).filter(|t| !t.is_empty()).unwrap_or("Untitled");
        let date = c.date.as_deref().map(|d| format!(" ({})", clip(d.trim(), 40))).unwrap_or_default();
        body.push_str(&format!("## Conversation {}: {title}{date}\n{}\n\n", i + 1, clip(c.text.trim(), MAX_CONVERSATION_CHARS)));
    }
    format!(
        r#"Below are messages a person wrote to an AI assistant, grouped by conversation. Only their side is shown.

List what a new assistant should remember about this person: identity, role, work, projects,
tools and stack, preferences, how they like answers, the people and things they mention by name,
and recurring constraints.

Do NOT include:
- questions, requests or one-off tasks
- facts about the world, or anything not about this person
- temporary context ("I'm tired today")
- secrets: passwords, API keys, card or account numbers
- health, religion, politics or sexuality unless the person plainly wants that remembered

Write each memory as one short sentence in the third person ("User is a product designer in Austin.").
Give the number of the conversation it came from. Don't repeat a memory; if it appears in several
conversations, cite the latest. If there is nothing worth remembering, return {{"memories": []}}.

{body}Return JSON only: {{"memories": [{{"fact": "...", "conversation": 1}}]}}"#
    )
}

/// Check a request's size before any model call.
pub fn validate(conversations: &[Conversation]) -> Result<()> {
    if conversations.is_empty() || conversations.len() > MAX_CONVERSATIONS {
        return Err(ServiceError::Provenance(format!("Send 1 to {MAX_CONVERSATIONS} conversations at a time.")));
    }
    let chars: usize = conversations.iter().map(|c| c.text.chars().count().min(MAX_CONVERSATION_CHARS)).sum();
    if chars > MAX_BATCH_CHARS {
        return Err(ServiceError::Provenance(format!("Send up to {MAX_BATCH_CHARS} characters at a time.")));
    }
    if conversations.iter().any(|c| c.id.trim().is_empty() || c.id.len() > 200) {
        return Err(ServiceError::Provenance("Every conversation needs an id of up to 200 characters.".into()));
    }
    Ok(())
}

/// Facts in a model reply, mapped to conversation ids. Out-of-range
/// conversation numbers and empty or overlong facts are dropped.
pub fn parse_reply(raw: &str, conversations: &[Conversation]) -> Option<Vec<Found>> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let items = v.get("memories")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|m| {
                let fact = m.get("fact")?.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
                let n = m.get("conversation")?.as_u64()? as usize;
                let c = conversations.get(n.checked_sub(1)?)?;
                (!fact.is_empty() && fact.chars().count() <= MAX_FACT_CHARS).then(|| Found { text: fact, conversation: c.id.clone() })
            })
            .collect(),
    )
}

fn chats_path(from: &str) -> Result<String> {
    let (_, path) = text_import::source_label(from)?;
    Ok(path.trim_end_matches(".md").to_string() + "-chats.md")
}

fn candidates(from: &str, found: Vec<Found>) -> Result<Vec<Candidate>> {
    if found.len() > MAX_FOUND {
        return Err(ServiceError::Provenance(format!("Import up to {MAX_FOUND} memories at a time.")));
    }
    let scheme = text_import::scheme(from)?;
    Ok(found
        .into_iter()
        .map(|f| Candidate { text: f.text, source: text_import::provenance_label(scheme, &format!("conversation/{}", f.conversation)) })
        .collect())
}

pub fn plan(db: &DbHandle, owner: &str, from: &str, found: Vec<Found>) -> Result<Plan> {
    let path = chats_path(from)?;
    let c = text_import::check(db, owner, candidates(from, found)?)?;
    Ok(Plan { from: from.to_string(), path, entries: c.entries.into_iter().map(|e| e.text).collect(), skipped: c.skipped, remaining: c.remaining })
}

pub fn apply(db: &DbHandle, owner: &str, from: &str, found: Vec<Found>, selected: Option<&[String]>) -> Result<(Option<String>, usize)> {
    let path = chats_path(from)?;
    let c = text_import::check(db, owner, candidates(from, found)?)?;
    let name = text_import::display_name(from)?;
    text_import::commit(db, owner, c.entries, selected, &path, &format!("Import memories from {name} chats"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str, text: &str) -> Conversation {
        Conversation { id: id.into(), title: Some("Trip".into()), date: Some("2026-09-01".into()), text: text.into() }
    }

    #[test]
    fn prompt_numbers_conversations_and_clips_text() {
        let long = "x".repeat(MAX_CONVERSATION_CHARS + 50);
        let p = build_prompt(&[conv("a", "I live in Saint Paul"), conv("b", &long)]);
        assert!(p.contains("## Conversation 1: Trip (2026-09-01)\nI live in Saint Paul"));
        assert!(p.contains("## Conversation 2: Trip"));
        assert!(!p.contains(&long));
    }

    #[test]
    fn reply_maps_numbers_to_ids_and_drops_bad_items() {
        let convs = [conv("c-1", "x"), conv("c-2", "y")];
        let raw = "```json\n{\"memories\": [{\"fact\": \"User lives in Saint Paul.\", \"conversation\": 2}, {\"fact\": \"Bad\", \"conversation\": 9}, {\"fact\": \"  \", \"conversation\": 1}, {\"fact\": \"Zero\", \"conversation\": 0}]}\n```";
        assert_eq!(parse_reply(raw, &convs).unwrap(), vec![Found { text: "User lives in Saint Paul.".into(), conversation: "c-2".into() }]);
        assert!(parse_reply("no json", &convs).is_none());
    }

    #[test]
    fn requests_are_bounded() {
        assert!(validate(&[]).is_err());
        let many: Vec<_> = (0..=MAX_CONVERSATIONS).map(|i| conv(&i.to_string(), "x")).collect();
        assert!(validate(&many).is_err());
        let big: Vec<_> = (0..MAX_CONVERSATIONS).map(|i| conv(&i.to_string(), &"y".repeat(MAX_CONVERSATION_CHARS))).collect();
        assert!(validate(&big).is_err());
        assert!(validate(&[conv("", "x")]).is_err());
        assert!(validate(&[conv("ok", "x")]).is_ok());
    }

    #[test]
    fn sources_cite_the_conversation_and_stay_valid() {
        let c = candidates("chatgpt", vec![Found { text: "User likes tea.".into(), conversation: "67a1-b2 c?d".into() }]).unwrap();
        assert_eq!(c[0].source, "chatgpt:conversation/67a1-b2-c-d");
        crate::memory_drive::validate_source(&c[0].source).unwrap();
        let o = candidates("other", vec![Found { text: "x".into(), conversation: "1".into() }]).unwrap();
        crate::memory_drive::validate_source(&o[0].source).unwrap();
        assert_eq!(chats_path("meta").unwrap(), "imports/meta-ai-chats.md");
    }
}
