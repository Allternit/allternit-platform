//! allternit-tts: Kokoro text-to-speech as a child process.
//!
//! This is the GPL-3.0-or-later part of Allternit voice: sherpa-onnx's
//! Kokoro frontend phonemises with espeak-ng (GPL-3.0-or-later), so the TTS
//! engine lives in its own program. `allternit-voice-service` runs it as a
//! child and talks to it over stdin/stdout (protocol in README.md):
//!
//! - stdin: one JSON request per line, `{"id","text","sid","speed"}`.
//! - stdout: frames `[kind u8][len u32 LE][payload]`; kind `J` = JSON event
//!   (`ready`, `chunk`, `done`, `error`), kind `P` = s16le mono PCM that
//!   belongs to the preceding `chunk` event.
//! - stdin EOF (the parent exited or closed the pipe) ends the process.
//!
//! Logs go to stderr.

use serde::Deserialize;
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig,
};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Request {
    id: String,
    text: String,
    #[serde(default)]
    sid: i32,
    #[serde(default)]
    speed: Option<f32>,
}

type Out = Arc<Mutex<std::io::BufWriter<std::io::Stdout>>>;

fn write_frame(out: &Out, kind: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut w = out.lock().unwrap_or_else(|e| e.into_inner());
    w.write_all(&[kind])?;
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn write_json(out: &Out, value: serde_json::Value) -> std::io::Result<()> {
    write_frame(out, b'J', value.to_string().as_bytes())
}

/// Emit one audio chunk: a `chunk` event, then the PCM frame.
fn write_chunk(out: &Out, id: &str, index: usize, samples: &[f32]) -> std::io::Result<()> {
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        pcm.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    write_json(
        out,
        serde_json::json!({"type": "chunk", "id": id, "index": index, "samples": samples.len()}),
    )?;
    write_frame(out, b'P', &pcm)
}

struct Args {
    model_dir: PathBuf,
    threads: i32,
}

fn parse_args() -> Result<Args, String> {
    let mut model_dir = None;
    let mut threads = std::env::var("ALLTERNIT_VOICE_THREADS")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--model-dir" => model_dir = it.next().map(PathBuf::from),
            "--threads" => {
                threads = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--threads needs a number")?
            }
            "--version" => {
                println!("allternit-tts {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        model_dir: model_dir.ok_or("--model-dir <kokoro dir> is required")?,
        threads,
    })
}

fn load(dir: &Path, threads: i32) -> Result<OfflineTts, String> {
    let path = |name: &str| -> Option<String> {
        let p = dir.join(name);
        p.exists().then(|| p.display().to_string())
    };
    let model = path("model.onnx")
        .or_else(|| path("model.int8.onnx"))
        .ok_or_else(|| format!("no Kokoro model in {}", dir.display()))?;
    let need = |name: &str| path(name).ok_or_else(|| format!("{name} missing in {}", dir.display()));
    // Kokoro v1.0 ships an English lexicon (espeak-ng covers words not in it).
    let lexicon = path("lexicon-us-en.txt");
    let config = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: Some(model),
                voices: Some(need("voices.bin")?),
                tokens: Some(need("tokens.txt")?),
                data_dir: Some(need("espeak-ng-data")?),
                lang: lexicon.as_ref().map(|_| "en-us".to_string()),
                lexicon,
                length_scale: 1.0,
                ..Default::default()
            },
            num_threads: threads,
            provider: Some("cpu".to_string()),
            debug: false,
            ..Default::default()
        },
        max_num_sentences: 1,
        ..Default::default()
    };
    OfflineTts::create(&config).ok_or_else(|| "failed to create Kokoro TTS".to_string())
}

/// Synthesise one request, streaming every piece sherpa-onnx hands to the
/// progress callback as soon as it exists.
fn handle(tts: &OfflineTts, out: &Out, req: Request) -> std::io::Result<()> {
    if req.sid < 0 || req.sid >= tts.num_speakers() {
        return write_json(
            out,
            serde_json::json!({"type": "error", "id": req.id,
                "error": format!("sid {} out of range (0..{})", req.sid, tts.num_speakers())}),
        );
    }
    let config = GenerationConfig {
        sid: req.sid,
        speed: req.speed.unwrap_or(1.0).clamp(0.5, 2.0),
        ..Default::default()
    };
    let index = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    let (cb_out, cb_id, cb_index, cb_total) =
        (out.clone(), req.id.clone(), index.clone(), total.clone());
    // Kokoro renders a sentence per call (max_num_sentences = 1) and hands
    // each one to this callback; the parent sends short chunks, so the
    // first audio leaves as soon as the first chunk is rendered.
    let callback = move |samples: &[f32], _progress: f32| -> bool {
        if samples.is_empty() {
            return true;
        }
        let i = cb_index.fetch_add(1, Ordering::SeqCst);
        cb_total.fetch_add(samples.len(), Ordering::SeqCst);
        write_chunk(&cb_out, &cb_id, i, samples).is_ok()
    };
    match tts.generate_with_config(&req.text, &config, Some(callback)) {
        Some(audio) => {
            // Fallback: a build whose callback never fired still answers.
            if index.load(Ordering::SeqCst) == 0 && !audio.samples().is_empty() {
                write_chunk(out, &req.id, 0, audio.samples())?;
                total.store(audio.samples().len(), Ordering::SeqCst);
            }
            write_json(
                out,
                serde_json::json!({"type": "done", "id": req.id,
                    "samples": total.load(Ordering::SeqCst)}),
            )
        }
        None => write_json(
            out,
            serde_json::json!({"type": "error", "id": req.id, "error": "Kokoro generation failed"}),
        ),
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("allternit-tts: {e}");
            std::process::exit(2);
        }
    };
    let out: Out = Arc::new(Mutex::new(std::io::BufWriter::new(std::io::stdout())));
    let tts = match load(&args.model_dir, args.threads) {
        Ok(t) => t,
        Err(e) => {
            let _ = write_json(&out, serde_json::json!({"type": "error", "id": null, "error": e}));
            eprintln!("allternit-tts: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "allternit-tts: Kokoro ready ({} speakers, {} Hz, {} threads)",
        tts.num_speakers(),
        tts.sample_rate(),
        args.threads
    );
    if write_json(
        &out,
        serde_json::json!({"type": "ready", "sample_rate": tts.sample_rate(),
            "num_speakers": tts.num_speakers()}),
    )
    .is_err()
    {
        return;
    }

    // stdin EOF = the parent is gone: exit.
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let result = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(&tts, &out, req),
            Err(e) => write_json(
                &out,
                serde_json::json!({"type": "error", "id": null, "error": format!("bad request: {e}")}),
            ),
        };
        if result.is_err() {
            break; // stdout closed: the parent is gone
        }
    }
}
