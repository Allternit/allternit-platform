//! Disk cache of pre-rendered speech for fixed phrases.
//!
//! Kokoro takes ~0.3-1.5 s to render even a two-word reply on a weak CPU, so
//! the lines that are known ahead of time ("Sure.", "One moment.", a call's
//! disclosure) are rendered once and replayed from disk in about a
//! millisecond.
//!
//! - Key: (voice id, speed, text). The text is whitespace-normalised, and the
//!   key also carries [`CACHE_VERSION`] so a model or format change never
//!   serves stale audio.
//! - Entry: one file `<sha256-prefix>.pcm` under `<model dir>/phrase-cache/`:
//!   the sample rate (u32 LE) then mono s16le PCM, the same precision the TTS
//!   child already sends.
//! - Size: LRU, capped at `ALLTERNIT_VOICE_PHRASE_CACHE_MB` (default 64,
//!   `0` turns the cache off). Entries over [`MAX_ENTRY_BYTES`] are not kept.
//! - Which text is stored: only phrases in [`FIXED_PHRASES`] or ones a caller
//!   registered with [`PhraseCache::register`] (a call's disclosure and
//!   greeting). Ordinary replies are never written to disk.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use sha2::{Digest, Sha256};
use tracing::{debug, warn};

/// Bump when the TTS model, voices or file format change.
pub const CACHE_VERSION: &str = "kokoro-v1.0-fp32/1";
const DEFAULT_CAP_MB: u64 = 64;
/// ~40 s of 24 kHz audio. Longer text is not a "phrase".
pub const MAX_ENTRY_BYTES: u64 = 2 * 1024 * 1024;
/// Registered (non-fixed) phrases remembered in memory.
const MAX_REGISTERED: usize = 512;

/// Acknowledgements the reply path often starts with.
pub const ACKNOWLEDGEMENTS: &[&str] = &[
    "Sure.",
    "Okay.",
    "Got it.",
    "Of course.",
    "Alright.",
    "Right.",
    "Yes.",
    "No problem.",
];

/// Short fillers for long waits (see `fillers` in VOICE_SESSION.md).
pub const FILLERS: &[&str] = &[
    "One moment.",
    "Let me check that.",
    "Just a second.",
    "Hang on a second.",
];

/// Everything stored without being registered first.
pub fn fixed_phrases() -> impl Iterator<Item = &'static str> {
    ACKNOWLEDGEMENTS.iter().chain(FILLERS.iter()).copied()
}

pub fn is_fixed(text: &str) -> bool {
    let t = normalize(text);
    fixed_phrases().any(|p| p == t)
}

/// Trim and collapse whitespace so cosmetic differences still match.
pub fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

struct Entry {
    bytes: u64,
    used: u64,
}

#[derive(Default)]
struct Index {
    entries: HashMap<String, Entry>,
    total: u64,
    tick: u64,
}

pub struct PhraseCache {
    dir: PathBuf,
    cap_bytes: u64,
    index: Mutex<Index>,
    registered: Mutex<HashSet<String>>,
}

impl PhraseCache {
    /// Cache under `<model_root>/phrase-cache`, sized from the environment.
    /// `None` when the cache is switched off (`ALLTERNIT_VOICE_PHRASE_CACHE_MB=0`).
    pub fn from_env(model_root: &Path) -> Option<Self> {
        let mb = std::env::var("ALLTERNIT_VOICE_PHRASE_CACHE_MB")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_CAP_MB);
        (mb > 0).then(|| Self::new(model_root.join("phrase-cache"), mb * 1024 * 1024))
    }

    pub fn new(dir: PathBuf, cap_bytes: u64) -> Self {
        let cache = Self {
            dir,
            cap_bytes,
            index: Mutex::new(Index::default()),
            registered: Mutex::new(HashSet::new()),
        };
        cache.scan();
        cache
    }

    /// Rebuild the index from disk; oldest mtime = least recently used.
    fn scan(&self) {
        let Ok(rd) = fs::read_dir(&self.dir) else { return };
        let mut found: Vec<(SystemTime, String, u64)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let key = name.strip_suffix(".pcm")?.to_string();
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, key, meta.len()))
            })
            .collect();
        found.sort();
        let mut idx = self.lock();
        for (_, key, bytes) in found {
            idx.tick += 1;
            let used = idx.tick;
            idx.total += bytes;
            idx.entries.insert(key, Entry { bytes, used });
        }
        drop(idx);
        self.evict();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(voice: &str, speed: f32, text: &str) -> String {
        let mut h = Sha256::new();
        h.update(CACHE_VERSION.as_bytes());
        h.update([0]);
        h.update(voice.as_bytes());
        h.update([0]);
        h.update(format!("{speed:.2}").as_bytes());
        h.update([0]);
        h.update(normalize(text).as_bytes());
        let d = h.finalize();
        d[..16].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.pcm"))
    }

    /// Remember that `text` should be stored once rendered.
    pub fn register(&self, text: &str) {
        let t = normalize(text);
        if t.is_empty() {
            return;
        }
        let mut r = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        if r.len() >= MAX_REGISTERED {
            r.clear();
        }
        r.insert(t);
    }

    /// Whether a rendered chunk of `text` should be written to the cache.
    pub fn wants(&self, text: &str) -> bool {
        let t = normalize(text);
        if t.is_empty() {
            return false;
        }
        fixed_phrases().any(|p| p == t)
            || self
                .registered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&t)
    }

    pub fn contains(&self, voice: &str, speed: f32, text: &str) -> bool {
        self.lock().entries.contains_key(&Self::key(voice, speed, text))
    }

    pub fn get(&self, voice: &str, speed: f32, text: &str) -> Option<CachedAudio> {
        let key = Self::key(voice, speed, text);
        {
            let mut idx = self.lock();
            idx.tick += 1;
            let tick = idx.tick;
            idx.entries.get_mut(&key)?.used = tick;
        }
        let path = self.path(&key);
        let bytes = match fs::read(&path) {
            Ok(b) if b.len() >= 4 && (b.len() - 4) % 2 == 0 => b,
            _ => {
                // Missing or damaged: forget it so it is re-rendered.
                self.forget(&key);
                let _ = fs::remove_file(&path);
                return None;
            }
        };
        // Best effort: keeps LRU order across restarts.
        if let Ok(f) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        let sample_rate = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let samples = bytes[4..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        Some(CachedAudio {
            samples,
            sample_rate,
        })
    }

    pub fn put(&self, voice: &str, speed: f32, text: &str, samples: &[f32], sample_rate: u32) {
        let bytes = 4 + samples.len() as u64 * 2;
        if samples.is_empty() || bytes > MAX_ENTRY_BYTES || bytes > self.cap_bytes {
            return;
        }
        let key = Self::key(voice, speed, text);
        let mut buf = Vec::with_capacity(bytes as usize);
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        for &s in samples {
            buf.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
        if let Err(e) = self.write_atomic(&key, &buf) {
            warn!("phrase cache: could not store {key}: {e}");
            return;
        }
        {
            let mut idx = self.lock();
            idx.tick += 1;
            let used = idx.tick;
            if let Some(old) = idx.entries.insert(key, Entry { bytes, used }) {
                idx.total -= old.bytes;
            }
            idx.total += bytes;
        }
        self.evict();
    }

    fn write_atomic(&self, key: &str, buf: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("{key}.{}.tmp", std::process::id()));
        let mut f = fs::File::create(&tmp)?;
        f.write_all(buf)?;
        f.sync_all().ok();
        drop(f);
        fs::rename(&tmp, self.path(key))
    }

    fn forget(&self, key: &str) {
        let mut idx = self.lock();
        if let Some(e) = idx.entries.remove(key) {
            idx.total -= e.bytes;
        }
    }

    fn evict(&self) {
        loop {
            let victim = {
                let idx = self.lock();
                if idx.total <= self.cap_bytes {
                    return;
                }
                idx.entries
                    .iter()
                    .min_by_key(|(_, e)| e.used)
                    .map(|(k, _)| k.clone())
            };
            let Some(key) = victim else { return };
            debug!("phrase cache: evicting {key}");
            self.forget(&key);
            let _ = fs::remove_file(self.path(&key));
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.lock().total
    }

    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| ((i % 100) as f32 / 100.0) - 0.5).collect()
    }

    #[test]
    fn round_trips_and_keys_on_voice_speed_text() {
        let dir = tempfile::tempdir().unwrap();
        let c = PhraseCache::new(dir.path().join("pc"), 1 << 20);
        assert!(c.get("af_heart", 1.0, "Sure.").is_none());
        c.put("af_heart", 1.0, "Sure.", &tone(2400), 24_000);
        let got = c.get("af_heart", 1.0, "  Sure.  ").unwrap();
        assert_eq!(got.sample_rate, 24_000);
        assert_eq!(got.samples.len(), 2400);
        assert!((got.samples[10] - tone(2400)[10]).abs() < 1e-3);
        assert!(c.get("am_adam", 1.0, "Sure.").is_none());
        assert!(c.get("af_heart", 1.2, "Sure.").is_none());
        assert!(c.get("af_heart", 1.0, "Sure").is_none());
    }

    #[test]
    fn survives_restart_and_evicts_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pc");
        // 3 entries of 4004 bytes fit in 13_000; the 4th evicts the oldest.
        let c = PhraseCache::new(p.clone(), 13_000);
        for t in ["a.", "b.", "c."] {
            c.put("v", 1.0, t, &tone(2000), 24_000);
        }
        assert_eq!(c.len(), 3);
        assert!(c.get("v", 1.0, "a.").is_some()); // refresh a; b is now oldest
        c.put("v", 1.0, "d.", &tone(2000), 24_000);
        assert_eq!(c.len(), 3);
        assert!(c.get("v", 1.0, "b.").is_none());
        assert!(c.total_bytes() <= 13_000);
        let again = PhraseCache::new(p, 13_000);
        assert_eq!(again.len(), 3);
        assert!(again.get("v", 1.0, "d.").is_some());
    }

    #[test]
    fn rejects_oversized_and_repairs_damaged_entries() {
        let dir = tempfile::tempdir().unwrap();
        let c = PhraseCache::new(dir.path().join("pc"), 1 << 24);
        c.put("v", 1.0, "long", &vec![0.1; (MAX_ENTRY_BYTES / 2) as usize], 24_000);
        assert!(c.is_empty());
        c.put("v", 1.0, "ok.", &tone(100), 24_000);
        let key = PhraseCache::key("v", 1.0, "ok.");
        fs::write(c.path(&key), [1u8, 2, 3]).unwrap();
        assert!(c.get("v", 1.0, "ok.").is_none());
        assert!(c.is_empty());
    }

    #[test]
    fn stores_only_fixed_or_registered_phrases() {
        let dir = tempfile::tempdir().unwrap();
        let c = PhraseCache::new(dir.path().join("pc"), 1 << 20);
        assert!(c.wants("One moment."));
        assert!(c.wants("  Sure. "));
        assert!(!c.wants("Your balance is $40."));
        c.register("Hi, you've reached Acme.");
        assert!(c.wants("Hi, you've   reached Acme."));
        assert!(is_fixed("Okay."));
        assert!(!is_fixed("Okay then."));
    }
}
