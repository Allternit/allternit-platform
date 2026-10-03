//! Custom voices with consent on file.
//!
//! - [`consent`]: the consent gate (cloud-api is the record of truth).
//! - [`pocket`]: Pocket TTS (voice cloning from a short reference clip).
//! - [`sentencepiece`]: the tokenizer Pocket TTS needs.
//!
//! A custom voice id is `custom:<uuid>`. [`CustomVoices`] ties the three
//! together: it refuses anything without a live consent grant, fetches the
//! clip, verifies its hash against the consent record, embeds it, and keeps
//! the embedding in memory only (never on disk), dropping it the moment
//! consent is gone.

pub mod consent;
pub mod pocket;
pub mod sentencepiece;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::audio;
use crate::models::{inference_threads, PackManager};
use crate::session::engine::EngineError;
use crate::session::protocol::codes;
use consent::{ConsentChecker, ConsentError, Grant};
use pocket::{PocketEngine, VoiceEmbedding, VoiceState, POCKET_SAMPLE_RATE};

pub const CUSTOM_PREFIX: &str = "custom:";
/// The model pack that holds the Pocket TTS files.
pub const VOICES_PACK: &str = "voices";
const EMBEDDING_CACHE: usize = 16;

pub fn is_custom(voice: &str) -> bool {
    voice.starts_with(CUSTOM_PREFIX)
}

fn refused(message: impl Into<String>) -> EngineError {
    EngineError::new(codes::CUSTOM_VOICE_REFUSED, message)
}

fn consent_error(e: ConsentError) -> EngineError {
    match e {
        ConsentError::NotOnFile => refused(e.to_string()),
        ConsentError::Unavailable(_) => EngineError::unavailable(e.to_string()),
    }
}

type CacheKey = (String, String);

/// Shared by every session of the process.
pub struct CustomVoices {
    checker: Option<Arc<dyn ConsentChecker>>,
    packs: Arc<PackManager>,
    engine: Mutex<Option<Arc<PocketEngine>>>,
    embeddings: Mutex<VecDeque<(CacheKey, Arc<VoiceEmbedding>)>>,
}

impl CustomVoices {
    /// `checker = None` means this service cannot verify consent (a Desktop
    /// sidecar, a misconfigured server): every custom voice is refused.
    pub fn new(packs: Arc<PackManager>, checker: Option<Arc<dyn ConsentChecker>>) -> Arc<Self> {
        Arc::new(Self {
            checker,
            packs,
            engine: Mutex::new(None),
            embeddings: Mutex::new(VecDeque::new()),
        })
    }

    /// Is there a live consent record for `owner`'s `voice`? Evicts the
    /// voice's cached embeddings when there is not.
    pub fn authorize(&self, owner: Option<&str>, voice: &str) -> Result<Grant, EngineError> {
        let Some(checker) = &self.checker else {
            return Err(refused(
                "custom voices need Allternit Cloud Voice (this voice service cannot verify consent)",
            ));
        };
        let Some(owner) = owner.filter(|o| !o.is_empty()) else {
            return Err(refused("custom voices need a signed-in Cloud Voice session"));
        };
        let id = voice.strip_prefix(CUSTOM_PREFIX).unwrap_or(voice);
        match checker.grant(owner, id) {
            Ok(g) => Ok(g),
            Err(e) => {
                if e == ConsentError::NotOnFile {
                    self.purge(id);
                }
                Err(consent_error(e))
            }
        }
    }

    /// Drop everything cached for a voice.
    pub fn purge(&self, voice_id: &str) {
        self.embeddings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|((id, _), _)| id != voice_id);
    }

    fn engine(&self) -> Result<Arc<PocketEngine>, EngineError> {
        let mut slot = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = &*slot {
            return Ok(e.clone());
        }
        let dir = self
            .packs
            .ensure_blocking(VOICES_PACK)
            .map_err(|e| EngineError::unavailable(format!("voices pack: {e}")))?;
        let engine = Arc::new(PocketEngine::load(&dir, inference_threads() as usize)?);
        *slot = Some(engine.clone());
        Ok(engine)
    }

    /// Authorize, then get the voice ready to speak (embedding cached by
    /// voice id + clip hash, memory only).
    pub fn open(&self, owner: Option<&str>, voice: &str) -> Result<OpenVoice, EngineError> {
        let grant = self.authorize(owner, voice)?;
        let id = voice.strip_prefix(CUSTOM_PREFIX).unwrap_or(voice).to_string();
        let owner = owner.unwrap_or_default();
        let engine = self.engine()?;
        let key = (id.clone(), grant.clip_sha256.clone());
        let cached = {
            let c = self.embeddings.lock().unwrap_or_else(|e| e.into_inner());
            c.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
        };
        let emb = match cached {
            Some(e) => e,
            None => {
                let checker = self.checker.as_ref().expect("authorize passed");
                let wav = checker.clip(owner, &id).map_err(consent_error)?;
                if crate::models::sha256_bytes(&wav) != grant.clip_sha256 {
                    return Err(refused("reference clip does not match its consent record"));
                }
                let (pcm, rate) = audio::decode_wav(&wav).map_err(|e| refused(format!("reference clip: {e}")))?;
                let pcm = audio::resample(&pcm, rate, POCKET_SAMPLE_RATE);
                let emb = Arc::new(engine.embed_clip(&pcm)?);
                let mut c = self.embeddings.lock().unwrap_or_else(|e| e.into_inner());
                c.retain(|(k, _)| k.0 != id); // a re-recorded clip replaces the old one
                c.push_back((key, emb.clone()));
                while c.len() > EMBEDDING_CACHE {
                    c.pop_front();
                }
                emb
            }
        };
        let state = engine.condition(&emb)?;
        Ok(OpenVoice { id, clip_sha256: grant.clip_sha256, engine, state })
    }

    #[cfg(test)]
    pub fn cache_len(&self) -> usize {
        self.embeddings.lock().unwrap().len()
    }

    #[cfg(test)]
    pub fn cache_insert(&self, voice_id: &str, sha: &str) {
        self.embeddings.lock().unwrap().push_back((
            (voice_id.into(), sha.into()),
            Arc::new(pocket::VoiceEmbedding::for_test()),
        ));
    }
}

/// A custom voice that passed consent and is conditioned for a session.
pub struct OpenVoice {
    pub id: String,
    pub clip_sha256: String,
    engine: Arc<PocketEngine>,
    state: VoiceState,
}

impl OpenVoice {
    pub fn synthesize(&self, text: &str, sink: &mut dyn FnMut(&[f32]) -> bool) -> Result<(), EngineError> {
        let _one_model_at_a_time = crate::models::inference_lock();
        self.engine.synthesize(&self.state, text, sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Mock {
        on_file: AtomicBool,
        calls: AtomicUsize,
    }
    impl ConsentChecker for Mock {
        fn grant(&self, owner: &str, voice_id: &str) -> Result<Grant, ConsentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if owner == "alice" && voice_id == "v1" && self.on_file.load(Ordering::SeqCst) {
                Ok(Grant { clip_sha256: "abc".into(), name: "Alice".into() })
            } else {
                Err(ConsentError::NotOnFile)
            }
        }
        fn clip(&self, _: &str, _: &str) -> Result<Vec<u8>, ConsentError> {
            Err(ConsentError::NotOnFile)
        }
    }

    fn voices(checker: Option<Arc<dyn ConsentChecker>>) -> Arc<CustomVoices> {
        CustomVoices::new(Arc::new(PackManager::with_root(std::env::temp_dir().join("cv-test-packs"))), checker)
    }

    #[test]
    fn without_a_checker_every_custom_voice_is_refused() {
        let v = voices(None);
        let e = v.authorize(Some("alice"), "custom:v1").unwrap_err();
        assert_eq!(e.code, codes::CUSTOM_VOICE_REFUSED);
        assert!(e.message.contains("Cloud Voice"));
    }

    #[test]
    fn without_an_owner_it_is_refused_before_cloud_is_asked() {
        let m = Arc::new(Mock { on_file: AtomicBool::new(true), calls: AtomicUsize::new(0) });
        let v = voices(Some(m.clone()));
        assert_eq!(v.authorize(None, "custom:v1").unwrap_err().code, codes::CUSTOM_VOICE_REFUSED);
        assert_eq!(v.authorize(Some(""), "custom:v1").unwrap_err().code, codes::CUSTOM_VOICE_REFUSED);
        assert_eq!(m.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn someone_elses_voice_or_no_record_is_refused() {
        let m = Arc::new(Mock { on_file: AtomicBool::new(true), calls: AtomicUsize::new(0) });
        let v = voices(Some(m));
        assert!(v.authorize(Some("alice"), "custom:v1").is_ok());
        assert_eq!(v.authorize(Some("mallory"), "custom:v1").unwrap_err().code, codes::CUSTOM_VOICE_REFUSED);
        assert_eq!(v.authorize(Some("alice"), "custom:nope").unwrap_err().code, codes::CUSTOM_VOICE_REFUSED);
    }

    #[test]
    fn revoking_consent_stops_the_voice_and_drops_its_cache() {
        let m = Arc::new(Mock { on_file: AtomicBool::new(true), calls: AtomicUsize::new(0) });
        let v = voices(Some(m.clone()));
        v.cache_insert("v1", "abc");
        v.cache_insert("other", "zzz");
        assert!(v.authorize(Some("alice"), "custom:v1").is_ok());
        assert_eq!(v.cache_len(), 2);
        m.on_file.store(false, Ordering::SeqCst); // the owner revokes
        let e = v.authorize(Some("alice"), "custom:v1").unwrap_err();
        assert_eq!(e.code, codes::CUSTOM_VOICE_REFUSED);
        assert_eq!(v.cache_len(), 1, "only the revoked voice is evicted");
    }

    #[test]
    fn an_unreachable_consent_system_fails_closed_without_evicting() {
        struct Down;
        impl ConsentChecker for Down {
            fn grant(&self, _: &str, _: &str) -> Result<Grant, ConsentError> {
                Err(ConsentError::Unavailable("unreachable".into()))
            }
            fn clip(&self, _: &str, _: &str) -> Result<Vec<u8>, ConsentError> {
                Err(ConsentError::Unavailable("unreachable".into()))
            }
        }
        let v = voices(Some(Arc::new(Down)));
        v.cache_insert("v1", "abc");
        let e = v.authorize(Some("alice"), "custom:v1").unwrap_err();
        assert_eq!(e.code, codes::ENGINE_UNAVAILABLE);
        assert_eq!(v.cache_len(), 1);
    }
}
