//! Answering-machine detection for outbound calls (§4.1 `call.voicemail.detected`).
//!
//! An outbound call doesn't speak until it knows who answered. For the first
//! seconds after answer [`AnswerScreen`] reads the caller-side STT text, the VAD
//! speech edges and the audio itself (a Goertzel beep detector) and decides:
//!
//! - **Human**: a short pickup ("Hello?", "Yes?"), or silence long enough that a
//!   person is waiting for us. The bot speaks its normal opening.
//! - **Machine**: phrases like "leave a message" / "after the tone", a long
//!   unbroken greeting, or a beep. The bot stays quiet until the beep, then
//!   leaves one short honest message and hangs up (`left_message`).
//! - **Unavailable**: "mailbox is full", "not accepting messages", "number is
//!   not in service". The bot hangs up without speaking (`hung_up`).
//!
//! Everything here is pure (time is passed in as `at_ms`), so it is tested with
//! transcripts and synthetic audio. Inbound calls use [`NoScreening`].

use std::f64::consts::TAU;

/// Beep detector sample rate: caller audio reaches the call at 16 kHz.
const RATE: u32 = 16_000;
/// 40 ms frames: 25 Hz bins.
const FRAME: usize = 640;
/// A beep is one steady tone for at least this many frames (8 × 40 ms = 320 ms).
const BEEP_FRAMES: u32 = 8;
/// Share of the frame's energy one frequency must hold to count as a pure tone.
const TONE_FRACTION: f64 = 0.6;
/// Frames quieter than this RMS (int16 scale, about -38 dBFS) are not tones.
const MIN_RMS: f64 = 400.0;
/// Candidate frequencies: 500–2000 Hz in 12.5 Hz steps, so no tone sits more
/// than a quarter bin from a candidate. Voicemail beeps live around 1 kHz.
const BEEP_LOW_HZ: f64 = 500.0;
const BEEP_HIGH_HZ: f64 = 2000.0;
const BEEP_STEP_HZ: f64 = 12.5;
/// Consecutive tone frames may drift this far in frequency.
const BEEP_DRIFT_HZ: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeepEvent {
    /// A steady tone has lasted long enough to be a beep.
    Started,
    /// The beep stopped: the machine is recording now.
    Ended,
}

fn goertzel_power(frame: &[f64], freq: f64) -> f64 {
    let coeff = 2.0 * (TAU * freq / RATE as f64).cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for x in frame {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Share of the frame's energy that sits at `freq` (1.0 for a pure tone on a
/// bin, lower between bins, near 0 for speech and noise).
fn tone_fraction(frame: &[f64], freq: f64) -> f64 {
    let energy: f64 = frame.iter().map(|x| x * x).sum();
    if energy <= 0.0 {
        return 0.0;
    }
    2.0 * goertzel_power(frame, freq) / (frame.len() as f64 * energy)
}

/// Best tone in a frame: `(frequency, energy share)`.
fn best_tone(frame: &[f64]) -> (f64, f64) {
    let steps = ((BEEP_HIGH_HZ - BEEP_LOW_HZ) / BEEP_STEP_HZ) as usize;
    (0..=steps)
        .map(|i| BEEP_LOW_HZ + i as f64 * BEEP_STEP_HZ)
        .map(|f| (f, tone_fraction(frame, f)))
        .fold((0.0, 0.0), |best, c| if c.1 > best.1 { c } else { best })
}

/// Finds beeps in 16 kHz mono PCM16: a pure tone near 1 kHz lasting at least
/// 300 ms. Speech never holds one frequency that steadily.
#[derive(Default)]
pub struct BeepDetector {
    buf: Vec<f64>,
    run: u32,
    run_freq: f64,
    active: bool,
}

impl BeepDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, samples: &[i16]) -> Vec<BeepEvent> {
        let mut out = Vec::new();
        self.buf.extend(samples.iter().map(|s| *s as f64));
        while self.buf.len() >= FRAME {
            let frame: Vec<f64> = self.buf.drain(..FRAME).collect();
            let rms = (frame.iter().map(|x| x * x).sum::<f64>() / FRAME as f64).sqrt();
            let (freq, frac) = best_tone(&frame);
            let tone = rms >= MIN_RMS && frac >= TONE_FRACTION;
            if tone {
                if self.run > 0 && (freq - self.run_freq).abs() <= BEEP_DRIFT_HZ {
                    self.run += 1;
                } else {
                    self.run = 1;
                }
                self.run_freq = freq;
                if self.run >= BEEP_FRAMES && !self.active {
                    self.active = true;
                    out.push(BeepEvent::Started);
                }
            } else {
                self.run = 0;
                if self.active {
                    self.active = false;
                    out.push(BeepEvent::Ended);
                }
            }
        }
        out
    }
}

/// What the call should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// A person answered: speak the opening now.
    Human,
    /// An answering machine: stay quiet and wait for the beep.
    Machine,
    /// The recording has started: leave the message, then hang up.
    LeaveMessage,
    /// Hang up without speaking. `reason` is for the log and the event.
    HangUp { reason: String },
}

/// The `action` of `call.voicemail.detected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicemailAction {
    LeftMessage,
    HungUp,
}

impl VoicemailAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            VoicemailAction::LeftMessage => "left_message",
            VoicemailAction::HungUp => "hung_up",
        }
    }
}

/// Fed by the call loop for outbound calls. Each method may return one
/// [`Signal`]; once `Human` or `HangUp` is returned the detector is done.
pub trait VoicemailDetector: Send {
    /// Caller-side STT text, interim or final.
    fn on_text(&mut self, text: &str, is_final: bool, at_ms: u64) -> Option<Signal>;
    /// VAD edge: the far end started or stopped speaking.
    fn on_speech(&mut self, active: bool, at_ms: u64) -> Option<Signal>;
    /// 16 kHz mono PCM16 from the far end.
    fn on_audio(&mut self, samples: &[i16], at_ms: u64) -> Option<Signal>;
    /// Time passing with nothing else happening.
    fn on_tick(&mut self, at_ms: u64) -> Option<Signal>;
    /// Whether this call screens answers at all (outbound only).
    fn active(&self) -> bool {
        true
    }
}

/// Inbound calls: the caller is a person who dialed us.
pub struct NoScreening;

impl VoicemailDetector for NoScreening {
    fn on_text(&mut self, _: &str, _: bool, _: u64) -> Option<Signal> {
        None
    }
    fn on_speech(&mut self, _: bool, _: u64) -> Option<Signal> {
        None
    }
    fn on_audio(&mut self, _: &[i16], _: u64) -> Option<Signal> {
        None
    }
    fn on_tick(&mut self, _: u64) -> Option<Signal> {
        None
    }
    fn active(&self) -> bool {
        false
    }
}

/// Phrases a greeting only uses when it is a machine.
const MACHINE_PHRASES: &[&str] = &[
    "leave a message",
    "leave your message",
    "leave me a message",
    "leave your name",
    "after the tone",
    "after the beep",
    "at the tone",
    "at the beep",
    "not available",
    "isn t available",
    "can t take your call",
    "cannot take your call",
    "can t come to the phone",
    "unable to take your call",
    "voicemail",
    "voice mail",
    "voice message",
    "answering machine",
    "please record",
    "record your message",
    "your call has been forwarded",
    "call has been forwarded",
    "the person you are trying to reach",
    "the party you are trying to reach",
    "the person you have called",
    "the number you have dialed",
    "the number you dialed",
    "the subscriber you have dialed",
];

/// Phrases of a mailbox that will not take a message: hang up, don't speak.
const UNAVAILABLE_PHRASES: &[&str] = &[
    "mailbox is full",
    "mailbox has not been set up",
    "mailbox is not set up",
    "voicemail is full",
    "not accepting messages",
    "not accepting any messages",
    "cannot accept messages",
    "can t accept messages",
    "can not accept messages",
    "unable to accept messages",
    "no longer in service",
    "not in service",
    "has been disconnected",
    "is not a working number",
    "cannot be completed as dialed",
    "mailbox is not available",
];

/// Lowercase letters and digits only; apostrophes become spaces ("can't" →
/// "can t") so STT spellings with and without them match the same phrase.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_phrase(norm: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| norm.contains(p))
}

/// Timing of the screen, in ms from answer.
#[derive(Debug, Clone, Copy)]
pub struct ScreenTimings {
    /// Nobody spoke by now: a person is waiting for us to talk.
    pub silence_means_human_ms: u64,
    /// Undecided by now: judge on what's been heard.
    pub window_ms: u64,
    /// At the window, a pause at least this long since the last speech reads as
    /// a person waiting; still talking reads as a machine monologue.
    pub human_pause_ms: u64,
    /// A machine greeting that ends without a beep: leave the message after
    /// this much quiet.
    pub quiet_before_message_ms: u64,
    /// Give up on a machine that never beeps nor falls quiet.
    pub machine_deadline_ms: u64,
    /// Settle after the beep ends before speaking.
    pub after_beep_ms: u64,
}

impl Default for ScreenTimings {
    fn default() -> Self {
        Self {
            silence_means_human_ms: 5_000,
            window_ms: 8_000,
            human_pause_ms: 1_500,
            quiet_before_message_ms: 4_000,
            machine_deadline_ms: 45_000,
            after_beep_ms: 300,
        }
    }
}

/// A final of this many words or fewer, with no machine phrase, is a pickup.
const PICKUP_MAX_WORDS: usize = 3;
/// An unbroken greeting at least this long is a machine reciting.
const MONOLOGUE_MIN_WORDS: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Listening,
    Machine,
    Done,
}

pub struct AnswerScreen {
    t: ScreenTimings,
    phase: Phase,
    beeps: BeepDetector,
    /// Words heard so far (finals only count toward the total).
    words_final: usize,
    heard_speech: bool,
    speech_active: bool,
    last_speech_ms: u64,
    beep_ended_at: Option<u64>,
    beep_seen: bool,
    in_beep: bool,
}

impl Default for AnswerScreen {
    fn default() -> Self {
        Self::new(ScreenTimings::default())
    }
}

impl AnswerScreen {
    pub fn new(t: ScreenTimings) -> Self {
        Self {
            t,
            phase: Phase::Listening,
            beeps: BeepDetector::new(),
            words_final: 0,
            heard_speech: false,
            speech_active: false,
            last_speech_ms: 0,
            beep_ended_at: None,
            beep_seen: false,
            in_beep: false,
        }
    }

    fn machine(&mut self) -> Option<Signal> {
        if self.phase == Phase::Listening {
            self.phase = Phase::Machine;
            return Some(Signal::Machine);
        }
        None
    }

    fn finish(&mut self, s: Signal) -> Option<Signal> {
        self.phase = Phase::Done;
        Some(s)
    }

    fn text_signal(&mut self, text: &str, is_final: bool, at_ms: u64) -> Option<Signal> {
        let norm = normalize(text);
        if norm.is_empty() {
            return None;
        }
        self.heard_speech = true;
        self.last_speech_ms = at_ms;
        if has_phrase(&norm, UNAVAILABLE_PHRASES) {
            return self.finish(Signal::HangUp { reason: format!("mailbox unavailable: \"{norm}\"") });
        }
        let words = norm.split_whitespace().count();
        if has_phrase(&norm, MACHINE_PHRASES) || words >= MONOLOGUE_MIN_WORDS {
            return self.machine();
        }
        if is_final {
            self.words_final += words;
            if words <= PICKUP_MAX_WORDS && self.phase == Phase::Listening && self.words_final <= PICKUP_MAX_WORDS {
                return self.finish(Signal::Human);
            }
        }
        None
    }

    fn time_signal(&mut self, at_ms: u64) -> Option<Signal> {
        match self.phase {
            Phase::Done => None,
            Phase::Listening => {
                if !self.heard_speech && !self.beep_seen && at_ms >= self.t.silence_means_human_ms {
                    return self.finish(Signal::Human);
                }
                if at_ms >= self.t.window_ms && (self.heard_speech || self.beep_seen) {
                    let paused = !self.speech_active && at_ms.saturating_sub(self.last_speech_ms) >= self.t.human_pause_ms;
                    if paused && !self.beep_seen {
                        return self.finish(Signal::Human);
                    }
                    return self.machine();
                }
                None
            }
            Phase::Machine => {
                if let Some(end) = self.beep_ended_at {
                    if at_ms >= end + self.t.after_beep_ms {
                        return self.finish(Signal::LeaveMessage);
                    }
                    return None;
                }
                if self.in_beep {
                    return None;
                }
                if at_ms >= self.t.machine_deadline_ms {
                    return self.finish(Signal::HangUp { reason: "machine never beeped".into() });
                }
                // No beep: the greeting fell quiet, so the recording is running.
                if !self.speech_active
                    && !self.beep_seen
                    && at_ms.saturating_sub(self.last_speech_ms) >= self.t.quiet_before_message_ms
                {
                    return self.finish(Signal::LeaveMessage);
                }
                None
            }
        }
    }
}

impl VoicemailDetector for AnswerScreen {
    fn on_text(&mut self, text: &str, is_final: bool, at_ms: u64) -> Option<Signal> {
        if self.phase == Phase::Done {
            return None;
        }
        self.text_signal(text, is_final, at_ms)
    }

    fn on_speech(&mut self, active: bool, at_ms: u64) -> Option<Signal> {
        if self.phase == Phase::Done {
            return None;
        }
        self.speech_active = active;
        if active {
            self.heard_speech = true;
        }
        self.last_speech_ms = at_ms;
        None
    }

    fn on_audio(&mut self, samples: &[i16], at_ms: u64) -> Option<Signal> {
        if self.phase == Phase::Done {
            return None;
        }
        for ev in self.beeps.push(samples) {
            match ev {
                BeepEvent::Started => {
                    self.beep_seen = true;
                    self.in_beep = true;
                    self.beep_ended_at = None;
                    // A beep only ever comes from a machine.
                    if let Some(s) = self.machine() {
                        return Some(s);
                    }
                }
                BeepEvent::Ended => {
                    self.in_beep = false;
                    self.beep_ended_at = Some(at_ms);
                }
            }
        }
        None
    }

    fn on_tick(&mut self, at_ms: u64) -> Option<Signal> {
        self.time_signal(at_ms)
    }
}

/// The message left on a machine. `custom` is the bot's configured voicemail
/// message; the default says who is calling and the callback number. Either way
/// it states that it's an AI assistant, since the caller identity is never
/// configurable away.
pub fn message(bot_name: Option<&str>, callback: &str, custom: Option<&str>) -> String {
    let who = match bot_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("Hi, this is {n}, an AI assistant."),
        None => "Hi, this is an AI assistant.".to_string(),
    };
    let spoken = crate::call_worker::transfer::speak_number(callback);
    match custom.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => format!("{who} {m}"),
        None if spoken.is_empty() => format!("{who} Sorry we missed you. Goodbye."),
        None => format!("{who} Sorry we missed you. You can call us back on {spoken}. Goodbye."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, ms: u32, amp: f64) -> Vec<i16> {
        let n = (RATE * ms / 1000) as usize;
        (0..n).map(|i| (amp * (TAU * freq * i as f64 / RATE as f64).sin()) as i16).collect()
    }

    /// Deterministic "speech-like" signal: a gliding two-tone buzz plus noise.
    fn speechish(ms: u32) -> Vec<i16> {
        let n = (RATE * ms / 1000) as usize;
        let mut seed = 12345u32;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((seed >> 16) as f64 / 65535.0 - 0.5) * 3000.0;
                let t = i as f64 / RATE as f64;
                let f0 = 120.0 + 30.0 * (TAU * 3.0 * t).sin();
                let s = 3000.0 * (TAU * f0 * t).sin() + 2500.0 * (TAU * 2.4 * f0 * t).sin() + 1500.0 * (TAU * 7.1 * f0 * t).sin();
                (s + noise) as i16
            })
            .collect()
    }

    fn events(det: &mut BeepDetector, audio: &[i16]) -> Vec<BeepEvent> {
        audio.chunks(320).flat_map(|c| det.push(c)).collect()
    }

    #[test]
    fn detects_a_one_kilohertz_beep() {
        let mut d = BeepDetector::new();
        let mut audio = vec![0i16; 8000];
        audio.extend(tone(1000.0, 500, 8000.0));
        audio.extend(vec![0i16; 4000]);
        assert_eq!(events(&mut d, &audio), [BeepEvent::Started, BeepEvent::Ended]);
    }

    #[test]
    fn detects_beeps_between_bins_and_other_pitches() {
        for f in [820.0, 987.0, 1013.0, 1337.0, 1760.0] {
            let mut d = BeepDetector::new();
            let mut audio = tone(f, 600, 6000.0);
            audio.extend(vec![0i16; 3200]);
            assert_eq!(events(&mut d, &audio), [BeepEvent::Started, BeepEvent::Ended], "{f} Hz");
        }
    }

    #[test]
    fn beep_through_noise_still_detected() {
        let mut d = BeepDetector::new();
        let noise = speechish(500);
        let beep: Vec<i16> = tone(1000.0, 500, 9000.0)
            .iter()
            .zip(noise.iter().cycle())
            .map(|(a, b)| a.saturating_add(b / 12))
            .collect();
        let mut audio = beep;
        audio.extend(vec![0i16; 3200]);
        assert_eq!(events(&mut d, &audio), [BeepEvent::Started, BeepEvent::Ended]);
    }

    #[test]
    fn short_blips_and_speech_are_not_beeps() {
        let mut d = BeepDetector::new();
        // 200 ms is under the 300 ms floor.
        let mut audio = tone(1000.0, 200, 8000.0);
        audio.extend(vec![0i16; 3200]);
        assert!(events(&mut d, &audio).is_empty());
        // Four seconds of voiced speech-like audio never holds one tone.
        assert!(events(&mut d, &speechish(4000)).is_empty());
        // A quiet hum is below the level floor.
        assert!(events(&mut d, &tone(1000.0, 800, 200.0)).is_empty());
        // A tone that glides is not steady.
        let glide: Vec<i16> = (0..8000)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                (8000.0 * (TAU * (600.0 + 1000.0 * t * t * 2.0) * t).sin()) as i16
            })
            .collect();
        assert!(events(&mut d, &glide).is_empty());
    }

    #[test]
    fn beep_split_across_chunks_counts_once() {
        let mut d = BeepDetector::new();
        let audio = tone(1000.0, 400, 8000.0);
        let mut evs = Vec::new();
        for c in audio.chunks(97) {
            evs.extend(d.push(c));
        }
        evs.extend(d.push(&vec![0i16; 1280]));
        assert_eq!(evs, [BeepEvent::Started, BeepEvent::Ended]);
    }

    #[test]
    fn normalize_handles_stt_spellings() {
        assert_eq!(normalize("Please leave a message, after the tone."), "please leave a message after the tone");
        assert!(has_phrase(&normalize("I can't take your call"), MACHINE_PHRASES));
        assert!(has_phrase(&normalize("I cant take your call"), &["cant take your call", "can t take your call"]));
    }

    // ---- the screen, driven with transcripts and audio on a timeline ----

    #[test]
    fn short_pickup_is_human() {
        let mut s = AnswerScreen::default();
        s.on_speech(true, 900);
        assert_eq!(s.on_text("Hel", false, 1000), None);
        s.on_speech(false, 1300);
        assert_eq!(s.on_text("Hello?", true, 1400), Some(Signal::Human));
        // Done: nothing further fires.
        assert_eq!(s.on_tick(20_000), None);
    }

    #[test]
    fn silence_for_five_seconds_is_human() {
        let mut s = AnswerScreen::default();
        assert_eq!(s.on_tick(4_900), None);
        assert_eq!(s.on_tick(5_000), Some(Signal::Human));
    }

    #[test]
    fn greeting_with_phrase_is_machine_then_message_after_beep() {
        let mut s = AnswerScreen::default();
        s.on_speech(true, 1000);
        assert_eq!(
            s.on_text("Hi you've reached Bob, I can't come to the phone, please leave a message after the tone", false, 3500),
            Some(Signal::Machine)
        );
        s.on_speech(false, 6500);
        assert_eq!(s.on_tick(7000), None, "waits for the beep, not just a pause");
        // The beep arrives as audio at 8 s.
        let mut audio = tone(1000.0, 500, 8000.0);
        audio.extend(vec![0i16; 3200]);
        let mut out = None;
        let mut t = 7500;
        for c in audio.chunks(320) {
            t += 20;
            if let Some(sig) = s.on_audio(c, t) {
                out = Some(sig);
            }
        }
        assert_eq!(out, None, "already machine; the beep just starts the settle clock");
        assert_eq!(s.on_tick(t + 100), None);
        assert_eq!(s.on_tick(t + 400), Some(Signal::LeaveMessage));
        assert_eq!(s.on_tick(t + 5000), None, "fires once");
    }

    #[test]
    fn beep_alone_marks_a_machine() {
        let mut s = AnswerScreen::default();
        let mut got = None;
        let mut t = 0;
        for c in tone(1000.0, 500, 8000.0).chunks(320) {
            t += 20;
            got = got.or(s.on_audio(c, t));
        }
        assert_eq!(got, Some(Signal::Machine));
    }

    #[test]
    fn long_monologue_is_machine_without_a_phrase() {
        let mut s = AnswerScreen::default();
        let text = "hello thank you for calling the offices of Smith and Jones we are open Monday through Friday";
        assert_eq!(s.on_text(text, true, 4000), Some(Signal::Machine));
    }

    #[test]
    fn machine_without_a_beep_gets_the_message_after_quiet() {
        let mut s = AnswerScreen::default();
        s.on_speech(true, 500);
        assert_eq!(s.on_text("please leave a message", true, 2500), Some(Signal::Machine));
        s.on_speech(false, 3000);
        assert_eq!(s.on_tick(6000), None);
        assert_eq!(s.on_tick(7100), Some(Signal::LeaveMessage));
    }

    #[test]
    fn machine_that_never_goes_quiet_nor_beeps_is_hung_up_on() {
        let mut s = AnswerScreen::default();
        s.on_speech(true, 500);
        s.on_text("leave a message", true, 1500);
        // Still "speaking" (e.g. music on hold) at the deadline.
        assert!(matches!(s.on_tick(45_000), Some(Signal::HangUp { .. })));
    }

    #[test]
    fn full_mailbox_hangs_up_without_speaking() {
        let mut s = AnswerScreen::default();
        let sig = s.on_text("The mailbox is full and cannot accept messages at this time", true, 3000);
        assert!(matches!(sig, Some(Signal::HangUp { .. })));
        let mut s = AnswerScreen::default();
        assert!(matches!(
            s.on_text("The number you have dialed is not in service", false, 2000),
            Some(Signal::HangUp { .. })
        ));
    }

    #[test]
    fn mid_length_pickup_with_a_pause_is_human_at_the_window() {
        let mut s = AnswerScreen::default();
        s.on_speech(true, 1000);
        assert_eq!(s.on_text("Yes this is Bob who's calling", true, 2600), None);
        s.on_speech(false, 2800);
        assert_eq!(s.on_tick(7_999), None);
        assert_eq!(s.on_tick(8_000), Some(Signal::Human));
    }

    #[test]
    fn mid_length_greeting_still_talking_at_the_window_is_machine() {
        let mut s = AnswerScreen::default();
        s.on_speech(true, 1000);
        s.on_text("Thanks for calling Acme", true, 3000);
        s.on_speech(true, 7500);
        assert_eq!(s.on_tick(8000), Some(Signal::Machine));
    }

    #[test]
    fn an_early_short_final_does_not_decide_after_more_words_arrived() {
        // "Hi" then more speech: words add up, so it isn't a lone pickup.
        let mut s = AnswerScreen::default();
        assert_eq!(s.on_text("Hi", true, 800), Some(Signal::Human));
        let mut s = AnswerScreen::default();
        assert_eq!(s.on_text("You've reached the Smiths", true, 2000), None);
        assert_eq!(s.on_text("Please", true, 3000), None);
    }

    #[test]
    fn no_screening_for_inbound() {
        let mut n = NoScreening;
        assert!(!n.active());
        assert_eq!(n.on_text("leave a message", true, 1), None);
        assert_eq!(n.on_tick(99_999), None);
    }

    #[test]
    fn default_message_is_honest_and_gives_a_callback() {
        let m = message(Some("Acme Plumbing"), "+16512686010", None);
        assert!(m.starts_with("Hi, this is Acme Plumbing, an AI assistant."));
        assert!(m.contains("6 5 1"), "{m}");
        assert!(m.ends_with("Goodbye."));
        // A configured message follows the fixed identity line.
        let m = message(Some("Acme"), "+16512686010", Some("We'll try you again tomorrow."));
        assert_eq!(m, "Hi, this is Acme, an AI assistant. We'll try you again tomorrow.");
        assert_eq!(message(None, "", None), "Hi, this is an AI assistant. Sorry we missed you. Goodbye.");
    }

    #[test]
    fn actions_name_the_contract_values() {
        assert_eq!(VoicemailAction::LeftMessage.as_str(), "left_message");
        assert_eq!(VoicemailAction::HungUp.as_str(), "hung_up");
    }
}
