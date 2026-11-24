//! Offline run of the meeting pipeline on recorded call audio: no LiveKit,
//! no cloud-api. Used by `examples/meeting_fixture.rs` to check the worker on
//! real captures (`services/meeting-bot/spike/out/<run>/call.webm`).
//!
//! The audio is fed in 20 ms chunks, the size the live worker gets from
//! LiveKit, so VAD timing matches the live path.

use std::sync::Arc;

use super::pipeline::{Output, SegmentProcessor, Transcriber};
use crate::stt::{SttEngine, SttModel, SAMPLE_RATE};

/// Run 16 kHz mono audio whose first sample is at `audio_epoch_ms` through
/// the pipeline, as if it arrived on `call-audio`. Blocking.
pub fn run(engine: &Arc<SttEngine>, model: SttModel, session_id: &str, audio_16k: &[f32], audio_epoch_ms: i64) -> Result<Vec<Output>, String> {
    let mut tr = Transcriber::new(engine, model, audio_epoch_ms, SegmentProcessor::new(session_id, audio_epoch_ms))?;
    let mut out = Vec::new();
    for chunk in audio_16k.chunks(SAMPLE_RATE as usize / 50) {
        out.extend(tr.feed(chunk));
    }
    out.extend(tr.finish().0);
    Ok(out)
}

/// `[mm:ss.s–mm:ss.s] text` lines and `TURN …` lines.
pub fn render(outputs: &[Output]) -> String {
    let ts = |ms: i64| format!("{:02}:{:04.1}", ms / 60_000, (ms % 60_000) as f64 / 1000.0);
    let mut s = String::new();
    for o in outputs {
        match o {
            Output::Segment(g) => s.push_str(&format!("[{}–{}] {}\n", ts(g.start_ms), ts(g.end_ms), g.text)),
            Output::Turn(t) => s.push_str(&format!(
                "TURN at {} heard={:?} confidence={:.2} text={:?}\n",
                ts(t.start_ms),
                t.heard,
                t.confidence,
                t.text
            )),
        }
    }
    s
}
