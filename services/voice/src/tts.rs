//! Text-to-speech: Kokoro-82M v1.0 (fp32) through the `allternit-tts` child
//! process.
//!
//! sherpa-onnx's Kokoro frontend links espeak-ng (GPL-3.0-or-later), so the
//! TTS engine runs as a separate GPL program (`services/voice-tts`, binary
//! `allternit-tts`) and this module is its client: it owns the voice table,
//! splits text into short chunks, and streams each chunk's audio back as the
//! child renders it. This crate (and `allternit-voice-service`) contains no
//! espeak-ng code.
//!
//! Voices and speaker ids (`sid`) are the English voices of the sherpa-onnx
//! `kokoro-multi-lang-v1_0` export, in the order of the model's
//! `speaker_names` metadata (sid 0..27).

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

use crate::models::{inference_lock, inference_threads, PackManager, KOKORO_DIR};
use crate::phrase_cache::PhraseCache;

/// One Kokoro voice (sid == index in [`VOICES`]).
#[derive(Debug, Clone, Copy)]
pub struct VoiceDef {
    pub id: &'static str,
    pub name: &'static str,
    pub language: &'static str,
    pub gender: &'static str,
}

/// Kokoro output rate.
pub const KOKORO_SAMPLE_RATE: u32 = 24_000;

pub const VOICES: &[VoiceDef] = &[
    VoiceDef {
        id: "af_alloy",
        name: "Alloy",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_aoede",
        name: "Aoede",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_bella",
        name: "Bella",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_heart",
        name: "Heart",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_jessica",
        name: "Jessica",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_kore",
        name: "Kore",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_nicole",
        name: "Nicole",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_nova",
        name: "Nova",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_river",
        name: "River",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_sarah",
        name: "Sarah",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "af_sky",
        name: "Sky",
        language: "en-US",
        gender: "female",
    },
    VoiceDef {
        id: "am_adam",
        name: "Adam",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_echo",
        name: "Echo",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_eric",
        name: "Eric",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_fenrir",
        name: "Fenrir",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_liam",
        name: "Liam",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_michael",
        name: "Michael",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_onyx",
        name: "Onyx",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_puck",
        name: "Puck",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "am_santa",
        name: "Santa",
        language: "en-US",
        gender: "male",
    },
    VoiceDef {
        id: "bf_alice",
        name: "Alice",
        language: "en-GB",
        gender: "female",
    },
    VoiceDef {
        id: "bf_emma",
        name: "Emma",
        language: "en-GB",
        gender: "female",
    },
    VoiceDef {
        id: "bf_isabella",
        name: "Isabella",
        language: "en-GB",
        gender: "female",
    },
    VoiceDef {
        id: "bf_lily",
        name: "Lily",
        language: "en-GB",
        gender: "female",
    },
    VoiceDef {
        id: "bm_daniel",
        name: "Daniel",
        language: "en-GB",
        gender: "male",
    },
    VoiceDef {
        id: "bm_fable",
        name: "Fable",
        language: "en-GB",
        gender: "male",
    },
    VoiceDef {
        id: "bm_george",
        name: "George",
        language: "en-GB",
        gender: "male",
    },
    VoiceDef {
        id: "bm_lewis",
        name: "Lewis",
        language: "en-GB",
        gender: "male",
    },
];

/// Kokoro v1.0's recommended voice.
pub const DEFAULT_VOICE: &str = "af_heart";

/// Resolve a requested voice id. "default", the old stub ids
/// (`en-us-female`, `en-us-male`) and the v0.19 default `af` map to real
/// voices.
pub fn resolve_voice(requested: Option<&str>) -> Result<&'static VoiceDef, String> {
    let id = match requested.map(str::trim).unwrap_or("") {
        "" | "default" | "en-us-female" | "af" => DEFAULT_VOICE,
        "en-us-male" => "am_adam",
        other => other,
    };
    VOICES
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| format!("unknown voice '{id}' (see GET /v1/voices)"))
}

/// Kokoro speaker index (sid) of a voice.
pub fn voice_sid(v: &VoiceDef) -> i32 {
    VOICES.iter().position(|x| x.id == v.id).unwrap_or(0) as i32
}

/// Path of the `allternit-tts` program: `ALLTERNIT_TTS_BIN`, else next to
/// this executable (Desktop ships both in `resources/bin`; cargo builds both
/// into the same target dir), else one directory up (cargo test binaries
/// live in `target/<profile>/deps`).
pub fn tts_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ALLTERNIT_TTS_BIN") {
        return Some(PathBuf::from(p));
    }
    let name = format!("allternit-tts{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let found = [Some(dir), dir.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join(&name))
        .find(|p| p.is_file());
    found
}

/// A running `allternit-tts`. Killed when dropped; it also exits on its own
/// when our end of its stdin closes (e.g. this process dies).
struct TtsChild {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sample_rate: u32,
    next_id: u64,
}

impl Drop for TtsChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_frame(r: &mut impl Read) -> std::io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((head[0], payload))
}

fn read_json(r: &mut impl Read) -> Result<serde_json::Value, String> {
    let (kind, payload) = read_frame(r).map_err(|e| format!("allternit-tts read: {e}"))?;
    if kind != b'J' {
        return Err(format!("allternit-tts: expected a JSON frame, got {kind:#x}"));
    }
    serde_json::from_slice(&payload).map_err(|e| format!("allternit-tts sent bad JSON: {e}"))
}

/// Why a request failed: the child is broken (restart it) or the request
/// itself was rejected.
enum ChildError {
    Dead(String),
    Rejected(String),
}

impl TtsChild {
    fn spawn(bin: &Path, model_dir: &Path, threads: i32) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .arg("--model-dir")
            .arg(model_dir)
            .arg("--threads")
            .arg(threads.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("start {}: {e}", bin.display()))?;
        let stdin = child.stdin.take().ok_or("allternit-tts: no stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("allternit-tts: no stdout")?);
        let ready = read_json(&mut stdout)?;
        if ready["type"] != "ready" {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "allternit-tts failed to start: {}",
                ready["error"].as_str().unwrap_or("unknown error")
            ));
        }
        let sample_rate = ready["sample_rate"].as_u64().unwrap_or(KOKORO_SAMPLE_RATE as u64) as u32;
        info!(
            "allternit-tts ready ({} speakers, {sample_rate} Hz, {threads} threads)",
            ready["num_speakers"]
        );
        Ok(Self {
            child,
            stdin,
            stdout,
            sample_rate,
            next_id: 0,
        })
    }

    /// Synthesise one chunk; `on_audio` gets each piece as it arrives and
    /// returns how many pieces it has seen. `streamed` reports whether any
    /// audio was delivered (a crash after that must not retry).
    fn request(
        &mut self,
        text: &str,
        sid: i32,
        speed: f32,
        streamed: &mut bool,
        on_audio: &mut dyn FnMut(&[f32]),
    ) -> Result<(), ChildError> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let line = serde_json::json!({"id": id, "text": text, "sid": sid, "speed": speed});
        writeln!(self.stdin, "{line}")
            .and_then(|_| self.stdin.flush())
            .map_err(|e| ChildError::Dead(format!("allternit-tts write: {e}")))?;
        loop {
            let event = read_json(&mut self.stdout).map_err(ChildError::Dead)?;
            match event["type"].as_str() {
                Some("chunk") => {
                    let (kind, pcm) = read_frame(&mut self.stdout)
                        .map_err(|e| ChildError::Dead(format!("allternit-tts read: {e}")))?;
                    if kind != b'P' {
                        return Err(ChildError::Dead("allternit-tts: missing PCM frame".into()));
                    }
                    let samples: Vec<f32> = pcm
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                        .collect();
                    *streamed = true;
                    on_audio(&samples);
                }
                Some("done") => return Ok(()),
                Some("error") => {
                    return Err(ChildError::Rejected(
                        event["error"].as_str().unwrap_or("allternit-tts error").to_string(),
                    ))
                }
                _ => return Err(ChildError::Dead(format!("allternit-tts: unexpected {event}"))),
            }
        }
    }
}

pub struct TtsEngine {
    packs: Arc<PackManager>,
    threads: i32,
    child: Mutex<Option<TtsChild>>,
    /// Pre-rendered fixed phrases (acks, fillers, call disclosures).
    cache: Option<Arc<PhraseCache>>,
}

impl TtsEngine {
    pub fn new(packs: Arc<PackManager>) -> Self {
        let cache = PhraseCache::from_env(&packs.root_dir()).map(Arc::new);
        Self {
            packs,
            threads: inference_threads(),
            child: Mutex::new(None),
            cache,
        }
    }

    /// Mark texts (a call's disclosure and greeting) to be stored in the
    /// phrase cache when first rendered, and render them now in the
    /// background so the first playback is already a cache hit.
    pub fn register_phrases(self: &Arc<Self>, texts: &[String], voice: Option<&str>) {
        let Some(cache) = &self.cache else { return };
        // The cache is keyed by rendered chunk, so register the chunks.
        for t in texts {
            for chunk in split_for_streaming(t) {
                cache.register(&chunk);
            }
        }
        self.prerender(texts.to_vec(), voice.map(str::to_string));
    }

    /// Render the built-in acknowledgements and fillers for `voice` in the
    /// background (idempotent; already cached phrases are skipped).
    pub fn prewarm_fixed_phrases(self: &Arc<Self>, voice: Option<&str>) {
        let texts = crate::phrase_cache::fixed_phrases().map(String::from).collect();
        self.prerender(texts, voice.map(str::to_string));
    }

    fn prerender(self: &Arc<Self>, texts: Vec<String>, voice: Option<String>) {
        if self.cache.is_none() {
            return;
        }
        let me = self.clone();
        let _ = std::thread::Builder::new().name("tts-prerender".into()).spawn(move || {
            for t in texts {
                // Each call renders (and stores) at most the uncached chunks.
                if let Err(e) = me.synthesize_stream(&t, voice.as_deref(), None, |_, _, _, _| true) {
                    warn!("phrase pre-render failed: {e}");
                    break;
                }
            }
        });
    }

    pub fn num_threads(&self) -> i32 {
        self.threads
    }

    /// True while an `allternit-tts` child is running with the model loaded.
    pub fn is_ready(&self) -> bool {
        self.child.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Download the `tts` pack (first use) and start `allternit-tts`.
    /// Blocking.
    pub fn prepare(&self) -> Result<(), String> {
        let mut guard = self.lock_child()?;
        self.ensure_child(&mut guard).map(|_| ())
    }

    fn lock_child(&self) -> Result<std::sync::MutexGuard<'_, Option<TtsChild>>, String> {
        self.child
            .lock()
            .map_err(|e| format!("TTS engine lock poisoned: {e}"))
    }

    fn ensure_child<'a>(
        &self,
        guard: &'a mut Option<TtsChild>,
    ) -> Result<&'a mut TtsChild, String> {
        if guard.is_none() {
            let dir = self.packs.ensure_blocking("tts")?.join(KOKORO_DIR);
            let bin = tts_binary().ok_or(
                "allternit-tts not found next to allternit-voice-service (set ALLTERNIT_TTS_BIN)",
            )?;
            *guard = Some(TtsChild::spawn(&bin, &dir, self.threads)?);
        }
        Ok(guard.as_mut().expect("child just set"))
    }

    /// Synthesise `text` in one go. Returns (samples in -1..1, sample rate).
    /// Blocking.
    pub fn synthesize(
        &self,
        text: &str,
        voice: Option<&str>,
        speed: Option<f32>,
    ) -> Result<(Vec<f32>, u32), String> {
        let mut all = Vec::new();
        let mut rate = KOKORO_SAMPLE_RATE;
        self.synthesize_stream(text, voice, speed, |_, _, samples, r| {
            all.extend_from_slice(samples);
            rate = r;
            true
        })?;
        Ok((all, rate))
    }

    /// Streaming synthesis. The text is split for streaming (sentences, and
    /// the first sentence cut at its first clause, see
    /// [`split_for_streaming`]); `on_chunk(index, chunk_text, samples,
    /// sample_rate)` is called for every piece of audio as soon as the
    /// child renders it. Return `false` to stop after the current chunk
    /// (e.g. barge-in). Blocking.
    pub fn synthesize_stream<F>(
        &self,
        text: &str,
        voice: Option<&str>,
        speed: Option<f32>,
        mut on_chunk: F,
    ) -> Result<(), String>
    where
        F: FnMut(usize, &str, &[f32], u32) -> bool,
    {
        let sid = voice_sid(resolve_voice(voice)?);
        let speed = speed.unwrap_or(1.0).clamp(0.5, 2.0);
        let mut index = 0usize;
        let mut keep_going = true;
        for chunk in split_for_streaming(text) {
            if !keep_going {
                break;
            }
            let voice_id = resolve_voice(voice)?.id;
            if let Some(hit) = self.cache.as_ref().and_then(|c| c.get(voice_id, speed, &chunk)) {
                keep_going = on_chunk(index, &chunk, &hit.samples, hit.sample_rate);
                index += 1;
                continue;
            }
            let store = self.cache.as_ref().filter(|c| c.wants(&chunk));
            let mut rendered: Vec<f32> = Vec::new();
            // One chunk at a time under the service-wide inference lock, so
            // TTS and STT never run their models at the same moment.
            let _infer = inference_lock();
            let mut guard = self.lock_child()?;
            let mut attempt = 0;
            loop {
                attempt += 1;
                let child = self.ensure_child(&mut guard)?;
                let rate = child.sample_rate;
                let mut streamed = false;
                rendered.clear();
                let mut on_audio = |samples: &[f32]| {
                    if store.is_some() {
                        rendered.extend_from_slice(samples);
                    }
                    if keep_going {
                        keep_going = on_chunk(index, &chunk, samples, rate);
                    }
                    index += 1;
                };
                match child.request(&chunk, sid, speed, &mut streamed, &mut on_audio) {
                    Ok(()) => {
                        // Only a chunk that played to the end is stored.
                        if let (Some(c), true) = (store, keep_going) {
                            c.put(voice_id, speed, &chunk, &rendered, rate);
                        }
                        break;
                    }
                    Err(ChildError::Rejected(e)) => return Err(e),
                    Err(ChildError::Dead(e)) => {
                        warn!("allternit-tts died: {e}; restarting");
                        *guard = None; // drop = kill + reap
                        if streamed || attempt >= 2 {
                            return Err(e);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Most words in the first streamed chunk. Kokoro's cost grows with the
/// chunk, so the first audio comes from a very short one.
const FIRST_CHUNK_MAX_WORDS: usize = 4;
/// Fewest words in a first chunk cut from a longer sentence.
const FIRST_CHUNK_MIN_WORDS: usize = 2;

/// Words a phrase naturally starts with; cutting just before one keeps the
/// prosody of both halves natural.
const BREAK_BEFORE: &[&str] = &[
    "and", "but", "or", "so", "because", "that", "which", "who", "when", "while", "if", "to",
    "for", "of", "in", "on", "at", "by", "with", "from", "over", "under", "after", "before",
    "about", "into", "the", "a", "an", "your", "this", "my",
];

/// Split text for streaming TTS: sentences (see [`split_sentences`]), with
/// the first sentence cut short so first audio arrives early. The first
/// chunk is: up to the first clause mark (`,` `;` `:` or a dash, at least 2
/// words in, within 4 words); otherwise, for a first sentence of more than 4
/// words, 2-4 words, cut just before a phrase-starting word where possible
/// (else at 3). Sentences of up to 4 words are not cut.
pub fn split_for_streaming(text: &str) -> Vec<String> {
    let mut sentences = split_sentences(text);
    if sentences.is_empty() {
        return sentences;
    }
    let first = sentences.remove(0);
    let words: Vec<&str> = first.split_whitespace().collect();
    let n = words.len();
    let mut cut = None;
    for (i, w) in words.iter().enumerate().take(FIRST_CHUNK_MAX_WORDS) {
        let clause_end = w.ends_with([',', ';', ':']) || w.ends_with('—') || w.ends_with('–');
        let dash = matches!(*w, "-" | "—" | "–");
        if i + 1 >= FIRST_CHUNK_MIN_WORDS && i + 1 < n && (clause_end || dash) {
            cut = Some(i + 1);
            break;
        }
    }
    if cut.is_none() && n > FIRST_CHUNK_MAX_WORDS {
        let hi = FIRST_CHUNK_MAX_WORDS.min(n - 1);
        cut = (FIRST_CHUNK_MIN_WORDS..=hi).min_by_key(|&k| {
            let next = words[k].trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            let bonus = if BREAK_BEFORE.contains(&next.as_str()) { 2 } else { 0 };
            ((k as i64 - 3).abs() - bonus, k)
        });
    }
    let mut out = match cut {
        Some(k) => vec![words[..k].join(" "), words[k..].join(" ")],
        None => vec![first],
    };
    out.extend(sentences);
    out
}

/// Common abbreviations whose trailing period must not split a sentence.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "mx", "dr", "prof", "st", "sr", "jr", "vs", "etc", "eg", "ie", "approx",
    "no", "vol", "dept", "est", "fig", "capt", "lt", "col", "gen", "sen", "rep", "gov", "pres",
    "messrs", "mlle", "mme", "mst",
];

/// Split text into sentences for streaming TTS.
///
/// Splits after sentence-ending punctuation (`.`, `!`, `?`, `…`, newlines)
/// when followed by whitespace/end. Periods inside decimals ("3.14"),
/// initialisms ("U.S."), and common abbreviations ("Dr.", "e.g.") do not
/// split. This is a heuristic — Kokoro pronounces joined punctuation fine,
/// so occasional over-joining is harmless.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        current.push(c);
        if c == '!' || c == '?' || c == '…' || c == '\n' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        } else if c == '.' {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();

            // decimal / version numbers: '.' between digits never splits.
            let between_digits = prev.is_some_and(|p| p.is_ascii_digit())
                && next.is_some_and(|n| n.is_ascii_digit());

            // initialisms and nested abbreviations: "U.S.", "e.g.", "i.e."
            // (a period directly preceded by another period).
            let initialism =
                i >= 2 && chars[i - 2] == '.' && prev.is_some_and(|p| p.is_ascii_alphabetic());

            // abbreviation: alphabetic token before the period.
            let mut token_start = i;
            while token_start > 0 && chars[token_start - 1].is_ascii_alphabetic() {
                token_start -= 1;
            }
            let token: String = chars[token_start..i]
                .iter()
                .collect::<String>()
                .to_lowercase();
            let abbreviation = token.len() <= 6 && ABBREVIATIONS.contains(&token.as_str());

            let boundary = match next {
                None => true,
                Some(n) => n.is_whitespace(),
            };
            if boundary && !between_digits && !initialism && !abbreviation {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    sentences.push(trimmed.to_string());
                }
                current.clear();
            }
        }
        i += 1;
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        sentences.push(trimmed.to_string());
    }
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_are_the_28_english_kokoro_v1_voices() {
        assert_eq!(VOICES.len(), 28);
        assert_eq!(voice_sid(resolve_voice(None).unwrap()), 3); // af_heart
        assert_eq!(VOICES[27].id, "bm_lewis");
        let mut ids: Vec<_> = VOICES.iter().map(|v| v.id).collect();
        ids.sort();
        assert_eq!(ids, VOICES.iter().map(|v| v.id).collect::<Vec<_>>(), "sid order is alphabetical");
    }

    #[test]
    fn resolve_voice_accepts_legacy_and_rejects_unknown() {
        assert_eq!(resolve_voice(None).unwrap().id, "af_heart");
        assert_eq!(resolve_voice(Some("default")).unwrap().id, "af_heart");
        assert_eq!(resolve_voice(Some("af")).unwrap().id, "af_heart");
        assert_eq!(resolve_voice(Some("en-us-female")).unwrap().id, "af_heart");
        assert_eq!(resolve_voice(Some("en-us-male")).unwrap().id, "am_adam");
        assert_eq!(resolve_voice(Some("bm_george")).unwrap().id, "bm_george");
        assert!(resolve_voice(Some("x")).is_err());
    }

    #[test]
    fn first_chunk_cut_at_first_clause() {
        assert_eq!(
            split_for_streaming("Thank you for calling, how can I help you today? Bye."),
            vec!["Thank you for calling,", "how can I help you today?", "Bye."]
        );
    }

    #[test]
    fn long_first_sentence_cut_near_middle_before_phrase_word() {
        assert_eq!(
            split_for_streaming("The quick brown fox jumps over the lazy dog."),
            vec!["The quick brown fox", "jumps over the lazy dog."]
        );
        let s = split_for_streaming(
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen.",
        );
        assert!(s[0].split_whitespace().count() <= 4 && s[0].split_whitespace().count() >= 2);
        assert_eq!(s.join(" ").split_whitespace().count(), 14);
    }

    #[test]
    fn short_first_sentence_and_one_word_clause_not_cut() {
        assert_eq!(split_for_streaming("Hello world."), vec!["Hello world."]);
        assert_eq!(
            split_for_streaming("Well, I think so."),
            vec!["Well, I think so."]
        );
        assert!(split_for_streaming("  ").is_empty());
    }


    #[test]
    fn split_basic_sentences() {
        assert_eq!(
            split_sentences("Hello world. How are you? I am fine!"),
            vec!["Hello world.", "How are you?", "I am fine!"]
        );
    }

    #[test]
    fn split_keeps_abbreviations_and_decimals() {
        let s = split_sentences("Dr. Smith waited. It cost 3.14 dollars.");
        assert_eq!(s, vec!["Dr. Smith waited.", "It cost 3.14 dollars."]);
        let s = split_sentences("Use e.g. apples or i.e. fruit.");
        assert_eq!(s, vec!["Use e.g. apples or i.e. fruit."]);
    }

    #[test]
    fn split_keeps_initials() {
        let s = split_sentences("U.S. troops left. Then silence.");
        assert_eq!(s, vec!["U.S. troops left.", "Then silence."]);
    }

    #[test]
    fn split_multiline_and_empty() {
        assert_eq!(
            split_sentences("Line one\nLine two\n\n"),
            vec!["Line one", "Line two"]
        );
        assert!(split_sentences("   ").is_empty());
    }

    #[test]
    fn split_trailing_without_punctuation() {
        assert_eq!(
            split_sentences("One. Two without end"),
            vec!["One.", "Two without end"]
        );
    }
}
