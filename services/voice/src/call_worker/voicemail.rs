//! Voicemail detection hook (outbound calls only; §4.1 `call.voicemail.detected`).
//!
//! TODO(outbound): outbound calls are not enabled yet (joe-07's consent gate
//! starts them). When they are, implement a detector (answering-machine cues in
//! the first seconds of caller transcript + beep detection on the audio) and
//! return [`VoicemailAction`]; the call emits `call.voicemail.detected {action}`
//! and either leaves the bot's message or hangs up. Inbound calls never run it.

/// What to do once a voicemail greeting is detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoicemailAction {
    LeaveMessage,
    Hangup,
}

impl VoicemailAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            VoicemailAction::LeaveMessage => "leave_message",
            VoicemailAction::Hangup => "hangup",
        }
    }
}

pub trait VoicemailDetector: Send {
    /// Called with each final caller transcript segment and its offset from
    /// call start. Return `Some` once, when a voicemail greeting is detected.
    fn observe_final(&mut self, text: &str, at_ms: u64) -> Option<VoicemailAction>;
}

/// Detector used until outbound ships: never fires.
pub struct NoVoicemailDetection;

impl VoicemailDetector for NoVoicemailDetection {
    fn observe_final(&mut self, _text: &str, _at_ms: u64) -> Option<VoicemailAction> {
        None
    }
}
