//! Answer shapes: the one set of answer-layout rules and named templates
//! every brain gets with each agent chat turn (Claude Code, Codex, Gemini,
//! Kimi, Allternit Cloud models — gizzi passes the composed block to each
//! CLI as its system prompt), so the same kind of question gets the same
//! shape whichever model answers. Plain Markdown, never forced JSON.
//!
//! The rules live in `answer_shapes.md`, the named templates in
//! `answer_templates.json` (rendered into the rules, and used by the
//! per-turn template picker); the length line comes from the user's
//! Response Style. allternit-ai sends the same block itself (it also covers
//! turns that skip this bridge, like Code mode's local gizzi), so the bridge
//! adds its copy only when the client prompt doesn't already carry one.
//! Both files are byte-identical to allternit-ai `src/lib/openui/` copies;
//! the tests pin their SHA-256 in both repos — change both together.

use serde::Deserialize;
use std::sync::OnceLock;

const SHAPES_MD: &str = include_str!("answer_shapes.md");
const TEMPLATES_JSON: &str = include_str!("answer_templates.json");

/// The heading that opens the block; how a client-sent copy is detected.
pub(crate) const ANSWER_SHAPES_MARKER: &str = "## Answer shape\n";

/// The longest stretch of a message the picker reads (normalized, ASCII).
const PICK_MAX_CHARS: usize = 4000;

#[derive(Debug, Deserialize)]
pub(crate) struct AnswerTemplate {
    pub id: String,
    pub name: String,
    pub when: String,
    pub priority: u32,
    pub triggers: Vec<String>,
    pub sections: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// An OpenUI card that can carry the interactive part, if any.
    #[serde(default)]
    pub card: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Registry {
    fallback: String,
    templates: Vec<AnswerTemplate>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        serde_json::from_str(TEMPLATES_JSON).expect("answer_templates.json is valid")
    })
}

/// The named templates, in display order.
pub(crate) fn templates() -> &'static [AnswerTemplate] {
    &registry().templates
}

/// What a template asks for: its section headings, or its note.
fn outline(t: &AnswerTemplate) -> String {
    if t.sections.is_empty() {
        t.note.clone().unwrap_or_default()
    } else {
        t.sections.join(", ")
    }
}

/// How long a reply is, per Response Style. Also STYLE.md's directive.
pub(crate) fn length_rule(style: &str) -> &'static str {
    match style {
        "concise" => "Length (Concise): the bottom line plus at most 3 bullets. Use a template's section names as bold lead-ins instead of headings, and skip the title and dateline.",
        "detailed" => "Length (Detailed): the full template, with reasoning, examples and edge cases inside each section.",
        "custom" => "Length: follow the user's custom instructions; where they say nothing, the bottom line then the template's sections, each a few lines.",
        _ => "Length (Balanced): the bottom line, then the template's sections, each a few lines.",
    }
}

/// The full answer-shape block for a Response Style.
pub(crate) fn answer_shapes_block(style: &str) -> String {
    let lines: Vec<String> = templates()
        .iter()
        .map(|t| format!("- {} ({}): {}.", t.name, t.when, outline(t)))
        .collect();
    SHAPES_MD
        .trim_end()
        .replace("{{LENGTH}}", length_rule(style))
        .replace("{{TEMPLATES}}", &lines.join("\n"))
}

/// True when a client-sent prompt already carries the answer-shape block.
pub(crate) fn has_answer_shapes(prompt: Option<&str>) -> bool {
    prompt.is_some_and(|p| {
        p.starts_with(ANSWER_SHAPES_MARKER) || p.contains(&format!("\n{ANSWER_SHAPES_MARKER}"))
    })
}

/// Lowercase ASCII words separated by single spaces, padded with a space on
/// each side so a phrase matches only whole words. Same as allternit-ai's
/// `normalizeForPick`.
fn normalize(text: &str) -> String {
    let mut out = String::from(" ");
    for c in text.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
        if out.len() > PICK_MAX_CHARS {
            break;
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// The template for a user message: the lowest-priority template with a
/// trigger phrase in it (whole words, case-insensitive), else the fallback.
/// Deterministic and cheap: no model call.
pub(crate) fn pick_template(message: &str) -> &'static AnswerTemplate {
    let text = normalize(message);
    let reg = registry();
    let mut by_priority: Vec<&AnswerTemplate> = reg.templates.iter().collect();
    by_priority.sort_by_key(|t| t.priority);
    by_priority
        .into_iter()
        .find(|t| {
            t.triggers
                .iter()
                .any(|phrase| text.contains(&normalize(phrase)))
        })
        .or_else(|| reg.templates.iter().find(|t| t.id == reg.fallback))
        .expect("the fallback template exists")
}

/// The one line that names this turn's template. `cards` adds the template's
/// card suggestion (only when the card manual rides along).
pub(crate) fn template_line(t: &AnswerTemplate, cards: bool) -> String {
    let mut line = format!("Use answer template: {} ({}).", t.name, outline(t));
    if let (true, Some(card)) = (cards, &t.card) {
        line.push_str(&format!(
            " If the user has to pick or adjust, a {card} card can hold that part."
        ));
    }
    line
}

/// The template line for a bridge turn, or None for an empty message.
/// Cards are on when the client sent the interactive-answers manual.
pub(crate) fn turn_template_line(message: &str, client_prompt: Option<&str>) -> Option<String> {
    if message.trim().is_empty() {
        return None;
    }
    let cards = client_prompt.is_some_and(|p| p.contains("tagged `openui`"));
    Some(template_line(pick_template(message), cards))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// SHA-256 of the two shared files. allternit-ai's answer-shapes.test.ts
    /// pins the same values: change a file in both repos, then both pins.
    const SHAPES_MD_SHA256: &str =
        "1ad471125703864897a5d723cc11ff462621e3ee4b87d486c2f29f280e85acb3";
    const TEMPLATES_JSON_SHA256: &str =
        "a92ff59eff6128c314c65d224ac64016a7134b404849d0bb3c1d69fd20dbdd7f";

    fn sha(text: &str) -> String {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn shared_files_match_the_app_copies() {
        assert_eq!(
            sha(SHAPES_MD),
            SHAPES_MD_SHA256,
            "answer_shapes.md changed: update allternit-ai's copy and both pins"
        );
        assert_eq!(
            sha(TEMPLATES_JSON),
            TEMPLATES_JSON_SHA256,
            "answer_templates.json changed: update allternit-ai's copy and both pins"
        );
    }

    #[test]
    fn block_has_rules_templates_and_one_length_line_per_style() {
        for style in ["concise", "balanced", "detailed", "custom"] {
            let block = answer_shapes_block(style);
            assert!(block.starts_with(ANSWER_SHAPES_MARKER));
            assert!(block.contains("Open with the bottom line"));
            assert!(block.contains("Don't narrate your work"));
            assert!(block.contains("[title](url)"));
            assert!(block.contains("Where an answer goes"));
            assert!(block.contains(length_rule(style)));
            assert!(!block.contains("{{"));
            for t in templates() {
                assert!(
                    block.contains(&format!("- {} (", t.name)),
                    "{style}: missing {}",
                    t.name
                );
            }
        }
        let block = answer_shapes_block("balanced");
        assert!(block.contains("- Comparison (\"A vs B\", \"best X for…\"): Criteria, Comparison (table, one row per option), Recommendation, What would change my choice, Next step."));
        assert!(block.contains("- Quick answer (a fact, yes/no, a definition): the bottom line, then up to 3 bullets. No headings."));
        assert!(answer_shapes_block("concise").contains("at most 3 bullets"));
        assert!(answer_shapes_block("detailed").contains("full template"));
        // Unknown styles get Balanced.
        assert_eq!(
            answer_shapes_block("weird"),
            answer_shapes_block("balanced")
        );
    }

    #[test]
    fn block_stays_lean() {
        // ~4 chars per token: keep the block under ~600 tokens.
        assert!(answer_shapes_block("concise").len() < 2400);
    }

    #[test]
    fn detects_client_copy() {
        assert!(has_answer_shapes(Some(&answer_shapes_block("balanced"))));
        assert!(has_answer_shapes(Some(&format!(
            "intro\n\n{}",
            answer_shapes_block("concise")
        ))));
        assert!(!has_answer_shapes(Some("## Answer shapes are nice")));
        assert!(!has_answer_shapes(None));
    }

    #[test]
    fn picker_cases() {
        let cases = [
            ("Compare Postgres and MySQL for a small app", "comparison"),
            ("React vs Vue?", "comparison"),
            ("Which is better for me, the M4 or the M5?", "comparison"),
            ("Draft a rollout plan for the new pricing", "plan"),
            ("What are the steps to launch?", "plan"),
            ("How do I reset my password?", "how-to"),
            ("how to export a PDF", "how-to"),
            ("Why is the build broken?", "troubleshooting"),
            ("I get an ERROR: EACCES when I run it", "troubleshooting"),
            ("The upload doesn't work", "troubleshooting"),
            ("Status of the Windows release?", "status-report"),
            ("Give me an update on the migration", "status-report"),
            (
                "Research the latest on solid-state batteries",
                "research-brief",
            ),
            (
                "Can you look into who makes the best e-bikes",
                "research-brief",
            ),
            ("Should I take the job offer?", "decision"),
            ("What are my options here", "decision"),
            ("What's the capital of Peru?", "quick-answer"),
            ("", "quick-answer"),
            // Whole words only: "plant" is not "plan", "previous" is not "vs".
            ("Which plant needs the least light", "quick-answer"),
            ("the previous one", "quick-answer"),
            // Priority: comparison beats decision, how-to beats troubleshooting.
            ("Should I pick A vs B?", "comparison"),
            ("How do I fix this error?", "how-to"),
        ];
        for (message, want) in cases {
            assert_eq!(pick_template(message).id, want, "{message:?}");
        }
    }

    #[test]
    fn template_line_names_one_template() {
        let cmp = pick_template("A vs B");
        assert_eq!(
            template_line(cmp, false),
            "Use answer template: Comparison (Criteria, Comparison (table, one row per option), Recommendation, What would change my choice, Next step)."
        );
        assert!(template_line(cmp, true).ends_with("a Compare card can hold that part."));
        assert!(!template_line(pick_template("hi"), true).contains("card"));
        assert_eq!(turn_template_line("  ", None), None);
        assert!(
            turn_template_line("plan my week", Some("x tagged `openui` y"))
                .unwrap()
                .contains("Checklist card")
        );
        assert!(!turn_template_line("plan my week", Some("no cards"))
            .unwrap()
            .contains("card"));
    }
}
