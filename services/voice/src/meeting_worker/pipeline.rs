//! Continuous transcription: 16 kHz call audio → Silero VAD + sherpa STT →
//! STT segments for the pod → wake-word turns.
//!
//! Unlike a call there is no turn-taking, no barge-in and no end-of-turn
//! model: every VAD segment is transcribed and sent, all meeting long. The
//! pod labels each segment's speaker from its active-speaker timeline,
//! assigns `seq` and ingests it (CONTRACT.md "pod ↔ worker names").
//!
//! Split in two so the wake logic runs without models:
//! - [`SegmentProcessor`] (pure): one finished STT segment (epoch times) →
//!   an [`SttSegment`] and maybe a [`TurnEvent`].
//! - [`Transcriber`]: owns the [`SttStream`] (not `Send`: run it on its own
//!   thread) and feeds its finals to the processor.

use std::sync::Arc;

use serde::Serialize;

use super::wake;
use crate::stt::{SttEngine, SttEvent, SttModel, SttStream};

/// One STT segment, the `allternit.meeting.stt` payload (CONTRACT.md
/// "pod ↔ worker names"; services/meeting-bot/src/livekit.ts `SttSegment`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SttSegment {
    /// Ms from the first audio the worker received on `call-audio`.
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Epoch ms the speech started. The pod prefers it over `start_ms`.
    pub start_at: i64,
}

/// A wake-word turn: someone in the call asked Gizzi something. The
/// `allternit.meeting.turn` payload, worker → pod: `{turn_id, text,
/// asker_label?, confidence, start_ms}` (coordinator binding 2026-11-13).
/// The worker has no speaker timeline, so it leaves `asker_label` out; the pod
/// labels the asker from `start_ms`..`end_ms` and runs the in-pod agent.
/// `end_ms`, `start_at` and `heard` are extra fields the pod may ignore.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnEvent {
    /// `wake-<session id>-<start_at>`: the same turn always gets the same id,
    /// so a resend can't start a second agent turn.
    pub turn_id: String,
    /// What was asked, wake word removed (the next segment when the asker
    /// paused after the name). `MeetingTurnRequest.trigger.text`.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asker_label: Option<String>,
    /// How sure the wake word is (spelling and position), 0..1.
    pub confidence: f32,
    /// Where the turn was said, ms from the start of `call-audio`.
    pub start_ms: i64,
    pub end_ms: i64,
    /// Epoch ms of `start_ms`.
    pub start_at: i64,
    /// The words heard as the name ("Jizzy").
    pub heard: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Segment(SttSegment),
    Turn(TurnEvent),
}

/// A wake word with nothing after it waits this long for the question.
const PENDING_WAKE_MS: i64 = 6_000;

struct PendingWake {
    heard: String,
    wake_confidence: f32,
    start_at: i64,
    end_at: i64,
}

pub struct SegmentProcessor {
    session_id: String,
    /// Epoch ms of the first audio on `call-audio` (offsets count from it).
    track_epoch_ms: i64,
    pending: Option<PendingWake>,
}

impl SegmentProcessor {
    pub fn new(session_id: impl Into<String>, track_epoch_ms: i64) -> Self {
        Self { session_id: session_id.into(), track_epoch_ms, pending: None }
    }

    fn offset(&self, epoch: i64) -> i64 {
        (epoch - self.track_epoch_ms).max(0)
    }

    /// One finished segment, times in epoch ms.
    pub fn on_segment(&mut self, start_at: i64, end_at: i64, text: &str) -> Vec<Output> {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let end_at = end_at.max(start_at);
        let mut out = vec![Output::Segment(SttSegment {
            start_ms: self.offset(start_at),
            end_ms: self.offset(end_at),
            text: text.to_string(),
            start_at,
        })];

        // A wake word waiting for its question: this segment is the question
        // if it follows soon enough and has no wake word of its own.
        let own = wake::detect(text);
        if let Some(p) = self.pending.take() {
            if own.is_none() && start_at - p.end_at <= PENDING_WAKE_MS {
                out.push(Output::Turn(self.turn(text.to_string(), p.heard, p.wake_confidence, p.start_at, end_at)));
                return out;
            }
        }
        if let Some(hit) = own {
            if hit.question.is_empty() {
                self.pending = Some(PendingWake { heard: hit.heard, wake_confidence: hit.confidence, start_at, end_at });
            } else {
                out.push(Output::Turn(self.turn(hit.question, hit.heard, hit.confidence, start_at, end_at)));
            }
        }
        out
    }

    fn turn(&self, text: String, heard: String, confidence: f32, start_at: i64, end_at: i64) -> TurnEvent {
        TurnEvent {
            turn_id: format!("wake-{}-{start_at}", self.session_id),
            text,
            asker_label: None,
            confidence,
            start_ms: self.offset(start_at),
            end_ms: self.offset(end_at),
            start_at,
            heard,
        }
    }
}

/// STT stream + processor. Feed 16 kHz mono f32; blocking (runs inference).
pub struct Transcriber {
    stt: SttStream,
    /// Epoch ms of the first sample fed to this stream.
    stream_epoch_ms: i64,
    /// Samples fed so far (to place the next chunk in time).
    fed: u64,
    pub processor: SegmentProcessor,
}

impl Transcriber {
    /// Loads the recogniser on first use (blocking, may download the pack).
    pub fn new(engine: &Arc<SttEngine>, model: SttModel, stream_epoch_ms: i64, processor: SegmentProcessor) -> Result<Self, String> {
        Ok(Self { stt: engine.stream(model)?, stream_epoch_ms, fed: 0, processor })
    }

    /// Epoch ms the next fed sample belongs to, by sample count.
    pub fn expected_epoch_ms(&self) -> i64 {
        self.stream_epoch_ms + (self.fed * 1000 / crate::stt::SAMPLE_RATE as u64) as i64
    }

    pub fn feed(&mut self, samples: &[f32]) -> Vec<Output> {
        self.fed += samples.len() as u64;
        let events = self.stt.feed(samples);
        handle(&mut self.processor, self.stream_epoch_ms, events)
    }

    /// End of audio (leave, pause, a gap in the track): flush the speech in
    /// progress. Returns the processor so the next stream keeps a pending wake word.
    pub fn finish(self) -> (Vec<Output>, SegmentProcessor) {
        let Transcriber { stt, stream_epoch_ms, mut processor, .. } = self;
        let out = handle(&mut processor, stream_epoch_ms, stt.finish());
        (out, processor)
    }
}

fn handle(processor: &mut SegmentProcessor, stream_epoch_ms: i64, events: Vec<SttEvent>) -> Vec<Output> {
    let mut out = Vec::new();
    for ev in events {
        if let SttEvent::Final(seg) = ev {
            let s = stream_epoch_ms + (seg.start * 1000.0).round() as i64;
            let e = stream_epoch_ms + (seg.end * 1000.0).round() as i64;
            out.extend(processor.on_segment(s, e, &seg.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_794_575_055_354;

    fn segs(out: &[Output]) -> Vec<&SttSegment> {
        out.iter().filter_map(|o| if let Output::Segment(s) = o { Some(s) } else { None }).collect()
    }
    fn turns(out: &[Output]) -> Vec<&TurnEvent> {
        out.iter().filter_map(|o| if let Output::Turn(t) = o { Some(t) } else { None }).collect()
    }

    #[test]
    fn segments_carry_track_offsets_and_absolute_start() {
        let mut p = SegmentProcessor::new("s1", T0);
        let out = p.on_segment(T0 + 4_100, T0 + 9_000, "  First item.  ");
        assert_eq!(segs(&out), [&SttSegment { start_ms: 4_100, end_ms: 9_000, text: "First item.".into(), start_at: T0 + 4_100 }]);
        assert!(turns(&out).is_empty());
        assert!(p.on_segment(T0 + 9_500, T0 + 9_600, "   ").is_empty());
        // The wire shape the pod parses (livekit.ts SttSegment).
        let json = serde_json::to_value(segs(&out)[0]).unwrap();
        assert_eq!(json, serde_json::json!({"start_ms": 4100, "end_ms": 9000, "text": "First item.", "start_at": T0 + 4_100}));
    }

    #[test]
    fn wake_word_in_a_segment_is_a_turn() {
        let mut p = SegmentProcessor::new("s1", T0);
        let out = p.on_segment(T0 + 23_000, T0 + 30_000, "Assign it to Jordan, Jizzy, please take note of that action item.");
        let t = turns(&out)[0];
        assert_eq!(t.text, "please take note of that action item.");
        assert_eq!(t.heard, "Jizzy");
        assert_eq!((t.start_ms, t.end_ms, t.start_at), (23_000, 30_000, T0 + 23_000));
        // The wire shape the pod parses: {turn_id, text, asker_label?, confidence, start_ms}.
        let json = serde_json::to_value(t).unwrap();
        assert_eq!(json["turn_id"], format!("wake-s1-{}", T0 + 23_000));
        assert_eq!(json["text"], "please take note of that action item.");
        assert!(json.get("asker_label").is_none());
        assert!(json["confidence"].as_f64().unwrap() >= 0.7);
        assert_eq!(json["start_ms"], 23_000);
    }

    #[test]
    fn bare_wake_word_takes_the_next_segment_as_the_question() {
        let mut p = SegmentProcessor::new("s1", T0);
        assert!(turns(&p.on_segment(T0 + 1_000, T0 + 1_600, "Hey Gizzi?")).is_empty());
        let out = p.on_segment(T0 + 2_500, T0 + 5_000, "What did we decide on pricing?");
        let t = turns(&out)[0];
        assert_eq!(t.text, "What did we decide on pricing?");
        assert_eq!((t.start_ms, t.end_ms), (1_000, 5_000));
    }

    #[test]
    fn a_stale_bare_wake_word_is_dropped() {
        let mut p = SegmentProcessor::new("s1", T0);
        p.on_segment(T0 + 1_000, T0 + 1_500, "Gizzi.");
        let out = p.on_segment(T0 + 20_000, T0 + 22_000, "Anyway, moving on to the roadmap.");
        assert!(turns(&out).is_empty());
        assert_eq!(segs(&out).len(), 1);
    }
}
