//! Text-to-speech: Kokoro-82M (int8 English v0.19) through sherpa-onnx.
//!
//! Voice names and their speaker ids (`sid`) are pinned from the upstream
//! kLegacy v0.19 release (`hexgrad/kLegacy`, folder `v0.19/voices/`): 11
//! voices, alphabetical order, matching `voices.bin` layout
//! (num_speakers × 511 × 256 float32).

use sherpa_onnx::{GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig};
use std::sync::Mutex;
use tracing::info;

use crate::models::{find_file, PackManager};

/// One installed Kokoro voice (sid == index in this array).
#[derive(Debug, Clone, Copy)]
pub struct VoiceDef {
    pub id: &'static str,
    pub name: &'static str,
    pub language: &'static str,
    pub gender: &'static str,
}

/// 24 kHz output, matching the Kokoro model metadata (`sample_rate`).
pub const KOKORO_SAMPLE_RATE: u32 = 24_000;

pub const VOICES: &[VoiceDef] = &[
    VoiceDef {
        id: "af",
        name: "Default (US Female)",
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
        id: "af_nicole",
        name: "Nicole",
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
        id: "am_michael",
        name: "Michael",
        language: "en-US",
        gender: "male",
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

pub const DEFAULT_VOICE: &str = "af";

/// Resolve a requested voice id ("default" and legacy stub ids map to `af`).
pub fn resolve_voice(requested: Option<&str>) -> Result<&'static VoiceDef, String> {
    let id = requested.map(str::trim).unwrap_or("");
    match id {
        "" | "default" | "en-us-female" | "en-us-male" => {
            return Ok(&VOICES[0]);
        }
        _ => {}
    }
    VOICES
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| format!("unknown voice '{id}' (see GET /v1/voices)"))
}

/// Map a voice id to its Kokoro speaker index (sid).
pub fn voice_sid(v: &VoiceDef) -> i32 {
    VOICES.iter().position(|x| x.id == v.id).unwrap_or(0) as i32
}

pub struct TtsEngine {
    pub manager: PackManager,
    threads: i32,
    inner: Mutex<Option<(OfflineTts, i32)>>,
}

impl TtsEngine {
    pub fn new(manager: PackManager) -> Self {
        let threads = std::env::var("ALLTERNIT_VOICE_THREADS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        Self {
            manager,
            threads,
            inner: Mutex::new(None),
        }
    }

    pub fn num_threads(&self) -> i32 {
        self.threads
    }

    /// Build (or reuse) the Kokoro engine. Blocking.
    pub fn prepare(&self) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| format!("TTS engine lock poisoned: {e}"))?;
        if guard.is_some() {
            return Ok(());
        }
        let dir = self
            .manager
            .ensure_blocking("small")
            .map_err(|e| format!("small pack: {e}"))?;
        let model = find_file(&dir, &["model.int8.onnx", "model.onnx"])
            .ok_or_else(|| "kokoro model.onnx missing".to_string())?;
        let voices = find_file(&dir, &["voices.bin"])
            .ok_or_else(|| "kokoro voices.bin missing".to_string())?;
        let tokens = find_file(&dir, &["tokens.txt"])
            .ok_or_else(|| "kokoro tokens.txt missing".to_string())?;
        let data_dir = find_file(&dir, &["espeak-ng-data"])
            .map(|p| p.display().to_string())
            .or_else(|| Some(dir.display().to_string()));

        let config = OfflineTtsConfig {
            model: sherpa_onnx::OfflineTtsModelConfig {
                kokoro: OfflineTtsKokoroModelConfig {
                    model: Some(model.display().to_string()),
                    voices: Some(voices.display().to_string()),
                    tokens: Some(tokens.display().to_string()),
                    data_dir,
                    lexicon: None,
                    lang: Some("en-us".to_string()),
                    length_scale: 1.0,
                    dict_dir: None,
                },
                num_threads: self.threads,
                provider: Some("cpu".to_string()),
                debug: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let tts = OfflineTts::create(&config).ok_or_else(|| "failed to create Kokoro TTS")?;
        let sample_rate = tts.sample_rate();
        info!(
            "TTS ready: kokoro-en-v0_19, {} speakers, {} Hz",
            tts.num_speakers(),
            sample_rate
        );
        *guard = Some((tts, sample_rate));
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Synthesise one text block. Returns (samples f32 -1..1, sample_rate).
    /// Blocking.
    pub fn synthesize(
        &self,
        text: &str,
        voice: Option<&str>,
        speed: Option<f32>,
    ) -> Result<(Vec<f32>, i32), String> {
        self.prepare()?;
        let guard = self
            .inner
            .lock()
            .map_err(|e| format!("TTS engine lock poisoned: {e}"))?;
        let (tts, sample_rate) = guard.as_ref().ok_or("TTS engine not initialised")?;
        let voice_def = resolve_voice(voice)?;
        let config = GenerationConfig {
            speed: speed.unwrap_or(1.0),
            sid: voice_sid(voice_def),
            ..Default::default()
        };
        let audio = tts
            .generate_with_config(text, &config, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| "Kokoro generation failed".to_string())?;
        Ok((audio.samples().to_vec(), *sample_rate))
    }
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
    fn voices_cover_11_with_default_first() {
        assert_eq!(VOICES.len(), 11);
        assert_eq!(VOICES[0].id, DEFAULT_VOICE);
        // sids must be the alphabetical positions matching voices.bin
        for (i, v) in VOICES.iter().enumerate() {
            assert_eq!(voice_sid(v), i as i32);
        }
    }

    #[test]
    fn resolve_voice_accepts_legacy_and_rejects_unknown() {
        assert_eq!(resolve_voice(None).unwrap().id, "af");
        assert_eq!(resolve_voice(Some("default")).unwrap().id, "af");
        assert_eq!(resolve_voice(Some("en-us-male")).unwrap().id, "af");
        assert_eq!(resolve_voice(Some("bm_george")).unwrap().id, "bm_george");
        assert!(resolve_voice(Some("x")).is_err());
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
