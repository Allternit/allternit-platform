//! Warm transfer: hold the caller, dial the target into a separate consult
//! room, brief them by voice, and only then connect them to the caller.
//!
//! This file is the state machine and the wording. The LiveKit side (consult
//! room, outbound SIP leg, `MoveParticipant`, REFER fallback) sits behind
//! [`ConsultDriver`], so the machine is tested with fakes; `room.rs` supplies
//! the real driver.
//!
//! ```text
//! dial (ring timeout) ─ no answer ─────────────────────────────▶ NoAnswer
//!   │ answered
//! brief by voice (the call speaks it into the consult room)
//!   │
//! target presses 1 (accept timeout) ─ hangs up / other ─────────▶ Declined
//!   │ accepted                         silence ─────────────────▶ NoAnswer
//! bridge: MoveParticipant, else REFER the caller ─ error ───────▶ Failed
//!   ▼
//! Bridged (the bot leaves)
//! ```
//!
//! Any outcome except `Bridged` takes the caller off hold and the bot says so.
//! The target must press 1, so a voicemail box or an IVR that "answers" never
//! gets the caller.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use super::events::Speaker;

/// How long the target's phone rings before we give up.
pub const DEFAULT_RING_TIMEOUT: Duration = Duration::from_secs(25);
/// How long the target has to press 1 after the briefing.
pub const DEFAULT_ACCEPT_TIMEOUT: Duration = Duration::from_secs(20);
/// Upper bound on the whole attempt, whatever the driver does.
const OVERALL_SLACK: Duration = Duration::from_secs(45);
/// Longest summary read to the target.
const SUMMARY_MAX_CHARS: usize = 240;
/// Caller lines included in the summary.
const SUMMARY_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialError {
    /// Rang out, or the line was busy or rejected the call.
    NoAnswer,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    Accepted,
    /// Hung up, or pressed something other than 1.
    Declined,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMethod {
    /// The target joined the caller's room (`MoveParticipant`).
    Moved,
    /// The caller was SIP-REFERred to the target's number.
    Refer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    Bridged(BridgeMethod),
    NoAnswer,
    Declined,
    Failed(String),
}

impl TransferOutcome {
    pub fn ok(&self) -> bool {
        matches!(self, TransferOutcome::Bridged(_))
    }

    /// The `reason` of `call.transferred` when it didn't work.
    pub fn reason(&self) -> Option<String> {
        match self {
            TransferOutcome::Bridged(_) => None,
            TransferOutcome::NoAnswer => Some("no answer from the transfer target".into()),
            TransferOutcome::Declined => Some("the transfer target declined the call".into()),
            TransferOutcome::Failed(r) => Some(r.clone()),
        }
    }

    /// What the bot tells the caller when it comes off hold.
    pub fn caller_line(&self, to_name: Option<&str>) -> Option<String> {
        let who = to_name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("them");
        match self {
            TransferOutcome::Bridged(_) => None,
            TransferOutcome::NoAnswer => Some(format!(
                "Thanks for holding. I couldn't reach {who} just now. I'm still here, and I can take a message or help another way."
            )),
            TransferOutcome::Declined => Some(format!(
                "Thanks for holding. {who} can't take the call right now. I'm still here, and I can take a message or help another way."
            )),
            TransferOutcome::Failed(_) => Some(
                "Thanks for holding. I wasn't able to connect you just now. I'm still here, and I can take a message or help another way."
                    .into(),
            ),
        }
    }
}

/// The LiveKit side of a warm transfer. Every method is for one consult leg.
pub trait ConsultDriver: Send + Sync + 'static {
    /// Create the consult room and dial the target into it. Resolves when the
    /// target answers.
    fn dial(&self, ring_timeout: Duration) -> BoxFuture<'_, Result<(), DialError>>;
    /// Wait for the target to press 1.
    fn await_accept(&self, timeout: Duration) -> BoxFuture<'_, Accept>;
    /// Connect the target and the caller.
    fn bridge(&self) -> BoxFuture<'_, Result<BridgeMethod, String>>;
    /// Tear down the consult room and the bot's connection to it. Called on
    /// every outcome, once.
    fn cleanup(&self) -> BoxFuture<'_, ()>;
}

/// Speaks `text` into the consult room; resolves `true` once it has been said.
pub type BriefFn = Arc<dyn Fn(String) -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Debug, Clone)]
pub struct TransferPlan {
    pub briefing: String,
    pub ring_timeout: Duration,
    pub accept_timeout: Duration,
}

/// Run one warm transfer attempt to its outcome.
pub async fn run_warm_transfer(driver: Arc<dyn ConsultDriver>, plan: TransferPlan, brief: BriefFn) -> TransferOutcome {
    let overall = plan.ring_timeout + plan.accept_timeout + OVERALL_SLACK;
    let attempt = async {
        match driver.dial(plan.ring_timeout).await {
            Ok(()) => {}
            Err(DialError::NoAnswer) => return TransferOutcome::NoAnswer,
            Err(DialError::Failed(r)) => return TransferOutcome::Failed(format!("couldn't dial the transfer target: {r}")),
        }
        if !brief(plan.briefing.clone()).await {
            return TransferOutcome::Failed("couldn't brief the transfer target".into());
        }
        match driver.await_accept(plan.accept_timeout).await {
            Accept::Accepted => {}
            Accept::Declined => return TransferOutcome::Declined,
            Accept::TimedOut => return TransferOutcome::NoAnswer,
        }
        match driver.bridge().await {
            Ok(m) => TransferOutcome::Bridged(m),
            Err(r) => TransferOutcome::Failed(format!("couldn't connect the call: {r}")),
        }
    };
    let outcome = match tokio::time::timeout(overall, attempt).await {
        Ok(o) => o,
        Err(_) => TransferOutcome::Failed("the transfer took too long".into()),
    };
    driver.cleanup().await;
    outcome
}

/// Why a warm transfer can't even start, or `Ok` when the worker has what it
/// needs. Warm transfer dials out, so it needs an outbound trunk and the
/// consent gate's reference for the (owner-chosen) target number.
pub fn check_prerequisites(outbound_trunk: Option<&str>, consent_ref: Option<&str>) -> Result<(), String> {
    if outbound_trunk.map(str::trim).filter(|t| !t.is_empty()).is_none() {
        return Err("warm transfer needs an outbound SIP trunk, and ALLTERNIT_OUTBOUND_TRUNK_ID is not set on the call worker".into());
    }
    if consent_ref.map(str::trim).filter(|c| !c.is_empty()).is_none() {
        return Err("warm transfer dials out and needs a consentRef from the consent gate; the control carried none".into());
    }
    Ok(())
}

/// Digits grouped for speech: `+16512686010` → `6 5 1, 2 6 8, 6 0 1 0`.
pub fn speak_number(num: &str) -> String {
    let digits: Vec<char> = num.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return String::new();
    }
    let spaced = |d: &[char]| d.iter().map(char::to_string).collect::<Vec<_>>().join(" ");
    let d = if digits.len() == 11 && digits[0] == '1' { &digits[1..] } else { &digits[..] };
    if d.len() == 10 {
        format!("{}, {}, {}", spaced(&d[..3]), spaced(&d[3..6]), spaced(&d[6..]))
    } else {
        d.chunks(3).map(spaced).collect::<Vec<_>>().join(", ")
    }
}

/// Short summary of what the caller said, from the call's final transcript
/// lines. A template, not a model call: nothing waits on the brain while the
/// target's phone is ringing.
pub fn summarize(transcript: &[(Speaker, String)]) -> String {
    let said: Vec<&str> = transcript
        .iter()
        .filter(|(s, t)| *s == Speaker::Caller && !t.trim().is_empty())
        .map(|(_, t)| t.trim())
        .collect();
    if said.is_empty() {
        return "They haven't said what they need yet.".into();
    }
    let recent = &said[said.len().saturating_sub(SUMMARY_LINES)..];
    let mut joined = recent.join(" ");
    if joined.chars().count() > SUMMARY_MAX_CHARS {
        let cut: String = joined.chars().take(SUMMARY_MAX_CHARS).collect();
        let cut = cut.rsplit_once(' ').map(|(a, _)| a.to_string()).unwrap_or(cut);
        joined = format!("{}…", cut.trim_end_matches([',', '.', ' ']));
    }
    format!("They said: {joined}")
}

/// What the bot says to the target.
pub fn briefing(bot_name: Option<&str>, caller: &str, transcript: &[(Speaker, String)]) -> String {
    let who = match bot_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("Hi, this is {n}, an AI assistant."),
        None => "Hi, this is an AI assistant.".to_string(),
    };
    let from = match speak_number(caller) {
        n if n.is_empty() => "I have a caller on the line.".to_string(),
        n => format!("I have a caller on the line from {n}."),
    };
    format!("{who} {from} {} Press 1 to take the call, or hang up to decline.", summarize(transcript))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        dial: Mutex<Option<Result<(), DialError>>>,
        accept: Mutex<Option<Accept>>,
        bridge: Mutex<Option<Result<BridgeMethod, String>>>,
        log: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(
            dial: Result<(), DialError>,
            accept: Accept,
            bridge: Result<BridgeMethod, String>,
        ) -> Arc<Self> {
            Arc::new(Self {
                dial: Mutex::new(Some(dial)),
                accept: Mutex::new(Some(accept)),
                bridge: Mutex::new(Some(bridge)),
                log: Mutex::default(),
            })
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl ConsultDriver for Fake {
        fn dial(&self, _: Duration) -> BoxFuture<'_, Result<(), DialError>> {
            self.log.lock().unwrap().push("dial".into());
            let r = self.dial.lock().unwrap().take().unwrap();
            Box::pin(async move { r })
        }
        fn await_accept(&self, _: Duration) -> BoxFuture<'_, Accept> {
            self.log.lock().unwrap().push("accept".into());
            let r = self.accept.lock().unwrap().take().unwrap();
            Box::pin(async move { r })
        }
        fn bridge(&self) -> BoxFuture<'_, Result<BridgeMethod, String>> {
            self.log.lock().unwrap().push("bridge".into());
            let r = self.bridge.lock().unwrap().take().unwrap();
            Box::pin(async move { r })
        }
        fn cleanup(&self) -> BoxFuture<'_, ()> {
            self.log.lock().unwrap().push("cleanup".into());
            Box::pin(async {})
        }
    }

    fn plan() -> TransferPlan {
        TransferPlan {
            briefing: "brief".into(),
            ring_timeout: Duration::from_secs(25),
            accept_timeout: Duration::from_secs(20),
        }
    }

    fn brief_ok(spoken: Arc<Mutex<Vec<String>>>) -> BriefFn {
        Arc::new(move |t| {
            spoken.lock().unwrap().push(t);
            Box::pin(async { true })
        })
    }

    #[tokio::test]
    async fn happy_path_dials_briefs_waits_for_1_then_bridges() {
        let d = Fake::new(Ok(()), Accept::Accepted, Ok(BridgeMethod::Moved));
        let spoken = Arc::default();
        let out = run_warm_transfer(d.clone(), plan(), brief_ok(Arc::clone(&spoken))).await;
        assert_eq!(out, TransferOutcome::Bridged(BridgeMethod::Moved));
        assert!(out.ok() && out.reason().is_none());
        assert_eq!(d.log(), ["dial", "accept", "bridge", "cleanup"]);
        assert_eq!(*spoken.lock().unwrap(), ["brief"]);
    }

    #[tokio::test]
    async fn no_answer_never_briefs_or_bridges() {
        let d = Fake::new(Err(DialError::NoAnswer), Accept::Accepted, Ok(BridgeMethod::Moved));
        let spoken = Arc::default();
        let out = run_warm_transfer(d.clone(), plan(), brief_ok(Arc::clone(&spoken))).await;
        assert_eq!(out, TransferOutcome::NoAnswer);
        assert_eq!(d.log(), ["dial", "cleanup"]);
        assert!(spoken.lock().unwrap().is_empty());
        assert_eq!(out.reason().unwrap(), "no answer from the transfer target");
    }

    #[tokio::test]
    async fn dial_failure_is_reported_with_its_reason() {
        let d = Fake::new(Err(DialError::Failed("trunk rejected".into())), Accept::Accepted, Ok(BridgeMethod::Moved));
        let out = run_warm_transfer(d.clone(), plan(), brief_ok(Arc::default())).await;
        assert_eq!(out, TransferOutcome::Failed("couldn't dial the transfer target: trunk rejected".into()));
        assert_eq!(d.log(), ["dial", "cleanup"]);
    }

    #[tokio::test]
    async fn declined_and_silent_targets_are_not_bridged() {
        for (acc, want) in [(Accept::Declined, TransferOutcome::Declined), (Accept::TimedOut, TransferOutcome::NoAnswer)] {
            let d = Fake::new(Ok(()), acc, Ok(BridgeMethod::Moved));
            let out = run_warm_transfer(d.clone(), plan(), brief_ok(Arc::default())).await;
            assert_eq!(out, want);
            assert_eq!(d.log(), ["dial", "accept", "cleanup"], "never bridged");
        }
    }

    #[tokio::test]
    async fn bridge_failure_and_brief_failure_fail_cleanly() {
        let d = Fake::new(Ok(()), Accept::Accepted, Err("room gone".into()));
        let out = run_warm_transfer(d.clone(), plan(), brief_ok(Arc::default())).await;
        assert_eq!(out, TransferOutcome::Failed("couldn't connect the call: room gone".into()));
        assert_eq!(d.log().last().unwrap(), "cleanup");

        let d = Fake::new(Ok(()), Accept::Accepted, Ok(BridgeMethod::Refer));
        let silent: BriefFn = Arc::new(|_| Box::pin(async { false }));
        let out = run_warm_transfer(d.clone(), plan(), silent).await;
        assert!(matches!(out, TransferOutcome::Failed(_)));
        assert_eq!(d.log(), ["dial", "cleanup"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stuck_driver_times_out() {
        struct Stuck;
        impl ConsultDriver for Stuck {
            fn dial(&self, _: Duration) -> BoxFuture<'_, Result<(), DialError>> {
                Box::pin(std::future::pending())
            }
            fn await_accept(&self, _: Duration) -> BoxFuture<'_, Accept> {
                unreachable!()
            }
            fn bridge(&self) -> BoxFuture<'_, Result<BridgeMethod, String>> {
                unreachable!()
            }
            fn cleanup(&self) -> BoxFuture<'_, ()> {
                Box::pin(async {})
            }
        }
        let out = run_warm_transfer(Arc::new(Stuck), plan(), brief_ok(Arc::default())).await;
        assert_eq!(out, TransferOutcome::Failed("the transfer took too long".into()));
    }

    #[test]
    fn prerequisites_name_what_is_missing() {
        assert!(check_prerequisites(Some("ST_x"), Some("cc_1")).is_ok());
        let e = check_prerequisites(None, Some("cc_1")).unwrap_err();
        assert!(e.contains("ALLTERNIT_OUTBOUND_TRUNK_ID"), "{e}");
        let e = check_prerequisites(Some("ST_x"), None).unwrap_err();
        assert!(e.contains("consentRef"), "{e}");
        assert!(check_prerequisites(Some("  "), Some("cc")).is_err());
        assert!(check_prerequisites(Some("ST_x"), Some("")).is_err());
    }

    #[test]
    fn numbers_are_spoken_digit_by_digit() {
        assert_eq!(speak_number("+16512686010"), "6 5 1, 2 6 8, 6 0 1 0");
        assert_eq!(speak_number("+442071838750"), "4 4 2, 0 7 1, 8 3 8, 7 5 0");
        assert_eq!(speak_number("sip:desk@pbx"), "");
    }

    #[test]
    fn summary_uses_recent_caller_lines_only() {
        let t = vec![
            (Speaker::Bot, "Hi, how can I help?".to_string()),
            (Speaker::Caller, "My water heater is leaking".to_string()),
            (Speaker::Bot, "I'm sorry to hear that.".to_string()),
            (Speaker::Caller, "It's in the basement".to_string()),
        ];
        assert_eq!(summarize(&t), "They said: My water heater is leaking It's in the basement");
        assert_eq!(summarize(&[]), "They haven't said what they need yet.");
        let long = vec![(Speaker::Caller, "word ".repeat(100))];
        let s = summarize(&long);
        assert!(s.chars().count() < SUMMARY_MAX_CHARS + 20 && s.ends_with('…'), "{s}");
    }

    #[test]
    fn briefing_names_the_bot_the_caller_and_asks_for_1() {
        let t = vec![(Speaker::Caller, "I need a quote".to_string())];
        let b = briefing(Some("Acme"), "+15551230000", &t);
        assert!(b.starts_with("Hi, this is Acme, an AI assistant."));
        assert!(b.contains("from 5 5 5, 1 2 3, 0 0 0 0"), "{b}");
        assert!(b.contains("They said: I need a quote"));
        assert!(b.ends_with("Press 1 to take the call, or hang up to decline."));
    }

    #[test]
    fn caller_lines_are_honest() {
        assert!(TransferOutcome::NoAnswer.caller_line(Some("Sam")).unwrap().contains("couldn't reach Sam"));
        assert!(TransferOutcome::Declined.caller_line(None).unwrap().contains("can't take the call"));
        assert!(TransferOutcome::Failed("x".into()).caller_line(None).unwrap().contains("wasn't able"));
        assert!(TransferOutcome::Bridged(BridgeMethod::Moved).caller_line(None).is_none());
    }
}
