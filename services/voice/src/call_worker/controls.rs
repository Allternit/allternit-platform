//! Call controls from the phone UI (§4.1, incl. the 2026-10-02 additions).
//!
//! UI → cloud-api `POST /api/v1/voice/calls/{callId}/control` → LiveKit data
//! packet on topic `allternit.call.control` → [`parse_control`] → the call
//! applies it and acks with `call.state.changed` (or the matching event).

use serde::Deserialize;

use super::events::{CallEvent, Speaker, TransferMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Bot,
    Caller,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Hangup,
    Mute(Target),
    Unmute(Target),
    Hold,
    Resume,
    Dtmf(String),
    /// `consent_ref` is the consent gate's reference for dialing `to`; only a
    /// warm transfer (which dials out) needs it.
    Transfer { to: String, mode: TransferMode, consent_ref: Option<String> },
    /// A human joins (cloud-api minted their publish token); the bot goes quiet
    /// but keeps transcribing.
    Takeover { by: String },
    /// End the takeover; the bot resumes with the same transcript context.
    Release { by: String },
    /// Someone is listening in (receive-only token minted by cloud-api). No
    /// change on the worker side beyond the ack.
    Listen,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    #[error("control is not valid JSON: {0}")]
    Json(String),
    #[error("unknown control action `{0}`")]
    UnknownAction(String),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Deserialize)]
struct Wire {
    action: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    digits: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    /// Who sent it (owner user id), filled by cloud-api. Proposed field.
    #[serde(default)]
    by: Option<String>,
    /// The consent gate's reference for dialing `to` (warm transfer). Filled by
    /// cloud-api from `consent_ref_for`.
    #[serde(default, rename = "consentRef")]
    consent_ref: Option<String>,
}

const MAX_DTMF: usize = 32;

pub fn parse_control(bytes: &[u8]) -> Result<Control, ControlError> {
    let w: Wire = serde_json::from_slice(bytes).map_err(|e| ControlError::Json(e.to_string()))?;
    let target = || -> Result<Target, ControlError> {
        match w.target.as_deref() {
            // A phone screen's mute button mutes the bot's voice by default.
            None | Some("bot") => Ok(Target::Bot),
            Some("caller") => Ok(Target::Caller),
            Some(t) => Err(ControlError::Invalid(format!("unknown mute target `{t}`"))),
        }
    };
    let by = || w.by.clone().filter(|b| !b.is_empty()).unwrap_or_else(|| "owner".into());
    Ok(match w.action.as_str() {
        "hangup" => Control::Hangup,
        "mute" => Control::Mute(target()?),
        "unmute" => Control::Unmute(target()?),
        "hold" => Control::Hold,
        "resume" => Control::Resume,
        "dtmf" => {
            let d = w.digits.clone().unwrap_or_default();
            if d.is_empty() || d.len() > MAX_DTMF || !d.chars().all(|c| dtmf_code(c).is_some()) {
                return Err(ControlError::Invalid(format!("invalid dtmf digits `{d}`")));
            }
            Control::Dtmf(d)
        }
        "transfer" => {
            let to = w.to.clone().unwrap_or_default();
            if !is_transfer_target(&to) {
                return Err(ControlError::Invalid(format!("invalid transfer target `{to}`")));
            }
            let mode = match w.mode.as_deref() {
                None | Some("cold") => TransferMode::Cold,
                Some("warm") => TransferMode::Warm,
                Some(m) => return Err(ControlError::Invalid(format!("unknown transfer mode `{m}`"))),
            };
            Control::Transfer { to, mode, consent_ref: w.consent_ref.clone().filter(|c| !c.trim().is_empty()) }
        }
        "takeover" => Control::Takeover { by: by() },
        "release" => Control::Release { by: by() },
        "listen" => Control::Listen,
        other => return Err(ControlError::UnknownAction(other.to_string())),
    })
}

/// RFC 4733 event code for a DTMF character.
pub fn dtmf_code(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(c as u32 - '0' as u32),
        '*' => Some(10),
        '#' => Some(11),
        'A'..='D' => Some(12 + (c as u32 - 'A' as u32)),
        'a'..='d' => Some(12 + (c as u32 - 'a' as u32)),
        _ => None,
    }
}

/// E.164 (`+` and 7–15 digits) or a `sip:`/`tel:` URI.
pub fn is_transfer_target(to: &str) -> bool {
    if let Some(d) = to.strip_prefix('+') {
        return (7..=15).contains(&d.len())
            && d.chars().all(|c| c.is_ascii_digit())
            && !d.starts_with('0');
    }
    (to.starts_with("sip:") || to.starts_with("tel:+")) && to.len() > 6 && !to.contains(char::is_whitespace)
}

/// `tel:` URI LiveKit's TransferSIPParticipant expects for an E.164 number.
pub fn transfer_uri(to: &str) -> String {
    if to.starts_with('+') {
        format!("tel:{to}")
    } else {
        to.to_string()
    }
}

/// Live call state; `call.state.changed` reports it after every control.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallState {
    pub held: bool,
    pub muted_bot: bool,
    pub muted_caller: bool,
    /// Who took over, while a human is on the call.
    pub takeover_by: Option<String>,
    pub speaker: Option<Speaker>,
    /// The call is actually being recorded (not just configured to be).
    pub recording: bool,
}

impl CallState {
    /// The bot may speak and run turns.
    pub fn bot_active(&self) -> bool {
        !self.held && self.takeover_by.is_none()
    }

    pub fn state_event(&self) -> CallEvent {
        CallEvent::StateChanged {
            held: self.held,
            muted_bot: self.muted_bot,
            muted_caller: self.muted_caller,
            speaker: self.speaker,
            recording: self.recording,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Control, ControlError> {
        parse_control(s.as_bytes())
    }

    #[test]
    fn parses_every_action() {
        assert_eq!(p(r#"{"action":"hangup"}"#), Ok(Control::Hangup));
        assert_eq!(p(r#"{"action":"mute"}"#), Ok(Control::Mute(Target::Bot)));
        assert_eq!(p(r#"{"action":"mute","target":"caller"}"#), Ok(Control::Mute(Target::Caller)));
        assert_eq!(p(r#"{"action":"unmute","target":"bot"}"#), Ok(Control::Unmute(Target::Bot)));
        assert_eq!(p(r#"{"action":"hold"}"#), Ok(Control::Hold));
        assert_eq!(p(r#"{"action":"resume"}"#), Ok(Control::Resume));
        assert_eq!(p(r#"{"action":"dtmf","digits":"12#*"}"#), Ok(Control::Dtmf("12#*".into())));
        assert_eq!(
            p(r#"{"action":"transfer","to":"+15105550100"}"#),
            Ok(Control::Transfer { to: "+15105550100".into(), mode: TransferMode::Cold, consent_ref: None })
        );
        assert_eq!(
            p(r#"{"action":"transfer","to":"sip:desk@pbx.example","mode":"warm"}"#),
            Ok(Control::Transfer {
                to: "sip:desk@pbx.example".into(),
                mode: TransferMode::Warm,
                consent_ref: None
            })
        );
        assert_eq!(
            p(r#"{"action":"transfer","to":"+15105550100","mode":"warm","consentRef":"cc_1"}"#),
            Ok(Control::Transfer { to: "+15105550100".into(), mode: TransferMode::Warm, consent_ref: Some("cc_1".into()) })
        );
        assert_eq!(p(r#"{"action":"takeover","by":"user_1"}"#), Ok(Control::Takeover { by: "user_1".into() }));
        assert_eq!(p(r#"{"action":"release"}"#), Ok(Control::Release { by: "owner".into() }));
        assert_eq!(p(r#"{"action":"listen","extra":1}"#), Ok(Control::Listen));
    }

    #[test]
    fn rejects_bad_controls() {
        assert!(matches!(p("not json"), Err(ControlError::Json(_))));
        assert!(matches!(p(r#"{"action":"explode"}"#), Err(ControlError::UnknownAction(_))));
        assert!(matches!(p(r#"{"action":"dtmf"}"#), Err(ControlError::Invalid(_))));
        assert!(matches!(p(r#"{"action":"dtmf","digits":"12x"}"#), Err(ControlError::Invalid(_))));
        assert!(matches!(p(r#"{"action":"transfer","to":"5551234"}"#), Err(ControlError::Invalid(_))));
        assert!(matches!(p(r#"{"action":"transfer","to":"+0123456789"}"#), Err(ControlError::Invalid(_))));
        assert!(matches!(p(r#"{"action":"mute","target":"everyone"}"#), Err(ControlError::Invalid(_))));
        assert!(matches!(p(r#"{"action":"transfer","to":"+15105550100","mode":"blind"}"#), Err(ControlError::Invalid(_))));
    }

    #[test]
    fn dtmf_codes() {
        assert_eq!(dtmf_code('0'), Some(0));
        assert_eq!(dtmf_code('9'), Some(9));
        assert_eq!(dtmf_code('*'), Some(10));
        assert_eq!(dtmf_code('#'), Some(11));
        assert_eq!(dtmf_code('D'), Some(15));
        assert_eq!(dtmf_code('x'), None);
        assert_eq!(transfer_uri("+15105550100"), "tel:+15105550100");
        assert_eq!(transfer_uri("sip:a@b"), "sip:a@b");
    }

    #[test]
    fn state_event_and_activity() {
        let mut s = CallState::default();
        assert!(s.bot_active());
        s.takeover_by = Some("u".into());
        assert!(!s.bot_active());
        s.takeover_by = None;
        s.held = true;
        assert!(!s.bot_active());
        assert_eq!(
            s.state_event(),
            CallEvent::StateChanged { held: true, muted_bot: false, muted_caller: false, speaker: None, recording: false }
        );
    }
}
