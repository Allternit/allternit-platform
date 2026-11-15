//! The gateway uses the same generated picker registry as browser and Gizzi.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerPolicy {
    pub version: u8,
    pub mode: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}
#[derive(Deserialize)]
struct Template {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct Registry {
    templates: Vec<Template>,
    aliases: std::collections::HashMap<String, Vec<String>>,
}
fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| {
        serde_json::from_str(include_str!("answer_policy.registry.json"))
            .expect("generated answer picker")
    })
}
fn re(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("answer picker regex")
}
fn normalize(s: &str) -> String {
    re("[^a-z0-9]+")
        .replace_all(&s.to_ascii_lowercase(), " ")
        .trim()
        .into()
}
fn contains(text: &str, phrase: &str) -> bool {
    format!(" {text} ").contains(&format!(" {} ", normalize(phrase)))
}
fn core(id: &str) -> bool {
    matches!(
        id,
        "quick-answer"
            | "comparison"
            | "decision"
            | "plan"
            | "how-to"
            | "troubleshooting"
            | "status-report"
            | "research-brief"
            | "table"
            | "chart"
            | "form"
            | "calculator"
    )
}
impl AnswerPolicy {
    pub fn select(message: &str, visuals: bool) -> Self {
        let text: String = message.chars().take(8000).collect();
        let plain = |reason: &str| Self {
            version: 1,
            mode: "text".into(),
            reason: reason.into(),
            template: None,
        };
        let pick = |id: &str, reason: &str| Self {
            version: 1,
            mode: "template".into(),
            reason: reason.into(),
            template: Some(id.into()),
        };
        if !visuals
            || re(r"(?i)\b(?:plain text|text only|no cards|without cards)\b").is_match(&text)
        {
            return plain("text-only");
        }
        let request = re(r"```[\s\S]*?(?:```|$)").replace_all(&text, "");
        let request = re(r"(?m)^\s*>.*$").replace_all(&request, "").into_owned();
        if let Some(c)=re(r#"(?i)\b(?:use|render|show|select)\s+(?:the\s+)?(?:answer\s+)?template\s*:?\s*["'`]?([a-z][a-z0-9-]*)|\btemplate\s*:\s*["'`]?([a-z][a-z0-9-]*)"#).captures(&request){
   if let Some(id)=c.get(1).or_else(||c.get(2)).map(|m|m.as_str().to_ascii_lowercase()).filter(|id|registry().templates.iter().any(|t|t.id==*id)){return pick(&id,"explicit");}
  }
        let normalized = normalize(&request);
        for t in &registry().templates {
            for label in [&t.id, &t.name] {
                for verb in ["use", "render", "show", "select"] {
                    if contains(&normalized, &format!("{verb} {label} template"))
                        || contains(&normalized, &format!("{verb} the {label} template"))
                    {
                        return pick(&t.id, "explicit");
                    }
                }
            }
        }
        if re(r"\b(?:show|display|track|check|create|open|review)\b").is_match(&normalized)
            && (registry()
                .aliases
                .get("pull-request")
                .is_some_and(|a| a.iter().any(|p| contains(&normalized, p)))
                || re(r"https://github\.com/[^/\s]+/[^/\s]+/pull/\d+").is_match(&request))
        {
            return pick("pull-request", "job");
        }
        if re(r"\b(?:implement|refactor|debug|fix|css|component|write code)\b")
            .is_match(&normalized)
        {
            return plain("conversation");
        }
        if !re(r"\b(?:show|display|list|compare|summarize|calculate|track|find|give|make|create|prepare|review|analyze|check|plan)\b").is_match(&normalized)&&!re(r"^(?:what is|what s|how is|how s) (?:the )?(?:weather|temperature|stock price)\b").is_match(&normalized){return plain("conversation");}
        let mut matches = Vec::new();
        for t in &registry().templates {
            let mut phrases = registry().aliases.get(&t.id).cloned().unwrap_or_default();
            if !core(&t.id) {
                phrases.extend([t.id.clone(), t.name.clone()]);
            }
            for phrase in phrases {
                if contains(&normalized, &phrase) {
                    matches.push((
                        u8::from(core(&t.id)),
                        std::cmp::Reverse(normalize(&phrase).len()),
                        t.id.clone(),
                    ));
                }
            }
        }
        matches.sort();
        matches
            .first()
            .map(|(_, _, id)| pick(id, "job"))
            .unwrap_or_else(|| plain("conversation"))
    }
    pub fn instruction(&self) -> String {
        self.template.as_ref().map(|t|format!("Required answer template (policy v1): {t}. Return exactly one fenced openui JSON envelope, version 1, with template \"{t}\". No prose or other fences outside it. The application validates the completed response before showing it. Gather missing facts first; never invent data.")).unwrap_or_default()
    }
    pub fn validate(&self, response: &str) -> Result<(), String> {
        let Some(expected) = &self.template else {
            return Ok(());
        };
        if response.chars().count() > 66000 {
            return Err("Response exceeds the answer limit.".into());
        }
        let fence = re(r"^```openui(?:-lang)?[ \t]*\r?\n([\s\S]*?)\r?\n```$");
        let captures = fence
            .captures(response.trim())
            .ok_or_else(|| format!("Return only one complete openui envelope for {expected}."))?;
        let answer = crate::answer_contract::validate(&captures[1])?;
        if answer["template"] != *expected {
            return Err(format!("Expected {expected}."));
        }
        Ok(())
    }
    pub fn record(&self) -> Value {
        serde_json::to_value(self).expect("answer policy")
    }
}
