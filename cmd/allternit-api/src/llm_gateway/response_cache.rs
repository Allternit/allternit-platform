//! Response cache for the chat-completions hot path (P1.6).
//!
//! The gateway already has prompt/context cache *APIs* (`cache.rs`,
//! `context_cache.rs`) but nothing that short-circuits an identical request
//! before the upstream call. This module is that short-circuit: an in-memory
//! cache of completed non-streaming `chat.completion` bodies, keyed by a
//! SHA-256 hash of the output-affecting request fields.
//!
//! O7 (2026-10-01): this store is now the ONE shared exact-cache layer. The
//! gateway uses it (call type `gateway.chat`), and so do internal
//! `gizzi_completion` calls (`crate::completion_cache`), with a TTL per entry
//! (per call type). Streams are cached on completion as the equivalent
//! `chat.completion` body and replayed as a synthesized SSE stream
//! ([`replay_sse_frames`]), so one entry serves both wire shapes.
//!
//! Gateway eligibility (enforced by [`is_cacheable`] + [`is_deterministic`],
//! checked in `proxy::chat_completions`):
//! - streaming and non-streaming both (streams are written on completion);
//! - no `tools` / `tool_choice` — tool requests are never cached per spec;
//! - `n` absent or 1 and `best_of` not set — multi-sample requests are
//!   deliberately excluded;
//! - deterministic only: `temperature: 0`, unless the caller opts in with
//!   the `x-allternit-cache: on` request header.
//!
//! Keys are namespaced per tenant (or user when there is no tenant), so a
//! cached body is never served across accounts.
//!
//! Configuration (opt-in, disabled by default):
//! - `LLM_RESPONSE_CACHE_TTL_SECS` — entry time-to-live in seconds.
//!   `0` (the default) disables the cache entirely.
//! - `LLM_RESPONSE_CACHE_MAX_ENTRIES` — capacity cap (default 1024). On
//!   insert at capacity, expired entries are purged first, then the oldest
//!   entry is evicted.
//!
//! Hits/misses are counted by `crate::metrics` (`llm_response_cache_hits_total`
//! / `llm_response_cache_misses_total`) at the decision points in proxy.rs.
//! Cached hits are recorded as zero-cost usage rows by the proxy handler (no
//! upstream spend, same treatment as BYOK-served requests).

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use super::translate::ChatCompletionRequest;

/// Default capacity when `LLM_RESPONSE_CACHE_MAX_ENTRIES` is unset/unparseable.
pub const DEFAULT_MAX_ENTRIES: usize = 1024;

#[derive(Debug, Clone, Copy)]
pub struct ResponseCacheConfig {
    pub ttl_secs: u64,
    pub max_entries: usize,
}

impl ResponseCacheConfig {
    /// The cache is opt-in: a zero TTL means disabled.
    pub fn enabled(&self) -> bool {
        self.ttl_secs > 0 && self.max_entries > 0
    }

    pub fn from_env() -> Self {
        Self::parse(
            std::env::var("LLM_RESPONSE_CACHE_TTL_SECS").ok(),
            std::env::var("LLM_RESPONSE_CACHE_MAX_ENTRIES").ok(),
        )
    }

    fn parse(ttl: Option<String>, max_entries: Option<String>) -> Self {
        Self {
            ttl_secs: ttl.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
            max_entries: max_entries
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(DEFAULT_MAX_ENTRIES),
        }
    }
}

struct CacheEntry {
    body: Value,
    stored_at: Instant,
    ttl: Duration,
}

/// In-memory TTL cache. Hand-rolled rather than `moka` to avoid a new
/// dependency — the eviction policy (purge expired, then oldest) is trivial.
pub struct ResponseCache {
    config: ResponseCacheConfig,
    entries: RwLock<HashMap<String, CacheEntry>>,
}

impl ResponseCache {
    pub fn new(config: ResponseCacheConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Whether the gateway path is enabled (its env TTL is > 0).
    pub fn enabled(&self) -> bool {
        self.config.enabled()
    }

    /// The gateway's env TTL.
    pub fn gateway_ttl(&self) -> Duration {
        Duration::from_secs(self.config.ttl_secs)
    }

    /// Whether the store accepts entries at all (internal call types bring
    /// their own TTL, so they only need capacity).
    pub fn store_enabled(&self) -> bool {
        self.config.max_entries > 0
    }

    /// Store with an explicit TTL (internal call types). No-op for a zero TTL.
    pub fn put_with_ttl(&self, key: String, body: Value, ttl: Duration) {
        self.put_entry(key, body, ttl, Instant::now());
    }

    /// Fetch regardless of the gateway's env switch (each entry carries its
    /// own TTL).
    pub fn get_any(&self, key: &str) -> Option<Value> {
        self.lookup(key, Instant::now())
    }

    /// Process-wide cache, configured from env on first use.
    pub fn global() -> &'static ResponseCache {
        static GLOBAL: Lazy<ResponseCache> =
            Lazy::new(|| ResponseCache::new(ResponseCacheConfig::from_env()));
        &GLOBAL
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.get_at(key, Instant::now())
    }

    pub fn put(&self, key: String, body: Value) {
        self.put_at(key, body, Instant::now());
    }

    /// Fetch with an injectable clock (`get` is the production wrapper).
    /// Returns `None` when the gateway path is disabled, missing, or expired.
    fn get_at(&self, key: &str, now: Instant) -> Option<Value> {
        if !self.config.enabled() {
            return None;
        }
        self.lookup(key, now)
    }

    /// Expired entries are purged on read.
    fn lookup(&self, key: &str, now: Instant) -> Option<Value> {
        if !self.store_enabled() {
            return None;
        }
        {
            let entries = self.entries.read().ok()?;
            match entries.get(key) {
                Some(entry) if now.duration_since(entry.stored_at) < entry.ttl => {
                    return Some(entry.body.clone());
                }
                _ => {}
            }
        }
        // Missing or expired — drop the stale entry if present.
        if let Ok(mut entries) = self.entries.write() {
            let expired = entries
                .get(key)
                .is_some_and(|e| now.duration_since(e.stored_at) >= e.ttl);
            if expired {
                entries.remove(key);
            }
        }
        None
    }

    /// Store with an injectable clock (`put` is the production wrapper).
    /// No-op when the gateway path is disabled.
    fn put_at(&self, key: String, body: Value, now: Instant) {
        if !self.config.enabled() {
            return;
        }
        self.put_entry(key, body, Duration::from_secs(self.config.ttl_secs), now);
    }

    fn put_entry(&self, key: String, body: Value, ttl: Duration, now: Instant) {
        if !self.store_enabled() || ttl.is_zero() {
            return;
        }
        let Ok(mut entries) = self.entries.write() else {
            return;
        };
        if entries.len() >= self.config.max_entries && !entries.contains_key(&key) {
            // Purge expired first; if still full, evict the oldest entry.
            entries.retain(|_, e| now.duration_since(e.stored_at) < e.ttl);
            if entries.len() >= self.config.max_entries {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, e)| e.stored_at)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
        }
        entries.insert(key, CacheEntry { body, stored_at: now, ttl });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }
}

/// Request header that opts a non-deterministic gateway request into the
/// exact cache (`on`), or forces a bypass (`off`).
pub const CACHE_OPT_HEADER: &str = "x-allternit-cache";

/// Whether a request is shape-eligible for the response cache (tool-free,
/// single sample). Streaming is allowed: streams are cached on completion.
pub fn is_cacheable(request: &ChatCompletionRequest) -> bool {
    if request.tools.as_ref().is_some_and(|t| !t.is_empty()) || request.tool_choice.is_some() {
        return false;
    }
    if request.n.unwrap_or(1) > 1 || request.best_of.unwrap_or(false) {
        return false;
    }
    true
}

/// Deterministic sampling: only `temperature: 0` is cacheable by default
/// (O7). `opt_in` = the caller sent `x-allternit-cache: on`.
pub fn is_deterministic(request: &ChatCompletionRequest, opt_in: bool) -> bool {
    opt_in || request.temperature == Some(0.0)
}

/// Gateway cacheability from the request + its opt header value.
pub fn gateway_cacheable(request: &ChatCompletionRequest, opt_header: Option<&str>) -> bool {
    let opt = opt_header.map(|v| v.trim().to_ascii_lowercase());
    if opt.as_deref() == Some("off") {
        return false;
    }
    is_cacheable(request) && is_deterministic(request, opt.as_deref() == Some("on"))
}

/// Gateway key: [`cache_key`] namespaced by tenant (or user when the key has
/// no tenant), so cached bodies never cross accounts.
pub fn gateway_cache_key(request: &ChatCompletionRequest, tenant_or_user: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gateway.chat\0");
    hasher.update(tenant_or_user.as_bytes());
    hasher.update(b"\0");
    hasher.update(cache_key(request).as_bytes());
    hex::encode(hasher.finalize())
}

/// The `chat.completion` body equivalent to a finished stream, so a stream
/// writes the same entry a non-streaming call would.
pub fn completed_stream_body(
    id: &str,
    created: i64,
    model: &str,
    text: &str,
    finish_reason: &str,
    usage: super::translate::Usage,
) -> Value {
    serde_json::to_value(super::translate::ChatCompletionResponse::new(
        id.to_string(),
        created,
        model.to_string(),
        text.to_string(),
        finish_reason.to_string(),
        Some(usage),
    ))
    .unwrap_or(Value::Null)
}

/// Replay a cached `chat.completion` body as SSE `data:` payloads in the
/// same order the live stream emits them: role chunk, one content chunk,
/// finish chunk, optional usage chunk, `[DONE]`. `None` if the body has no
/// assistant text (nothing safe to replay).
pub fn replay_sse_frames(
    body: &Value,
    id: &str,
    created: i64,
    model: &str,
    include_usage: bool,
) -> Option<Vec<String>> {
    use super::translate::{ChatCompletionChunk, Usage};
    let choice = body.get("choices")?.get(0)?;
    let content = choice.get("message")?.get("content")?.as_str()?;
    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let mut frames = vec![
        ChatCompletionChunk::role_chunk(id, created, model).to_sse_data(),
        ChatCompletionChunk::content_chunk(id, created, model, content).to_sse_data(),
        ChatCompletionChunk::finish_chunk(id, created, model, finish).to_sse_data(),
    ];
    if include_usage {
        let u = body.get("usage");
        let n = |k: &str| u.and_then(|u| u.get(k)).and_then(Value::as_i64).unwrap_or(0);
        let d = |g: &str, k: &str| {
            u.and_then(|u| u.get(g)).and_then(|g| g.get(k)).and_then(Value::as_i64).unwrap_or(0)
        };
        let usage = Usage::new(
            n("prompt_tokens"),
            n("completion_tokens"),
            d("completion_tokens_details", "reasoning_tokens"),
            d("prompt_tokens_details", "cached_tokens"),
        );
        frames.push(ChatCompletionChunk::usage_chunk(id, created, model, usage).to_sse_data());
    }
    frames.push("[DONE]".to_string());
    Some(frames)
}

/// SHA-256 hash of the output-affecting request fields (hex-encoded).
///
/// Normalization: fields are serialized through a canonical JSON object
/// (serde_json maps serialize with sorted keys, struct fields in declaration
/// order) after the proxy handler has applied file-reference resolution and
/// context-cache prepends, so two semantically identical requests hash equal
/// regardless of JSON whitespace or key order on the wire.
pub fn cache_key(request: &ChatCompletionRequest) -> String {
    let canonical = json!({
        "model": request.model.trim(),
        "messages": request.messages,
        "temperature": request.temperature,
        "top_p": request.top_p,
        "max_tokens": request.max_tokens,
        "stop": request.stop,
        "presence_penalty": request.presence_penalty,
        "frequency_penalty": request.frequency_penalty,
        "response_format": request.response_format,
        "reasoning_effort": request.reasoning_effort,
        "service_tier": request.service_tier,
        "citations": request.citations,
        "n": request.n,
    });
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::translate::ChatMessage;

    fn config(ttl_secs: u64) -> ResponseCacheConfig {
        ResponseCacheConfig {
            ttl_secs,
            max_entries: 4,
        }
    }

    fn request(body: &str) -> ChatCompletionRequest {
        serde_json::from_str(body).expect("test request parses")
    }

    fn simple_request() -> ChatCompletionRequest {
        request(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#)
    }

    // ── key hashing ──────────────────────────────────────────────────────

    #[test]
    fn key_is_stable_across_wire_formatting() {
        let a = request(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"temperature":0.5}"#);
        // Different key order + whitespace, same semantics.
        let b = request(
            r#"{ "temperature": 0.5, "messages": [ { "content": "hi", "role": "user" } ],
                 "model": "gpt-4o" }"#,
        );
        assert_eq!(cache_key(&a), cache_key(&b));
    }

    #[test]
    fn key_changes_with_output_affecting_fields() {
        let base = simple_request();
        let other_model = request(r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#);
        let other_temp = request(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"temperature":0.7}"#);
        let other_msg = request(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"bye"}]}"#);
        assert_ne!(cache_key(&base), cache_key(&other_model));
        assert_ne!(cache_key(&base), cache_key(&other_temp));
        assert_ne!(cache_key(&base), cache_key(&other_msg));
    }

    #[test]
    fn key_ignores_unknown_wire_fields() {
        let a = simple_request();
        let b = request(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"some_future_field":42}"#);
        assert_eq!(cache_key(&a), cache_key(&b));
    }

    // ── eligibility ─────────────────────────────────────────────────────

    #[test]
    fn tool_requests_bypass_cache() {
        let with_tools = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"f"}}]}"#);
        let with_choice = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"tool_choice":"auto"}"#);
        assert!(!is_cacheable(&with_tools));
        assert!(!is_cacheable(&with_choice));
    }

    #[test]
    fn streaming_requests_are_cacheable_and_share_the_key() {
        let streaming = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true,"temperature":0}"#);
        let plain = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"temperature":0}"#);
        assert!(is_cacheable(&streaming));
        assert_eq!(cache_key(&streaming), cache_key(&plain));
    }

    #[test]
    fn gateway_requires_determinism_unless_opted_in() {
        let t0 = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"temperature":0}"#);
        let warm = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"temperature":0.7}"#);
        let unset = simple_request();
        assert!(gateway_cacheable(&t0, None));
        assert!(!gateway_cacheable(&warm, None));
        assert!(!gateway_cacheable(&unset, None));
        assert!(gateway_cacheable(&warm, Some("on")));
        assert!(!gateway_cacheable(&t0, Some("OFF")));
        let tools = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"temperature":0,"tools":[{"type":"function","function":{"name":"f"}}]}"#);
        assert!(!gateway_cacheable(&tools, Some("on")), "opt-in never overrides the tool exclusion");
    }

    #[test]
    fn gateway_key_is_namespaced_per_tenant() {
        let r = simple_request();
        assert_ne!(gateway_cache_key(&r, "tenant-a"), gateway_cache_key(&r, "tenant-b"));
        assert_eq!(gateway_cache_key(&r, "tenant-a"), gateway_cache_key(&r, "tenant-a"));
    }

    #[test]
    fn per_entry_ttl_is_independent_of_gateway_switch() {
        // Gateway path disabled (ttl 0), internal entries still work.
        let cache = ResponseCache::new(config(0));
        let t0 = Instant::now();
        cache.put_entry("k".into(), json!({"v": 1}), Duration::from_secs(5), t0);
        assert!(cache.lookup("k", t0 + Duration::from_secs(4)).is_some());
        assert!(cache.lookup("k", t0 + Duration::from_secs(6)).is_none());
        // The gateway accessor still refuses while its switch is off.
        cache.put_entry("g".into(), json!({"v": 2}), Duration::from_secs(5), t0);
        assert!(cache.get_at("g", t0).is_none());
        // Zero TTL never stores.
        cache.put_entry("z".into(), json!({}), Duration::ZERO, t0);
        assert!(cache.lookup("z", t0).is_none());
    }

    #[test]
    fn completed_stream_replays_as_equivalent_stream() {
        let usage = crate::llm_gateway::translate::Usage::new(11, 7, 0, 0);
        let body = completed_stream_body("chatcmpl-x", 1, "m", "hello world", "stop", usage);
        assert_eq!(body["choices"][0]["message"]["content"], "hello world");
        let cache = ResponseCache::new(config(60));
        cache.put("k".into(), body);
        let hit = cache.get("k").unwrap();
        let frames = replay_sse_frames(&hit, "chatcmpl-y", 2, "m", true).unwrap();
        assert_eq!(frames.last().unwrap(), "[DONE]");
        let chunks: Vec<Value> = frames[..frames.len() - 1]
            .iter()
            .map(|f| serde_json::from_str(f).unwrap())
            .collect();
        assert!(chunks.iter().all(|c| c["object"] == "chat.completion.chunk" && c["id"] == "chatcmpl-y"));
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
        let text: String = chunks
            .iter()
            .filter_map(|c| c["choices"].get(0)?["delta"]["content"].as_str().map(str::to_string))
            .collect();
        assert_eq!(text, "hello world");
        assert_eq!(chunks[2]["choices"][0]["finish_reason"], "stop");
        assert_eq!(chunks[3]["usage"]["completion_tokens"], 7);
        // Without include_usage there is no usage frame.
        let frames = replay_sse_frames(&hit, "i", 2, "m", false).unwrap();
        assert_eq!(frames.len(), 4);
        // Bodies without text are not replayed.
        assert!(replay_sse_frames(&json!({"choices": []}), "i", 0, "m", false).is_none());
    }

    // ── TTL / storage ───────────────────────────────────────────────────

    #[test]
    fn hit_returns_stored_body() {
        let cache = ResponseCache::new(config(60));
        let key = cache_key(&simple_request());
        cache.put(key.clone(), json!({"id": "chatcmpl-1"}));
        assert_eq!(cache.get(&key), Some(json!({"id": "chatcmpl-1"})));
    }

    #[test]
    fn entries_expire_with_ttl() {
        let cache = ResponseCache::new(config(60));
        let t0 = Instant::now();
        cache.put_at("k".into(), json!({"v": 1}), t0);
        assert!(cache.get_at("k", t0 + Duration::from_secs(59)).is_some());
        assert!(cache.get_at("k", t0 + Duration::from_secs(61)).is_none());
        // Expired entry is purged on read.
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn disabled_by_default_and_never_serves() {
        // Default TTL is 0 = disabled, opt-in.
        assert!(!ResponseCacheConfig::parse(None, None).enabled());
        let cache = ResponseCache::new(config(0));
        cache.put("k".into(), json!({"v": 1}));
        assert_eq!(cache.len(), 0);
        assert!(cache.get("k").is_none());
    }

    #[test]
    fn evicts_oldest_at_capacity() {
        let cache = ResponseCache::new(config(3600));
        let t0 = Instant::now();
        for i in 0..4 {
            cache.put_at(format!("k{i}"), json!({"i": i}), t0 + Duration::from_secs(i as u64));
        }
        assert_eq!(cache.len(), 4);
        cache.put_at("k4".into(), json!({"i": 4}), t0 + Duration::from_secs(4));
        assert_eq!(cache.len(), 4);
        assert!(cache.get_at("k0", t0 + Duration::from_secs(5)).is_none());
        assert!(cache.get_at("k4", t0 + Duration::from_secs(5)).is_some());
    }

    #[test]
    fn eviction_prefers_purging_expired() {
        let cache = ResponseCache::new(config(10));
        let t0 = Instant::now();
        for i in 0..4 {
            cache.put_at(format!("k{i}"), json!({"i": i}), t0);
        }
        // All four are expired at t0+20; inserting must purge them, not evict
        // a live entry (there are none), and land the new key.
        cache.put_at("new".into(), json!({"i": 9}), t0 + Duration::from_secs(20));
        assert_eq!(cache.len(), 1);
        assert!(cache.get_at("new", t0 + Duration::from_secs(21)).is_some());
    }

    // ── hit short-circuit shape (mirrors the proxy handler) ─────────────

    #[test]
    fn warm_cache_serves_body_without_upstream_and_tokens_are_meterable() {
        let cache = ResponseCache::new(config(60));
        let req = simple_request();
        assert!(is_cacheable(&req));
        let key = cache_key(&req);
        // Miss first (this is where the handler would call upstream).
        assert!(cache.get(&key).is_none());
        // Store the upstream chat.completion body.
        let body = json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}
        });
        cache.put(key.clone(), body.clone());
        // Second identical request: hit — the handler returns before any
        // session create/upstream send, and token counts parse for the
        // zero-cost usage row.
        let hit = cache.get(&key).expect("warm entry");
        assert_eq!(hit, body);
        let usage = hit.get("usage").unwrap();
        assert_eq!(usage.get("prompt_tokens").and_then(Value::as_i64), Some(11));
        assert_eq!(usage.get("completion_tokens").and_then(Value::as_i64), Some(7));
    }


    #[test]
    fn multipart_and_text_content_hash_differently() {
        let text = request(r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#);
        let parts = request(r#"{"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#);
        assert_ne!(cache_key(&text), cache_key(&parts));
    }

    #[test]
    fn empty_tools_vec_does_not_bypass_but_hashes_like_absent() {
        let none = simple_request();
        let empty = request(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"tools":[]}"#);
        let _empty_msg: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert!(is_cacheable(&empty));
        assert_eq!(cache_key(&none), cache_key(&empty));
    }
}
