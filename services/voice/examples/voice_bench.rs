//! Voice bench (Phase 1 acceptance gate): STT accuracy/latency and TTS
//! latency on the target-machine settings (CPU only, 2 inference threads).
//!
//! ```text
//! cargo run --release -p voice-service --example voice_bench -- \
//!     --data ~/.allternit/voice-bench [--threads 2] [--limit N] [--out FILE]
//! ```
//!
//! `--data` holds `wb/*.wav` (clean 16 kHz), `pstn/*.wav` (phone line:
//! 300–3400 Hz, 8 kHz μ-law, back to 16 kHz), `pstn/*.8k.wav` (raw 8 kHz
//! μ-law) and `refs.json` (key -> LibriSpeech test-clean reference).
//!
//! Every model runs in its own child process (`--only <job>`), so the peak
//! RSS reported for a model is that model's alone. Output is JSONL, default
//! `services/voice/bench/results-<host>.jsonl`:
//! - `{"kind":"stt", model, cond, n, wer, fin_p50_ms, fin_p95_ms, rtf, peak_rss_mb, threads}`
//! - `{"kind":"tts", model, n, ttfa_p50_ms, ttfa_p95_ms, ttfa_max_ms, rtf, peak_rss_mb, child_peak_rss_mb, threads}`
//!   (`child_peak_rss_mb` is the `allternit-tts` process, where Kokoro runs)
//!
//! Definitions:
//! - WER: corpus WER (total edits / total reference words) after lowercase,
//!   hyphens -> spaces, punctuation stripped.
//! - Finalisation latency: the utterance is streamed through `SttStream` in
//!   100 ms chunks as fast as possible; latency is the wall time of feeding
//!   the last chunk plus `finish()`, i.e. from the last audio sample arriving
//!   to the final transcript being available. Excludes the VAD's 0.3 s
//!   end-of-speech hangover (end-of-turn detection is Phase 1b's job).
//! - STT RTF: batch `transcribe()` wall time / audio duration.
//! - TTS time to first audio (ttfa): for each of 10 fixed sentences, wall
//!   time until `TtsEngine::synthesize_stream` delivers its first audio
//!   (the first clause, rendered by the `allternit-tts` child), as
//!   `/v1/tts/stream` does. Needs `allternit-tts` built next to the bench
//!   (`cargo build --release -p allternit-tts`).
//! - TTS RTF: synthesis wall time / audio duration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use voice_service::audio::{decode_wav, resample_to_16k};
use voice_service::models::PackManager;
use voice_service::stt::{SttEngine, SttEvent, SttModel};
use voice_service::tts::TtsEngine;

const TTS_SENTENCES: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "Allternit turns your computer into an AI workspace.",
    "Voice input should feel instant, even on older hardware.",
    "She sells seashells by the seashore every Sunday morning.",
    "The committee approved the budget after a short recess.",
    "Please confirm your appointment by replying to this message.",
    "In nineteen sixty nine, Apollo eleven carried the first people to the Moon.",
    "The recipe calls for two cups of flour and a pinch of salt.",
    "Reliable software is built in small, verifiable steps.",
    "Thank you for calling; how may I direct your call today?",
];

const CONDITIONS: &[(&str, &str, bool)] = &[
    // (cond, dir, raw 8 kHz files only)
    ("wb", "wb", false),
    ("pstn", "pstn", false),
    ("pstn8k", "pstn", true),
];

struct Args {
    data: PathBuf,
    threads: usize,
    limit: Option<usize>,
    out: Option<PathBuf>,
    only: Option<String>,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut a = Args {
        data: PathBuf::new(),
        threads: 2,
        limit: None,
        out: None,
        only: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" => a.data = args.next().map(expand_home).unwrap_or_default(),
            "--threads" => a.threads = args.next().and_then(|v| v.parse().ok()).unwrap_or(2),
            "--limit" => a.limit = args.next().and_then(|v| v.parse().ok()),
            "--out" => a.out = args.next().map(PathBuf::from),
            "--only" => a.only = args.next(),
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    if a.data.as_os_str().is_empty() {
        eprintln!("--data <dir> is required");
        std::process::exit(2);
    }
    a
}

fn expand_home(p: String) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(&p)),
        None => PathBuf::from(p),
    }
}

fn main() -> anyhow::Result<()> {
    let args = parse_args();
    std::env::set_var("ALLTERNIT_VOICE_THREADS", args.threads.to_string());
    match args.only.as_deref() {
        Some(job) => run_job(job, &args),
        None => run_all(&args),
    }
}

/// Parent: run each job in a child process and collect its JSONL.
fn run_all(args: &Args) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut lines = Vec::new();
    for job in ["moonshine", "parakeet", "tts"] {
        eprintln!("== {job}");
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--data")
            .arg(&args.data)
            .arg("--threads")
            .arg(args.threads.to_string())
            .arg("--only")
            .arg(job)
            .stderr(std::process::Stdio::inherit());
        if let Some(l) = args.limit {
            cmd.arg("--limit").arg(l.to_string());
        }
        let out = cmd.output()?;
        if !out.status.success() {
            anyhow::bail!("job {job} failed: {}", out.status);
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.starts_with('{') {
                println!("{line}");
                lines.push(line.to_string());
            }
        }
    }
    let path = args.out.clone().unwrap_or_else(default_out);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, lines.join("\n") + "\n")?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

fn default_out() -> PathBuf {
    let host = hostname();
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("bench")
        .join(format!("results-{host}.jsonl"))
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Child: one model, all conditions. JSONL on stdout, progress on stderr.
fn run_job(job: &str, args: &Args) -> anyhow::Result<()> {
    // The pack manager needs a runtime context only to download missing
    // packs; enter one for the whole job.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let _enter = rt.enter();
    let packs = Arc::new(PackManager::new());
    match job {
        "moonshine" => bench_stt(SttModel::Moonshine, packs, args),
        "parakeet" => bench_stt(SttModel::Parakeet, packs, args),
        "tts" => bench_tts(packs, args),
        other => anyhow::bail!("unknown job {other}"),
    }
}

fn bench_stt(model: SttModel, packs: Arc<PackManager>, args: &Args) -> anyhow::Result<()> {
    let refs: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(args.data.join("refs.json"))?)?;
    let engine = Arc::new(SttEngine::new(packs));
    let load = Instant::now();
    engine.prepare(model).map_err(anyhow::Error::msg)?;
    eprintln!("{} loaded in {:?}", model.id(), load.elapsed());

    for &(cond, dir, raw8k) in CONDITIONS {
        let files = list_wavs(&args.data.join(dir), raw8k, args.limit)?;
        let (mut edits, mut ref_words) = (0usize, 0usize);
        let mut fin_ms = Vec::new();
        let (mut proc_secs, mut audio_secs) = (0f64, 0f64);
        for path in &files {
            let key = utt_key(path);
            let Some(reference) = refs.get(&key) else {
                continue;
            };
            let (raw, rate) = decode_wav(&std::fs::read(path)?)?;
            let samples = resample_to_16k(&raw, rate);
            let dur = samples.len() as f64 / 16_000.0;

            // Batch: RTF + the transcript scored for WER.
            let t = Instant::now();
            let segments = engine
                .transcribe(&samples, model)
                .map_err(anyhow::Error::msg)?;
            proc_secs += t.elapsed().as_secs_f64();
            audio_secs += dur;
            let hyp = segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let (e, n) = word_edits(reference, &hyp);
            edits += e;
            ref_words += n;

            // Streaming: finalisation latency after the last sample.
            let mut stream = engine.stream(model).map_err(anyhow::Error::msg)?;
            let chunks: Vec<&[f32]> = samples.chunks(1_600).collect();
            let (last, rest) = chunks.split_last().expect("non-empty audio");
            for c in rest {
                let _ = stream.feed(c);
            }
            let t = Instant::now();
            let mut events = stream.feed(last);
            events.extend(stream.finish());
            fin_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            let _finals = events
                .iter()
                .filter(|e| matches!(e, SttEvent::Final(_)))
                .count();
            if e > 0 && std::env::var_os("BENCH_VERBOSE").is_some() {
                eprintln!("  {cond}/{key} edits={e}/{n}\n    ref: {reference}\n    hyp: {hyp}");
            }
        }
        let row = serde_json::json!({
            "kind": "stt",
            "model": model.id(),
            "cond": cond,
            "n": fin_ms.len(),
            "wer": round2(100.0 * edits as f64 / ref_words.max(1) as f64),
            "fin_p50_ms": percentile(&mut fin_ms, 50.0).round(),
            "fin_p95_ms": percentile(&mut fin_ms, 95.0).round(),
            "rtf": round3(proc_secs / audio_secs.max(1e-9)),
            "peak_rss_mb": peak_rss_mb(),
            "threads": args.threads,
        });
        eprintln!("{row}");
        println!("{row}");
    }
    Ok(())
}

fn bench_tts(packs: Arc<PackManager>, args: &Args) -> anyhow::Result<()> {
    let engine = TtsEngine::new(packs);
    let load = Instant::now();
    engine.prepare().map_err(anyhow::Error::msg)?;
    eprintln!("kokoro loaded in {:?}", load.elapsed());
    // Warm-up (first inference allocates arenas); not counted.
    engine
        .synthesize("Warm up.", None, None)
        .map_err(anyhow::Error::msg)?;

    let mut ttfa = Vec::new();
    let (mut proc_secs, mut audio_secs) = (0f64, 0f64);
    for text in TTS_SENTENCES {
        let t = Instant::now();
        let mut first = None;
        engine
            .synthesize_stream(text, None, None, |_, _, samples, rate| {
                first.get_or_insert(t.elapsed().as_secs_f64() * 1000.0);
                audio_secs += samples.len() as f64 / rate as f64;
                true
            })
            .map_err(anyhow::Error::msg)?;
        proc_secs += t.elapsed().as_secs_f64();
        ttfa.push(first.unwrap_or(0.0));
    }
    // Kokoro runs in the allternit-tts child: stop it (drop = kill + wait)
    // so its peak RSS is reported through RUSAGE_CHILDREN.
    drop(engine);
    let child_rss = peak_rss_children_mb();
    let max = ttfa.iter().cloned().fold(0.0, f64::max);
    let row = serde_json::json!({
        "kind": "tts",
        "model": "kokoro-multi-lang-v1_0 (fp32, allternit-tts)",
        "n": ttfa.len(),
        "ttfa_p50_ms": percentile(&mut ttfa, 50.0).round(),
        "ttfa_p95_ms": percentile(&mut ttfa, 95.0).round(),
        "ttfa_max_ms": max.round(),
        "rtf": round3(proc_secs / audio_secs.max(1e-9)),
        "peak_rss_mb": peak_rss_mb(),
        "child_peak_rss_mb": child_rss,
        "threads": args.threads,
    });
    eprintln!("{row}");
    println!("{row}");
    Ok(())
}

/// Peak RSS of reaped child processes in MB (None on Windows).
fn peak_rss_children_mb() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) } != 0 {
            return None;
        }
        let raw = usage.ru_maxrss as f64;
        let bytes = if cfg!(target_os = "macos") {
            raw
        } else {
            raw * 1024.0
        };
        Some((bytes / 1_048_576.0).round())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn list_wavs(dir: &Path, raw8k: bool, limit: Option<usize>) -> anyhow::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with(".wav") && name.ends_with(".8k.wav") == raw8k
        })
        .collect();
    out.sort();
    if let Some(l) = limit {
        out.truncate(l);
    }
    Ok(out)
}

/// `u07.8k.wav` / `u07.wav` -> `u07`.
fn utt_key(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.split('.').next().unwrap_or("").to_string()
}

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('-', " ")
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// (word edit distance, reference word count).
fn word_edits(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r = normalize(reference);
    let h = normalize(hypothesis);
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut cur = vec![i; h.len() + 1];
        for j in 1..=h.len() {
            let sub = prev[j - 1] + usize::from(r[i - 1] != h[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    (prev[h.len()], r.len())
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Peak resident set size of this process in MB (None on Windows).
fn peak_rss_mb() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
            return None;
        }
        let raw = usage.ru_maxrss as f64;
        // macOS reports bytes, Linux kilobytes.
        let bytes = if cfg!(target_os = "macos") {
            raw
        } else {
            raw * 1024.0
        };
        Some((bytes / 1_048_576.0).round())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wer_normalisation() {
        assert_eq!(word_edits("Hello, world!", "hello world"), (0, 2));
        assert_eq!(word_edits("well-known fact", "well known fact"), (0, 3));
        assert_eq!(word_edits("a b c", "a x c d"), (2, 3));
    }
}
