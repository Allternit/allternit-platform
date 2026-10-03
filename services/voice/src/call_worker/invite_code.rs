//! Outbound call that reads a phone-invite verification code aloud.
//!
//! cloud-api places it when a carrier blocks the texted code (room metadata
//! `purpose: "invite_code"`, the code in the SIP participant attribute `otp`).
//! There is no conversation: the call speaks one fixed script and hangs up.
//! The code is spoken digit by digit, twice, and is never logged or put in a
//! transcript event; recording is skipped for these calls (the room wiring
//! passes `Recording::none()`).

use std::fmt;

/// Room metadata `purpose` that selects this call type.
pub const PURPOSE: &str = "invite_code";
/// SIP participant attribute that carries the code.
pub const OTP_ATTR: &str = "otp";
/// What the call's transcript says instead of the code.
pub const REDACTED_TRANSCRIPT: &str = "verification code read";
/// `call.ended` reason when the code was read (or left on a machine).
pub const END_READ: &str = "bot_hangup";
/// `call.ended` reason when the `otp` was missing or invalid.
pub const END_FAILED: &str = "failed";

const MIN_DIGITS: usize = 4;
const MAX_DIGITS: usize = 10;

/// The code for one call. `Debug` never shows the digits.
#[derive(Clone, PartialEq, Eq)]
pub struct InviteCall {
    code: Option<String>,
}

impl fmt::Debug for InviteCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteCall").field("code", &self.code.as_ref().map(|_| "<redacted>")).finish()
    }
}

impl InviteCall {
    /// From the `otp` attribute. Anything but 4–10 ASCII digits is invalid.
    pub fn from_attr(otp: Option<&str>) -> Self {
        let code = otp.map(str::trim).filter(|c| (MIN_DIGITS..=MAX_DIGITS).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit()));
        Self { code: code.map(String::from) }
    }

    pub fn is_valid(&self) -> bool {
        self.code.is_some()
    }

    pub fn end_reason(&self) -> &'static str {
        if self.is_valid() {
            END_READ
        } else {
            END_FAILED
        }
    }

    /// The whole call, as one utterance for the speak path. Full stops between
    /// digits make the voice pause on each one; the ellipsis is the longer
    /// pause before the repeat.
    pub fn script(&self, bot_name: Option<&str>) -> String {
        let name = bot_name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Allternit");
        let hello = format!("Hi, this is an automated call from {name}, an AI assistant.");
        match &self.code {
            Some(code) => {
                let digits = spoken_digits(code);
                format!("{hello} This is {name}'s verification code: {digits} ... Again, your code is {digits} Goodbye.")
            }
            None => format!("{hello} Sorry, I can't read your verification code right now. Please request a new one. Goodbye."),
        }
    }
}

/// `"427193"` → `"4. 2. 7. 1. 9. 3."`
fn spoken_digits(code: &str) -> String {
    code.chars().map(|c| format!("{c}.")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_reads_the_digits_twice() {
        let c = InviteCall::from_attr(Some("427193"));
        assert_eq!(
            c.script(Some("Mia")),
            "Hi, this is an automated call from Mia, an AI assistant. This is Mia's verification code: \
             4. 2. 7. 1. 9. 3. ... Again, your code is 4. 2. 7. 1. 9. 3. Goodbye."
        );
        assert_eq!(c.end_reason(), "bot_hangup");
    }

    #[test]
    fn missing_bot_name_falls_back() {
        let c = InviteCall::from_attr(Some("1234"));
        assert!(c.script(None).starts_with("Hi, this is an automated call from Allternit, an AI assistant."));
        assert!(c.script(Some("  ")).contains("Allternit's verification code: 1. 2. 3. 4."));
    }

    #[test]
    fn invalid_codes_apologize_without_a_code() {
        for bad in [None, Some(""), Some("123"), Some("12345678901"), Some("12a456"), Some("12 34")] {
            let c = InviteCall::from_attr(bad);
            assert!(!c.is_valid(), "{bad:?}");
            assert_eq!(c.end_reason(), "failed");
            let s = c.script(Some("Mia"));
            assert!(s.contains("Sorry") && s.starts_with("Hi, this is an automated call from Mia, an AI assistant."));
            assert!(!s.chars().any(|ch| ch.is_ascii_digit()));
        }
    }

    #[test]
    fn debug_hides_the_code() {
        let c = InviteCall::from_attr(Some("427193"));
        let d = format!("{c:?}");
        assert!(!d.contains("427193") && d.contains("redacted"));
    }
}
