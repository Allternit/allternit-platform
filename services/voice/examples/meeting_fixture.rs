//! Offline meeting-worker run on recorded call audio.
//!
//! ```bash
//! cargo run -p voice-service --example meeting_fixture -- \
//!   services/meeting-bot/spike/out/local-capture/call.webm \
//!   [audio start epoch ms, default: audio/capture-start-ms beside the file]
//! ```
//!
//! Decodes the audio with `ffmpeg` (any format it reads) to 16 kHz mono and
//! prints what the worker would send: each segment, and each wake-word turn.
//! Lines starting `STT ` and `TURN ` are the exact `allternit.meeting.stt` /
//! `allternit.meeting.turn` payloads, so they can be piped into the pod's
//! speaker labeller (see spec/MEETING_WORKER.md "Offline check"). Needs the
//! `small` model pack (downloads on first use).

use std::process::Command;
use std::sync::Arc;

use voice_service::meeting_worker::{fixture, pipeline::Output};
use voice_service::models::PackManager;
use voice_service::stt::{SttEngine, SttModel};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(audio) = args.first().cloned() else {
        anyhow::bail!("usage: meeting_fixture <audio> [audio-start-epoch-ms]");
    };
    let epoch: i64 = match args.get(1) {
        Some(v) => v.parse()?,
        None => {
            let p = std::path::Path::new(&audio).with_file_name("audio").join("capture-start-ms");
            std::fs::read_to_string(&p)
                .map_err(|e| anyhow::anyhow!("{}: {e} (pass the start epoch ms)", p.display()))?
                .trim()
                .parse()?
        }
    };

    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i", &audio, "-ac", "1", "-ar", "16000", "-f", "s16le", "-"])
        .output()?;
    anyhow::ensure!(out.status.success(), "ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
    let samples: Vec<f32> = out.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
    eprintln!("audio {:.1} s from {audio}; first sample at epoch {epoch}", samples.len() as f32 / 16_000.0);

    let engine = Arc::new(SttEngine::new(Arc::new(PackManager::new())));
    let model = SttModel::parse(std::env::var("MEETING_WORKER_STT_MODEL").ok().as_deref()).map_err(anyhow::Error::msg)?;
    let t = std::time::Instant::now();
    let outputs = fixture::run(&engine, model, "fixture-session", &samples, epoch).map_err(anyhow::Error::msg)?;
    eprint!("{}", fixture::render(&outputs));
    let mut turns = 0;
    for o in &outputs {
        match o {
            Output::Segment(s) => println!("STT {}", serde_json::to_string(s)?),
            Output::Turn(tu) => {
                turns += 1;
                println!("TURN {}", serde_json::to_string(tu)?);
            }
        }
    }
    eprintln!(
        "{} segments, {turns} turn(s), model {}, {:.1} s wall",
        outputs.len() - turns,
        model.id(),
        t.elapsed().as_secs_f32()
    );
    Ok(())
}
