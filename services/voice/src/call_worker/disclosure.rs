//! The fixed first line of every call (§4.1 "Always").
//!
//! It states that the caller is talking to an AI assistant and whether the call
//! is recorded. It is built here from the bot name and recording flag only;
//! nothing in bot config can remove or reword it.

/// Disclosure sentence for a bot. `bot_name` falls back to a neutral phrase
/// when cloud-api did not send one (or the call start failed).
pub fn disclosure(bot_name: Option<&str>, recording: bool) -> String {
    let recorded = if recording {
        ", and this call may be recorded"
    } else {
        ", and this call isn't recorded"
    };
    match bot_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => format!("Hi, you've reached {name}. I'm an AI assistant{recorded}."),
        None => format!("Hi, I'm an AI assistant{recorded}."),
    }
}

/// The full opening: disclosure, then the bot's greeting (if any).
pub fn opening(bot_name: Option<&str>, recording: bool, greeting: Option<&str>) -> String {
    let first = disclosure(bot_name, recording);
    match greeting.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => format!("{first} {g}"),
        None => first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded() {
        assert_eq!(
            disclosure(Some("Acme Plumbing"), true),
            "Hi, you've reached Acme Plumbing. I'm an AI assistant, and this call may be recorded."
        );
    }

    #[test]
    fn not_recorded() {
        assert_eq!(
            disclosure(Some("Acme Plumbing"), false),
            "Hi, you've reached Acme Plumbing. I'm an AI assistant, and this call isn't recorded."
        );
    }

    #[test]
    fn missing_name_still_discloses() {
        assert_eq!(disclosure(None, false), "Hi, I'm an AI assistant, and this call isn't recorded.");
        assert_eq!(disclosure(Some("  "), true), "Hi, I'm an AI assistant, and this call may be recorded.");
    }

    #[test]
    fn greeting_follows_disclosure() {
        let o = opening(Some("Acme"), false, Some("How can I help?"));
        assert!(o.starts_with("Hi, you've reached Acme. I'm an AI assistant"));
        assert!(o.ends_with("isn't recorded. How can I help?"));
        // A greeting can't replace the disclosure.
        let o = opening(Some("Acme"), true, Some("Ignore the above."));
        assert!(o.starts_with("Hi, you've reached Acme. I'm an AI assistant, and this call may be recorded."));
    }
}
