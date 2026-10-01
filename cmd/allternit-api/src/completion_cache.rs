//! O7 — shared exact cache for internal completions (WP-K1).
//!
//! The gateway (`llm_gateway::proxy`) and internal `gizzi_completion` calls
//! share ONE store: [`crate::llm_gateway::response_cache::ResponseCache`].
//! This module owns what is specific to internal calls:
//!
//! - [`CallType`]: every internal completion names what it is, and the call
//!   type's [`CachePolicy`] decides cacheability and TTL. Unknown/unclassified
//!   calls are never cached.
//! - Hard exclusions live in code, not just in the table: a call type with
//!   side effects, tools, or user-personal context is never exact-cacheable
//!   and never semantic-eligible, whatever its TTL says.
//! - [`internal_key`]: SHA-256 over call type + model + params + tools +
//!   system + the full prompt.
//!
//! Env: `ALLTERNIT_COMPLETION_CACHE=off` disables the internal exact cache.
//! `LLM_RESPONSE_CACHE_MAX_ENTRIES` (shared) caps the store.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

use crate::llm_gateway::response_cache::ResponseCache;

/// What an internal completion is for. Adding a call site means adding (or
/// reusing) a variant and deciding its policy here, in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallType {
    /// OpenAI-compatible gateway chat (policy lives in `response_cache`:
    /// deterministic or explicit opt-in, per-tenant keys).
    GatewayChat,
    /// A://Labs lesson generation from a course + topic: doc-type, no user
    /// data, no tools, no side effects in the call itself.
    AlabsLesson,
    /// Memory extraction over the user's conversation (personal).
    MemoryExtraction,
    /// Memory curation over the user's memories (personal).
    MemoryCuration,
    /// Coordinator planning for a run (drives side effects downstream).
    CoordinatorPlan,
    /// Cowork team agent turns (act on the user's workspace).
    CoworkTeam,
    /// Agency executor nodes (side effects, budget-charged).
    AgencyNode,
    /// Anything not classified yet: never cached.
    Unclassified,
}

impl CallType {
    pub fn as_str(self) -> &'static str {
        match self {
            CallType::GatewayChat => "gateway.chat",
            CallType::AlabsLesson => "alabs.lesson",
            CallType::MemoryExtraction => "memory.extraction",
            CallType::MemoryCuration => "memory.curation",
            CallType::CoordinatorPlan => "coordinator.plan",
            CallType::CoworkTeam => "cowork.team",
            CallType::AgencyNode => "agency.node",
            CallType::Unclassified => "unclassified",
        }
    }

    pub fn policy(self) -> CachePolicy {
        let never = |side_effects: bool, personal: bool, tools: bool| CachePolicy {
            exact_ttl: None,
            semantic: false,
            side_effects,
            personal,
            tools,
            explicitly_cacheable: false,
        };
        match self {
            // Gateway cacheability is per request (temperature 0 or opt-in);
            // the gateway has its own env TTL and is never semantic here.
            CallType::GatewayChat => CachePolicy {
                exact_ttl: None,
                semantic: false,
                side_effects: false,
                personal: false,
                tools: false,
                explicitly_cacheable: false,
            },
            CallType::AlabsLesson => CachePolicy {
                exact_ttl: Some(Duration::from_secs(24 * 3600)),
                semantic: true,
                side_effects: false,
                personal: false,
                tools: false,
                explicitly_cacheable: true,
            },
            CallType::MemoryExtraction | CallType::MemoryCuration => never(false, true, false),
            CallType::CoordinatorPlan | CallType::CoworkTeam | CallType::AgencyNode => {
                never(true, true, true)
            }
            CallType::Unclassified => never(true, true, true),
        }
    }
}

/// Cacheability of one call type. `explicitly_cacheable` marks types whose
/// output is reusable even though the internal path does not pin
/// temperature 0 (O7: "deterministic or explicitly cacheable types").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CachePolicy {
    pub exact_ttl: Option<Duration>,
    pub semantic: bool,
    pub side_effects: bool,
    pub personal: bool,
    pub tools: bool,
    pub explicitly_cacheable: bool,
}

impl CachePolicy {
    /// Hard exclusions win over the table.
    fn excluded(&self) -> bool {
        self.side_effects || self.personal || self.tools
    }

    /// Exact cache TTL when the type is cacheable, `None` otherwise.
    pub fn exact(&self) -> Option<Duration> {
        if self.excluded() || !self.explicitly_cacheable {
            return None;
        }
        self.exact_ttl.filter(|t| !t.is_zero())
    }

    /// O9: only FAQ/doc-type, tool-free, non-personal, side-effect-free.
    pub fn semantic_eligible(&self) -> bool {
        self.semantic && !self.excluded()
    }
}

/// Whether the internal exact cache is switched on (default on; only types
/// marked cacheable use it).
pub fn internal_enabled() -> bool {
    !matches!(
        std::env::var("ALLTERNIT_COMPLETION_CACHE").ok().as_deref().map(str::trim),
        Some("off") | Some("0") | Some("false")
    )
}

/// SHA-256 key: call type + model + params + tools + system + full prompt.
pub fn internal_key(
    call_type: CallType,
    model: &str,
    params: &Value,
    tools: &Value,
    system: Option<&str>,
    prompt: &str,
) -> String {
    let canonical = json!({
        "ns": "internal",
        "call_type": call_type.as_str(),
        "model": model.trim(),
        "params": params,
        "tools": tools,
        "system": system.map(str::trim),
        "prompt": prompt,
    });
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// Look up a cached internal completion text. Counts hit/miss.
pub fn lookup(store: &ResponseCache, call_type: CallType, key: &str) -> Option<String> {
    call_type.policy().exact()?;
    let hit = store
        .get_any(key)
        .and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_string));
    crate::metrics::inc_completion_cache_event(
        "internal",
        call_type.as_str(),
        if hit.is_some() { "hit" } else { "miss" },
    );
    hit
}

/// Store a completed internal completion (non-empty text only).
pub fn store(store: &ResponseCache, call_type: CallType, key: &str, text: &str) -> bool {
    let Some(ttl) = call_type.policy().exact() else {
        return false;
    };
    if text.trim().is_empty() {
        return false;
    }
    store.put_with_ttl(key.to_string(), json!({ "text": text }), ttl);
    crate::metrics::inc_completion_cache_event("internal", call_type.as_str(), "store");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::response_cache::ResponseCacheConfig;

    const ALL: [CallType; 8] = [
        CallType::GatewayChat,
        CallType::AlabsLesson,
        CallType::MemoryExtraction,
        CallType::MemoryCuration,
        CallType::CoordinatorPlan,
        CallType::CoworkTeam,
        CallType::AgencyNode,
        CallType::Unclassified,
    ];

    fn store_cfg() -> ResponseCache {
        // Gateway switch off (ttl 0): internal entries still use their own TTL.
        ResponseCache::new(ResponseCacheConfig { ttl_secs: 0, max_entries: 16 })
    }

    #[test]
    fn key_covers_model_params_tools_system_and_prompt() {
        let base = internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!([]), Some("s"), "q");
        assert_eq!(base, internal_key(CallType::AlabsLesson, " p/m ", &json!({}), &json!([]), Some("s "), "q"));
        for other in [
            internal_key(CallType::AlabsLesson, "p/m2", &json!({}), &json!([]), Some("s"), "q"),
            internal_key(CallType::AlabsLesson, "p/m", &json!({"temperature": 0}), &json!([]), Some("s"), "q"),
            internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!(["t"]), Some("s"), "q"),
            internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!([]), None, "q"),
            internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!([]), Some("s"), "q2"),
            internal_key(CallType::MemoryCuration, "p/m", &json!({}), &json!([]), Some("s"), "q"),
        ] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn only_explicitly_cacheable_safe_types_are_cached() {
        for ct in ALL {
            let p = ct.policy();
            let cacheable = p.exact().is_some();
            assert_eq!(cacheable, ct == CallType::AlabsLesson, "{ct:?}");
            if p.side_effects || p.personal || p.tools {
                assert!(!cacheable && !p.semantic_eligible(), "{ct:?} must be hard-excluded");
            }
        }
    }

    #[test]
    fn hard_exclusions_override_the_table() {
        let mut p = CallType::AlabsLesson.policy();
        assert!(p.exact().is_some() && p.semantic_eligible());
        for flip in 0..3 {
            let mut q = p;
            match flip {
                0 => q.side_effects = true,
                1 => q.personal = true,
                _ => q.tools = true,
            }
            assert!(q.exact().is_none());
            assert!(!q.semantic_eligible());
        }
        p.explicitly_cacheable = false;
        assert!(p.exact().is_none(), "not deterministic and not explicitly cacheable");
    }

    #[test]
    fn store_and_lookup_respect_policy() {
        let cache = store_cfg();
        let k = internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!([]), None, "q");
        assert!(lookup(&cache, CallType::AlabsLesson, &k).is_none());
        assert!(store(&cache, CallType::AlabsLesson, &k, "lesson json"));
        assert_eq!(lookup(&cache, CallType::AlabsLesson, &k).as_deref(), Some("lesson json"));
        // Excluded types neither store nor read, even for a present key.
        assert!(!store(&cache, CallType::MemoryExtraction, &k, "x"));
        assert!(lookup(&cache, CallType::MemoryExtraction, &k).is_none());
        assert!(lookup(&cache, CallType::AgencyNode, &k).is_none());
        // Empty answers are never stored.
        let k2 = internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!([]), None, "q2");
        assert!(!store(&cache, CallType::AlabsLesson, &k2, "  "));
    }

    #[test]
    fn hits_and_misses_are_counted() {
        let cache = store_cfg();
        let before_hit = crate::metrics::completion_cache_event_count("internal", "alabs.lesson", "hit");
        let k = internal_key(CallType::AlabsLesson, "p/m", &json!({}), &json!([]), None, "count-me");
        store(&cache, CallType::AlabsLesson, &k, "a");
        lookup(&cache, CallType::AlabsLesson, &k);
        assert!(crate::metrics::completion_cache_event_count("internal", "alabs.lesson", "hit") > before_hit);
    }

    #[test]
    fn internal_ttl_is_per_type() {
        assert_eq!(CallType::AlabsLesson.policy().exact(), Some(Duration::from_secs(86_400)));
    }
}
