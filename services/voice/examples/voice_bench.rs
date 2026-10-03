//! Voice bench harness (Phase 1 acceptance gate).
//!
//! Usage:
//!   cargo run --release -p voice-service --example voice_bench -- \
//!     --data ~/.allternit/voice-bench [--threads 2] [--limit 40]
//!
//! Expects `wb/*.wav` (clean 16 kHz), `pstn/*.wav` (phone-line 8 kHz μ-law
//! re-wrapped at 16 kHz), `pstn/*.8k.wav` (raw 8 kHz), and `refs.json`
//! (utterance key -> reference text, LibriSpeech test-clean).
//!
//! Writes JSONL lines to `services/voice/bench/results-<host>.jsonl`:
//! per-utterance STT rows, per-model/condition STT summaries (WER mean,
//! p50/p95 finalisation latency, RTF, peak RSS), and per-sentence TTS rows
//! (time-to-first-chunk, RTF, peak RSS).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use voice_service::audio::{decode_any, resample_to_16k};
use voice_service::models::PackManager;
use voice_service::stt::{SttEngine, SttModel};
use voice_service::tts::{split_sentences, TtsEngine};

/// Fixed TTS benchmark sentences (kept stable across runs).
const TTS_SENTENCES: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "Allternit turns your computer into an AI workspace.",
    "Voice input should feel instant, even on old hardware.",
    "She sells seashells by the seashore every Sunday morning.",
    "The committee approved the budget after a short recess.",
    "Please confirm your appointment by replying to this message.",
    "In 1969, Apollo eleven carried the first humans to the Moon.",
    "The recipe calls for two cups of flour and a pinch of salt.",
    "Reliable software is built in small, verifiable steps.",
    "Thank you for calling; how may I direct your call today?",
];

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut data: Option<PathBuf> = None;
    let mut threads = 2usize;
    let mut limit: Option<usize> = None;
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" => data = args.next().map(PathBuf::from),
            "--threads" => threads = args.next().and_then(|v| v.parse().ok()).unwrap_or(2),
            "--limit" => limit = args.next().and_then(|v| v.parse().ok()),
            "--out" => out = args.next().map(PathBuf::from),
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    let data = match data {
        Some(d) => d,
        None => {
            eprintln!("--data <dir> is required");
            std::process::exit(2);
        }
    };
    std::env::set_var("ALLTERNIT_VOICE_THREADS", threads.to_string());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    rt.block_on(async_main(data, threads, limit, out))
}

async fn async_main(
    data: PathBuf,
    threads: usize,
    limit: Option<usize>,
    out_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    let refs: BTreeMap<String, String> =
        serde_json::from_slice(&tokio::fs::read(data.join("refs.json")).await?)?;

    let manager = PackManager::new();
    let stt = Arc::new(SttEngine::new(manager.clone()));
    let tts = Arc::new(TtsEngine::new(manager));

    // ── STT ────────────────────────────────────────────────────────────────
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for (model, model_id) in [
        (SttModel::Moonshine, "moonshine-tiny-en"),
        (SttModel::Parakeet, "parakeet-tdt-0.6b-v3-int8"),
    ] {
        for (condition, dir_name, only_8k) in [
            ("wb", "wb", false),
            ("pstn", "pstn", false),
            ("pstn8k", "pstn", true),
        ] {
            let dir = data.join(dir_name);
            let mut wavs: Vec<PathBuf> = Vec::new();
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("wav") {
                    continue;
                }
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let is_8k = name.ends_with(".8k.wav");
                if is_8k != only_8k {
                    continue;
                }
                wavs.push(path);
            }
            wavs.sort();
            if let Some(limit) = limit {
                wavs.truncate(limit);
            }

            let mut wers = Vec::new();
            let mut latencies = Vec::new();
            let mut rtfs = Vec::new();
            for wav in &wavs {
                let key = wav
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .trim_end_matches(".8k")
                    .to_string();
                let reference = refs.get(&key).cloned().unwrap_or_default();
                let bytes = std::fs::read(wav)?;
                let (raw, rate) = decode_any(&bytes)?;
                let samples = resample_to_16k(&raw, rate);
                let audio_secs = samples.len() as f32 / 16_000.0;

                let engine = stt.clone();
                let start = Instant::now();
                let segments =
                    tokio::task::spawn_blocking(move || engine.transcribe(&samples, model))
                        .await??;
                let elapsed = start.elapsed();
                // Finalisation latency: end-of-speech → final text. With a
                // whole-utterance request that is the decode time itself.
                let latency_ms = elapsed.as_secs_f32() * 1000.0;
                let rtf = elapsed.as_secs_f32() / audio_secs.max(1e-3);
                let hypothesis: String = segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let wer = word_error_rate(&reference, &hypothesis);
                wers.push(wer);
                latencies.push(latency_ms);
                rtfs.push(rtf);
                rows.push(serde_json::json!({
                    "kind": "stt",
                    "model": model_id,
                    "condition": condition,
                    "utt": key,
                    "audio_secs": round3(audio_secs),
                    "wer": round4(wer),
                    "finalization_ms": round1(latency_ms),
                    "rtf": round4(rtf),
                }));
                println!(
                    "[stt] {model_id:24} {condition:6} {key:6} wer={:5.2}% lat={:6.0}ms rtf={:.3}",
                    wer * 100.0,
                    latency_ms,
                    rtf
                );
            }

            let peak_rss_mb = peak_rss_mb();
            rows.push(serde_json::json!({
                "kind": "stt_summary",
                "model": model_id,
                "condition": condition,
                "n": wers.len(),
                "wer_mean": round4(mean(&wers)),
                "wer_median": round4(median(&wers)),
                "finalization_p50_ms": round1(percentile(&latencies, 50.0)),
                "finalization_p95_ms": round1(percentile(&latencies, 95.0)),
                "rtf_mean": round4(mean(&rtfs)),
                "peak_rss_mb": round1(peak_rss_mb),
                "threads": threads,
            }));
            println!(
                "[sum] {model_id:24} {condition:6} WER {:5.2}% (med {:5.2}%)  p50 {:6.0}ms  p95 {:6.0}ms  rtf {:.3}  rss {}MB",
                mean(&wers) * 100.0,
                median(&wers) * 100.0,
                percentile(&latencies, 50.0),
                percentile(&latencies, 95.0),
                mean(&rtfs),
                peak_rss_mb
            );
        }
    }

    // ── TTS ────────────────────────────────────────────────────────────────
    {
        let engine = tts.clone();
        // Warm up (loads Kokoro) before timing.
        tokio::task::spawn_blocking(move || engine.synthesize("Warm up.", Some("af"), None))
            .await??;
        let mut ttfcs = Vec::new();
        let mut rtfs = Vec::new();
        for (i, sentence) in TTS_SENTENCES.iter().enumerate() {
            let engine = tts.clone();
            let text = sentence.to_string();
            let (samples, sample_rate, first, total) = tokio::task::spawn_blocking(move || {
                // Stream-shaped synthesis: sentence-at-a-time, timing the
                // first sentence's audio (TTFC) and the whole batch.
                let sentences = split_sentences(&text);
                let t0 = Instant::now();
                let mut all = Vec::new();
                let mut rate = 0;
                let mut first: Option<std::time::Duration> = None;
                for s in &sentences {
                    let (chunk, r) = engine.synthesize(s, Some("af"), None)?;
                    if rate == 0 {
                        rate = r;
                    }
                    if first.is_none() {
                        first = Some(t0.elapsed());
                    }
                    all.extend_from_slice(&chunk);
                }
                Ok::<_, String>((all, rate, first.unwrap_or_default(), t0.elapsed()))
            })
            .await??;
            let audio_secs = samples.len() as f32 / sample_rate as f32;
            let rtf = total.as_secs_f32() / audio_secs.max(1e-3);
            ttfcs.push(first.as_secs_f32() * 1000.0);
            rtfs.push(rtf);
            rows.push(serde_json::json!({
                "kind": "tts",
                "voice": "af",
                "sentence": i,
                "audio_secs": round3(audio_secs),
                "ttfc_ms": round1(first.as_secs_f32() * 1000.0),
                "rtf": round4(rtf),
            }));
            println!(
                "[tts] sentence {:2} ttfc={:6.0}ms rtf={:.3}",
                i,
                first.as_secs_f32() * 1000.0,
                rtf
            );
        }
        let peak_rss_mb = peak_rss_mb();
        rows.push(serde_json::json!({
            "kind": "tts_summary",
            "voice": "af",
            "n": ttfcs.len(),
            "ttfc_mean_ms": round1(mean(&ttfcs)),
            "ttfc_max_ms": round1(ttfcs.iter().fold(0.0f32, |m, v| m.max(*v))),
            "rtf_mean": round4(mean(&rtfs)),
            "peak_rss_mb": round1(peak_rss_mb),
            "threads": threads,
        }));
        println!(
            "[sum] tts ttfc mean {:5.0}ms max {:5.0}ms  rtf {:.3}  rss {}MB",
            mean(&ttfcs),
            ttfcs.iter().fold(0.0f32, |m, v| m.max(*v)),
            mean(&rtfs),
            peak_rss_mb
        );
    }

    let host = hostname();
    let out = out_override.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("bench")
            .join(format!("results-{host}.jsonl"))
    });
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(&out)?;
    use std::io::Write;
    for row in &rows {
        writeln!(file, "{}", serde_json::to_string(row)?)?;
    }
    println!("wrote {} rows to {}", rows.len(), out.display());
    Ok(())
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    unsafe {
        if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).replace('.', "-");
        }
    }
    "unknown".to_string()
}

/// Peak resident set size of this process, MB (ru_maxrss: bytes on macOS,
/// KiB on Linux).
fn peak_rss_mb() -> f32 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            #[cfg(target_os = "linux")]
            return usage.ru_maxrss as f32 / 1024.0;
            #[cfg(target_os = "macos")]
            return usage.ru_maxrss as f32 / (1024.0 * 1024.0);
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            return usage.ru_maxrss as f32 / 1024.0;
        }
    }
    0.0
}

/// Word error rate over normalised tokens: lowercase, punctuation stripped,
/// hyphens become spaces.
fn word_error_rate(reference: &str, hypothesis: &str) -> f32 {
    let norm = |s: &str| -> Vec<String> {
        s.chars()
            .map(|c| {
                if c == '-' || c.is_whitespace() {
                    ' '
                } else {
                    c
                }
            })
            .collect::<String>()
            .chars()
            .map(|c| {
                if c.is_ascii_punctuation() {
                    ' '
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect::<String>()
            .split_whitespace()
            .map(|w| w.to_string())
            .collect()
    };
    let r = norm(reference);
    let h = norm(hypothesis);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let n = r.len();
    let m = h.len();
    // Levenshtein over words with standard DP.
    let mut prev: Vec<u32> = (0..=m as u32).collect();
    let mut cur = vec![0u32; m + 1];
    for i in 1..=n {
        cur[0] = i as u32;
        for j in 1..=m {
            let cost = if r[i - 1] == h[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m] as f32 / n as f32
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.iter().sum::<f32>() / v.len() as f32
}

fn median(v: &[f32]) -> f32 {
    percentile(v, 50.0)
}

fn percentile(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((p / 100.0) * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}
fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}
fn round4(v: f32) -> f32 {
    (v * 10000.0).round() / 10000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wer_exact_and_normalised() {
        assert_eq!(word_error_rate("Hello world", "hello world"), 0.0);
        assert_eq!(word_error_rate("well-known fact", "well known fact"), 0.0);
        assert_eq!(word_error_rate("One, two! Three?", "one two three"), 0.0);
        let wer = word_error_rate("the cat sat", "the dog sat fast");
        assert!((wer - 0.6666667).abs() < 1e-4);
        assert_eq!(word_error_rate("", ""), 0.0);
        assert_eq!(word_error_rate("a b", ""), 1.0);
    }
}
