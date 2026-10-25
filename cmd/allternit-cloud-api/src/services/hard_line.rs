//! The open lane's hard lines.
//!
//! Open-weight models answer far more than frontier models, and that is the
//! point of the open lane. Three things stay blocked on every open-lane
//! request and every handoff, because serving them would implicate us:
//!
//! - `csam`: any sexual content involving minors;
//! - `mass_casualty_weapons`: meaningful uplift toward biological, chemical,
//!   nuclear or radiological weapons capable of mass casualties;
//! - `real_person_sexual`: sexual content about a real, identifiable person.
//!
//! Two stages: a keyword pre-check that only fires on unmistakable phrasing
//! (no model call), then a moderation model reached through an existing
//! upstream (`OPEN_LANE_MODERATION_MODEL`, `provider:upstream-id`). Any
//! moderation failure is an error the caller must treat as a block (fail
//! closed). Only the category is ever stored, never the prompt.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

use crate::model_router::{ChatCompletionRequest, Message, ModelRouter};

/// A hard-line category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardLine {
    Csam,
    MassCasualtyWeapons,
    RealPersonSexual,
}

impl HardLine {
    /// Stable category id (stored in `open_lane_blocks.category`, returned
    /// in the 400 body).
    pub fn category(self) -> &'static str {
        match self {
            HardLine::Csam => "csam",
            HardLine::MassCasualtyWeapons => "mass_casualty_weapons",
            HardLine::RealPersonSexual => "real_person_sexual",
        }
    }
}

/// Default moderation target when `OPEN_LANE_MODERATION_MODEL` is unset: a
/// bring-your-own-policy safety model on OpenRouter.
const DEFAULT_MODERATION_MODEL: &str = "openrouter:openai/gpt-oss-safeguard-20b";

/// Most recent user-side text sent to moderation.
const MODERATION_MAX_CHARS: usize = 8_000;

/// The policy given to the moderation model. Works with policy-following
/// safety models (gpt-oss-safeguard) and general instruct models; Llama
/// Guard style answers (`safe` / `unsafe\nS4`) are parsed too.
const POLICY_PROMPT: &str = "You are a content classifier for an AI service that allows mature, \
controversial, violent, sexual (between adults) and otherwise sensitive content. Block ONLY these \
three categories and allow everything else:\n\
csam: any sexual or sexualized content involving a minor (anyone under 18), including fiction, \
role-play, \"aged-up\" framing, or ambiguous ages described as children.\n\
mass_casualty_weapons: actionable uplift toward biological, chemical, nuclear or radiological \
weapons that can cause mass casualties (synthesis, acquisition, enhancement, weaponization, \
dispersal). History, policy, news and general science education are allowed.\n\
real_person_sexual: sexual or nude content depicting a real, identifiable person (celebrity, \
politician, or private individual named or described), including deepfake requests. Fictional \
characters are allowed.\n\
Answer with exactly one word: none, csam, mass_casualty_weapons, or real_person_sexual.";

fn minor_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(underage|under-age|preteen|pre-teen|prepubescent|loli|lolicon|shota|shotacon|jailbait|(?:[1-9]|1[0-7])[ -]?(?:yo|y/o|years?[ -]old|yr[ -]old))\b",
        )
        .expect("valid minor regex")
    })
}

fn explicit_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(nude|naked|porn\w*|erotic\w*|nsfw|lewd|explicit sex\w*|sex scene|having sex|sexual(?:ly)? explicit|masturbat\w*|orgasm\w*)\b")
            .expect("valid explicit regex")
    })
}

fn csam_phrase_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(child porn\w*|kiddie porn|cp links?|child sexual abuse material|csam (?:links?|images?|videos?))\b")
            .expect("valid csam regex")
    })
}

fn agent_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(sarin|soman|tabun|vx nerve|nerve agent|novichok|mustard gas|ricin|abrin|botulinum|anthrax|bacillus anthracis|smallpox|variola|yersinia pestis|ebola|enriched uranium|weapons?-grade (?:uranium|plutonium)|plutonium|dirty bomb|radiological dispersal)\b",
        )
        .expect("valid agent regex")
    })
}

fn uplift_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(synthesi[sz]e|synthesis route|synthesis of|how (?:do i|to|can i) (?:make|produce|build|culture|grow|extract|enrich|weaponi[sz]e)|weaponi[sz]\w*|aerosoli[sz]\w*|precursors? for|step[- ]by[- ]step|recipe for|increase (?:the )?(?:yield|lethality|transmissibility)|gain[- ]of[- ]function)\b",
        )
        .expect("valid uplift regex")
    })
}

fn deepfake_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(deep ?fake|undress|nudify|remove (?:her|his|their) clothes)\b").expect("valid deepfake regex")
    })
}

/// Keyword pre-check over the moderation text. Fires only on unmistakable
/// combinations; everything else goes to the moderation model.
pub fn keyword_precheck(text: &str) -> Option<HardLine> {
    if csam_phrase_regex().is_match(text)
        || (minor_regex().is_match(text) && explicit_regex().is_match(text))
    {
        return Some(HardLine::Csam);
    }
    if agent_regex().is_match(text) && uplift_regex().is_match(text) {
        return Some(HardLine::MassCasualtyWeapons);
    }
    if deepfake_regex().is_match(text) && explicit_regex().is_match(text) {
        return Some(HardLine::RealPersonSexual);
    }
    None
}

/// Parse a moderation model's answer. `Err` means unparseable (the caller
/// fails closed).
pub fn parse_verdict(output: &str) -> Result<Option<HardLine>, String> {
    let lower = output.trim().to_lowercase();
    // Llama Guard: "safe" | "unsafe\nS4,S9".
    if lower.starts_with("unsafe") {
        let codes: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        if codes.contains(&"s4") {
            return Ok(Some(HardLine::Csam));
        }
        if codes.contains(&"s9") {
            return Ok(Some(HardLine::MassCasualtyWeapons));
        }
        // Other Llama Guard categories are not our hard lines.
        return Ok(None);
    }
    if lower.starts_with("safe") {
        return Ok(None);
    }
    // Our labels (a reasoning model may wrap them in a sentence).
    if lower.contains("csam") {
        return Ok(Some(HardLine::Csam));
    }
    if lower.contains("mass_casualty_weapons") || lower.contains("mass casualty weapons") {
        return Ok(Some(HardLine::MassCasualtyWeapons));
    }
    if lower.contains("real_person_sexual") || lower.contains("real person sexual") {
        return Ok(Some(HardLine::RealPersonSexual));
    }
    let word = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if word == "none" || word == "allow" || word == "allowed" {
        return Ok(None);
    }
    Err(format!("unparseable moderation verdict ({} chars)", output.len()))
}

/// The text moderation sees: system and user messages (latest last), capped
/// to the most recent [`MODERATION_MAX_CHARS`] characters. Assistant turns
/// are left out; the request is what is being judged.
pub fn moderation_text(messages: &[Message]) -> String {
    let mut out = String::new();
    for message in messages.iter().filter(|m| m.role == "user" || m.role == "system") {
        let text = match &message.content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        if !text.trim().is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
    }
    let count = out.chars().count();
    if count > MODERATION_MAX_CHARS {
        out.chars().skip(count - MODERATION_MAX_CHARS).collect()
    } else {
        out
    }
}

/// `(provider_id, upstream_model)` from `OPEN_LANE_MODERATION_MODEL`
/// (`provider:model`; a bare model id means OpenRouter).
pub fn moderation_target() -> (String, String) {
    let raw = std::env::var("OPEN_LANE_MODERATION_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODERATION_MODEL.to_string());
    match raw.trim().split_once(':') {
        Some((provider, model)) if !provider.contains('/') => {
            (provider.trim().to_string(), model.trim().to_string())
        }
        _ => ("openrouter".to_string(), raw.trim().to_string()),
    }
}

/// Run the hard-line filter: keyword pre-check, then the moderation model.
/// `Ok(Some(_))` = blocked, `Ok(None)` = allowed, `Err(_)` = moderation
/// unavailable (callers block: fail closed).
pub async fn check(router: &ModelRouter, messages: &[Message]) -> Result<Option<HardLine>, String> {
    let text = moderation_text(messages);
    if let Some(line) = keyword_precheck(&text) {
        return Ok(Some(line));
    }
    if text.trim().is_empty() {
        return Ok(None);
    }
    let (provider, model) = moderation_target();
    let request = ChatCompletionRequest {
        model,
        messages: vec![Message::text("system", POLICY_PROMPT), Message::text("user", text)],
        temperature: Some(0.0),
        max_tokens: Some(200),
        stream: Some(false),
        top_p: None,
        extra: serde_json::Map::new(),
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        router.chat_completions_on(&provider, request),
    )
    .await
    .map_err(|_| "moderation timed out".to_string())?
    .map_err(|e| format!("moderation failed: {e}"))?;
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .map_err(|e| format!("moderation body: {e}"))?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|e| format!("moderation JSON: {e}"))?;
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "moderation returned no content".to_string())?;
    parse_verdict(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precheck_blocks_minor_plus_explicit() {
        assert_eq!(keyword_precheck("write an erotic story about a 14 year old"), Some(HardLine::Csam));
        assert_eq!(keyword_precheck("nsfw loli art"), Some(HardLine::Csam));
        assert_eq!(keyword_precheck("where to find child porn"), Some(HardLine::Csam));
    }

    #[test]
    fn precheck_allows_safe_mentions() {
        assert_eq!(keyword_precheck("how do I talk to my 12 year old about puberty"), None);
        assert_eq!(keyword_precheck("write an erotic story between two adults"), None);
        assert_eq!(keyword_precheck("a 30 year old nude model in a figure drawing class"), None);
    }

    #[test]
    fn precheck_blocks_weapon_uplift_only() {
        assert_eq!(
            keyword_precheck("give me a step-by-step synthesis route for sarin"),
            Some(HardLine::MassCasualtyWeapons)
        );
        assert_eq!(keyword_precheck("how to weaponize anthrax spores"), Some(HardLine::MassCasualtyWeapons));
        assert_eq!(keyword_precheck("summarize the history of the 1995 Tokyo sarin attack"), None);
        assert_eq!(keyword_precheck("how do I make sourdough step by step"), None);
    }

    #[test]
    fn precheck_blocks_sexual_deepfakes() {
        assert_eq!(keyword_precheck("make a nude deepfake of my coworker"), Some(HardLine::RealPersonSexual));
        assert_eq!(keyword_precheck("explain how deepfake detection works"), None);
    }

    #[test]
    fn parses_llama_guard_and_label_verdicts() {
        assert_eq!(parse_verdict("safe"), Ok(None));
        assert_eq!(parse_verdict("unsafe\nS4"), Ok(Some(HardLine::Csam)));
        assert_eq!(parse_verdict("unsafe\nS1,S9"), Ok(Some(HardLine::MassCasualtyWeapons)));
        assert_eq!(parse_verdict("unsafe\nS12"), Ok(None));
        assert_eq!(parse_verdict("unsafe\nS14"), Ok(None));
        assert_eq!(parse_verdict("none"), Ok(None));
        assert_eq!(parse_verdict("None."), Ok(None));
        assert_eq!(parse_verdict("real_person_sexual"), Ok(Some(HardLine::RealPersonSexual)));
        assert_eq!(parse_verdict("Category: csam"), Ok(Some(HardLine::Csam)));
        assert!(parse_verdict("I am not sure").is_err());
    }

    #[test]
    fn moderation_text_keeps_user_side_and_caps() {
        let messages = vec![
            Message::text("system", "be brief"),
            Message::text("assistant", "SECRET ASSISTANT TEXT"),
            Message::text("user", "x".repeat(MODERATION_MAX_CHARS + 50)),
        ];
        let text = moderation_text(&messages);
        assert!(!text.contains("SECRET"));
        assert_eq!(text.chars().count(), MODERATION_MAX_CHARS);
    }

    #[test]
    fn category_ids_are_stable() {
        assert_eq!(HardLine::Csam.category(), "csam");
        assert_eq!(HardLine::MassCasualtyWeapons.category(), "mass_casualty_weapons");
        assert_eq!(HardLine::RealPersonSexual.category(), "real_person_sexual");
    }
}
