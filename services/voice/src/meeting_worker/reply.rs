//! Spoken replies: Kokoro (through `allternit-tts`, see `tts.rs`) → 48 kHz
//! 10 ms frames for the worker's LiveKit track `tts`, which the pod
//! subscribes to and plays into the call (CONTRACT.md "pod ↔ worker names").
//!
//! The pod asks for speech on data topic `allternit.meeting.speak`
//! (`{turn_id, text}`), only when the session has `speak_replies` on
//! (CONTRACT.md: chat is the default; voice is opt-in per meeting).

use std::sync::Arc;

use serde::Deserialize;

use crate::call_worker::audio::{Framer, Resampler, TRACK_RATE};
use crate::tts::TtsEngine;

/// Name of the published reply track (meeting-bot livekit.ts `TTS_TRACK`).
pub const TTS_TRACK: &str = "tts";

/// `allternit.meeting.speak` payload.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpeakRequest {
    #[serde(default)]
    pub turn_id: String,
    pub text: String,
}

/// Longest reply spoken (characters); a runaway reply is cut at a sentence.
pub const MAX_SPEAK_CHARS: usize = 2_000;

impl SpeakRequest {
    /// Parse a payload; `None` for bad JSON or empty text. Over-long text is
    /// cut at the last sentence end before the cap.
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let mut r: SpeakRequest = serde_json::from_slice(payload).ok()?;
        r.text = r.text.trim().to_string();
        if r.text.is_empty() {
            return None;
        }
        if r.text.chars().count() > MAX_SPEAK_CHARS {
            let cut: String = r.text.chars().take(MAX_SPEAK_CHARS).collect();
            let end = cut.rfind(['.', '!', '?']).map_or(cut.len(), |i| i + 1);
            r.text = cut[..end].to_string();
        }
        Some(r)
    }
}

/// Kokoro samples (-1..1 at `rate`) → 48 kHz mono i16 frames of 10 ms.
pub fn to_track_frames(samples: &[f32], rate: u32, framer: &mut Framer, resampler: &mut Resampler) -> Vec<Vec<i16>> {
    let pcm: Vec<i16> = samples.iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect();
    let pcm = if rate == TRACK_RATE { pcm } else { resampler.process(&pcm) };
    framer.push(&pcm)
}

/// Synthesize `text` sentence by sentence and hand each chunk's frames to
/// `on_frames` as soon as it renders (so the first words play while the rest
/// renders). `on_frames` returns false to stop (leave, speak_replies off).
/// Blocking: call from `spawn_blocking`.
pub fn speak_blocking(
    tts: &Arc<TtsEngine>,
    text: &str,
    voice: Option<&str>,
    mut on_frames: impl FnMut(Vec<Vec<i16>>) -> bool,
) -> Result<(), String> {
    let mut framer = Framer::new(TRACK_RATE, 10);
    let mut resampler: Option<Resampler> = None;
    let mut go_on = true;
    tts.synthesize_stream(text, voice, None, |_, _, samples, rate| {
        let r = resampler.get_or_insert_with(|| Resampler::new(rate, TRACK_RATE));
        go_on = on_frames(to_track_frames(samples, rate, &mut framer, r));
        go_on
    })?;
    if go_on {
        if let Some(last) = framer.flush() {
            on_frames(vec![last]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speak_payloads() {
        let r = SpeakRequest::parse(br#"{"turn_id":"wake-s1-1","text":"  Noted the action item.  "}"#).unwrap();
        assert_eq!((r.turn_id.as_str(), r.text.as_str()), ("wake-s1-1", "Noted the action item."));
        assert_eq!(SpeakRequest::parse(br#"{"turn_id":"t","text":"   "}"#), None);
        assert_eq!(SpeakRequest::parse(b"nope"), None);
        let long = format!("{} Tail without end", "One sentence. ".repeat(200));
        let r = SpeakRequest::parse(serde_json::json!({"text": long}).to_string().as_bytes()).unwrap();
        assert!(r.text.chars().count() <= MAX_SPEAK_CHARS);
        assert!(r.text.ends_with("sentence."));
    }
}
