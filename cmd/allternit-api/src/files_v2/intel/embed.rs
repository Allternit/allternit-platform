//! Embedding providers for meaning search.
//!
//! The runtime already has one local embedding path: `memory_index::global()`
//! calls an OpenAI-compatible endpoint (`ALLTERNIT_EMBED_URL`, default the
//! loopback sidecar on port 7719). Files reuses it as is. Its deterministic
//! hash fallback is NOT used here: it carries no meaning, so a meaning search
//! on it would return noise. With no model reachable the provider yields
//! nothing, no vectors are stored, and `semantic` search answers `unsupported`
//! while full text keeps working.

use crate::memory_index::{Embedded, InputType};

pub trait Embedder: Send + Sync {
    /// One vector per text, all from one model, or None when no model is
    /// available. `query` is true for the search text, false for documents.
    fn embed(&self, texts: &[String], query: bool) -> Option<Embedded>;
}

/// Meaning search off.
pub struct NullEmbedder;

impl Embedder for NullEmbedder {
    fn embed(&self, _texts: &[String], _query: bool) -> Option<Embedded> {
        None
    }
}

/// The runtime's local embedding endpoint. Called from worker or blocking
/// threads, never from an async task.
pub struct LocalEndpointEmbedder {
    handle: tokio::runtime::Handle,
}

impl LocalEndpointEmbedder {
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        Self { handle }
    }
}

impl Embedder for LocalEndpointEmbedder {
    fn embed(&self, texts: &[String], query: bool) -> Option<Embedded> {
        let kind = if query { InputType::Query } else { InputType::Document };
        let client = crate::memory_index::global();
        if !client.is_enabled() {
            return None;
        }
        self.handle.block_on(client.embed_remote(texts, kind)).ok()
    }
}

pub(crate) fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}
