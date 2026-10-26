//! Interactive answers: feeding a broken card's check back to the model.
//!
//! At the end of a chat turn the bridge checks every ```openui card the reply wrote
//! (`openui_text::check_reply`). The errors used to be stored and shown only; the model never
//! learned its card failed, so the next card often failed the same way.
//!
//! A silent repair turn inside the same stream isn't workable there: the reply's text has
//! already streamed to the app, a second gizzi prompt would leave a synthetic user message and a
//! duplicate reply in the session history, and the app has no frame that swaps a card in place.
//! So the errors ride along with the NEXT turn instead: [`queue`] keeps a short repair note per
//! chat, [`take`] hands it to the next send exactly once (as gizzi's turn-only `turnSystem`
//! layer — never stored on the user message), and the model fixes the card if the conversation
//! still needs it.
//!
//! Bounded to one retry: a turn that already carried a repair note never queues another, even
//! if its own cards fail too. Only card syntax / catalog errors are fed back (that is all
//! `check_card` reports). `ALLTERNIT_CARD_REPAIR=0|off|false` turns it off (default on).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::openui_text::check_reply;

/// Most failing cards one note describes.
const MAX_CARDS: usize = 2;
/// Most characters of a failing card's code quoted back to the model.
const MAX_CODE_CHARS: usize = 1500;
/// Most notes held at once (one per chat); the oldest is dropped past this.
const MAX_PENDING: usize = 512;

/// Whether broken cards are fed back to the model (default on).
pub fn enabled() -> bool {
    flag_on(std::env::var("ALLTERNIT_CARD_REPAIR").ok().as_deref())
}

fn flag_on(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("0" | "off" | "false" | "no"))
}

/// The repair note for a finished reply's card texts, or None when every card passed.
pub fn repair_note<'a>(texts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut failing: Vec<(Vec<String>, String)> = Vec::new();
    for text in texts {
        for check in check_reply(text) {
            if !check.diagnostics.is_empty() {
                failing.push((check.diagnostics, check.code));
            }
        }
    }
    if failing.is_empty() {
        return None;
    }
    let total = failing.len();
    let mut note = String::from(
        "Card check: your previous reply's interactive card (```openui) failed the check, so the user saw a plain-text fallback instead of the card.\n",
    );
    for (i, (diagnostics, code)) in failing.iter().take(MAX_CARDS).enumerate() {
        note.push_str(&format!("\nCard {}: {}\n", i + 1, diagnostics.join(" ")));
        note.push_str("```openui\n");
        note.push_str(&clip(code.trim_end(), MAX_CODE_CHARS));
        note.push_str("\n```\n");
    }
    if total > MAX_CARDS {
        note.push_str(&format!("\n({} more card(s) also failed.)\n", total - MAX_CARDS));
    }
    note.push_str(
        "\nIf this message still concerns that answer, start your reply with the corrected card: a full ```openui fence, a `root = Answer(...)` line, only catalog components with their required values, and every name defined. Otherwise apply these fixes to any card you write. Don't mention this note.",
    );
    Some(note)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n…");
    out
}

/// Whether a finished turn should queue a note: only when it failed and it wasn't itself the
/// retry (one repair attempt per broken card, never a loop).
pub fn should_queue(turn_carried_note: bool, note: &Option<String>) -> bool {
    !turn_carried_note && note.is_some()
}

struct Pending {
    notes: HashMap<String, (u64, String)>,
    seq: u64,
}

fn pending() -> &'static Mutex<Pending> {
    static PENDING: std::sync::OnceLock<Mutex<Pending>> = std::sync::OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Pending { notes: HashMap::new(), seq: 0 }))
}

fn key(user_id: &str, chat_id: &str) -> String {
    format!("{user_id}\u{1f}{chat_id}")
}

/// Keeps `note` for the chat's next send (replacing an older one).
pub fn queue(user_id: &str, chat_id: &str, note: String) {
    let Ok(mut p) = pending().lock() else { return };
    p.seq += 1;
    let seq = p.seq;
    p.notes.insert(key(user_id, chat_id), (seq, note));
    if p.notes.len() > MAX_PENDING {
        if let Some(oldest) = p.notes.iter().min_by_key(|(_, (s, _))| *s).map(|(k, _)| k.clone()) {
            p.notes.remove(&oldest);
        }
    }
}

/// The chat's pending note, removed so it is sent once.
pub fn take(user_id: &str, chat_id: &str) -> Option<String> {
    pending().lock().ok()?.notes.remove(&key(user_id, chat_id)).map(|(_, note)| note)
}

/// gizzi's turn-only system layer from its parts (location, repair note…), or None if all empty.
pub fn join_turn_layers(layers: &[Option<&str>]) -> Option<String> {
    let parts: Vec<&str> = layers.iter().flatten().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "Here.\n```openui\nroot = Answer(\"One\", [Text(\"x\")])\n```";
    const BAD: &str = "Here.\n```openui\nroot = Answer(\"Two\", [Nope(), missing])\n```\nDone.";

    #[test]
    fn flag_defaults_on() {
        assert!(flag_on(None));
        assert!(flag_on(Some("1")));
        assert!(flag_on(Some("")));
        assert!(!flag_on(Some("0")));
        assert!(!flag_on(Some(" OFF ")));
        assert!(!flag_on(Some("false")));
    }

    #[test]
    fn passing_cards_make_no_note() {
        assert_eq!(repair_note([GOOD]), None);
        assert_eq!(repair_note(["No card at all."]), None);
        assert_eq!(repair_note(std::iter::empty::<&str>()), None);
    }

    #[test]
    fn failing_cards_are_quoted_with_their_errors() {
        let note = repair_note([GOOD, BAD]).expect("a note");
        assert!(note.contains("Unknown component Nope."), "{note}");
        assert!(note.contains("Never defined: missing."), "{note}");
        assert!(note.contains("root = Answer(\"Two\""), "{note}");
        assert!(!note.contains("Answer(\"One\""), "passing card stays out: {note}");
        assert!(note.contains("Don't mention this note."));
    }

    #[test]
    fn note_is_bounded() {
        let many = format!("{BAD}\n{BAD}\n{BAD}");
        let note = repair_note([many.as_str()]).unwrap();
        assert_eq!(note.matches("```openui\n").count(), MAX_CARDS);
        assert!(note.contains("1 more card(s)"));
        let long = format!("```openui\nroot = Answer(\"L\", [Nope(\"{}\")])\n```", "x".repeat(4000));
        let note = repair_note([long.as_str()]).unwrap();
        assert!(note.len() < MAX_CODE_CHARS + 1200, "{}", note.len());
        assert!(note.contains('…'));
    }

    #[test]
    fn one_retry_only() {
        let note = Some("n".to_string());
        assert!(should_queue(false, &note));
        assert!(!should_queue(true, &note), "the retry turn never queues another");
        assert!(!should_queue(false, &None));
    }

    #[test]
    fn notes_are_taken_once_per_chat() {
        queue("u_repair", "c1", "first".into());
        queue("u_repair", "c1", "second".into());
        queue("u_repair", "c2", "other".into());
        assert_eq!(take("u_repair", "c1").as_deref(), Some("second"));
        assert_eq!(take("u_repair", "c1"), None);
        assert_eq!(take("u_other", "c2"), None, "keyed by user too");
        assert_eq!(take("u_repair", "c2").as_deref(), Some("other"));
    }

    #[test]
    fn turn_layers_join() {
        assert_eq!(join_turn_layers(&[None, Some("  ")]), None);
        assert_eq!(join_turn_layers(&[Some("loc"), None]).as_deref(), Some("loc"));
        assert_eq!(join_turn_layers(&[Some("loc"), Some("fix")]).as_deref(), Some("loc\n\nfix"));
    }
}
