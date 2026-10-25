//! Static model alias catalog.
//!
//! Maps Allternit-facing model ids to upstream provider/model pairs. This is
//! intentionally conservative: only models we can actually route today are
//! listed. Live upstream metadata is merged in by the router at runtime.

use std::collections::HashMap;

/// One entry in the alias catalog.
#[derive(Debug, Clone)]
pub struct ModelAliasEntry {
    /// Allternit-facing model id (e.g. `llama-3.1-8b`).
    pub alias: String,
    /// Which provider owns this alias (e.g. `openrouter`).
    pub provider: String,
    /// The provider's native model id (e.g. `meta-llama/llama-3.1-8b-instruct`).
    pub upstream_id: String,
    /// Additional ids that resolve to this same model.
    pub aliases: Option<Vec<String>>,
    /// Unix timestamp for the `/v1/models` `created` field.
    pub created: u64,
    /// Human-readable model name shown in the catalog.
    pub name: String,
    /// Input price per 1M tokens (USD). Used when live upstream pricing is unavailable.
    pub prompt_price: f64,
    /// Output price per 1M tokens (USD). Used when live upstream pricing is unavailable.
    pub completion_price: f64,
    /// Context length in tokens.
    pub context_length: u64,
}

/// Immutable catalog of model aliases.
#[derive(Debug, Clone, Default)]
pub struct ModelAliasMap {
    entries: Vec<ModelAliasEntry>,
    by_alias: HashMap<String, usize>,
}

impl ModelAliasMap {
    /// Build a catalog from a list of entries.
    pub fn new(entries: Vec<ModelAliasEntry>) -> Self {
        let mut by_alias = HashMap::with_capacity(entries.len() * 2);
        for (idx, entry) in entries.iter().enumerate() {
            by_alias.insert(entry.alias.clone(), idx);
            if let Some(aliases) = &entry.aliases {
                for a in aliases {
                    by_alias.insert(a.clone(), idx);
                }
            }
        }
        Self { entries, by_alias }
    }

    /// Resolve an Allternit model id to its catalog entry.
    pub fn resolve(&self, alias: &str) -> Option<&ModelAliasEntry> {
        self.by_alias.get(alias).and_then(|&idx| self.entries.get(idx))
    }

    /// Iterate all primary entries (one per model, not per alias).
    pub fn entries(&self) -> &[ModelAliasEntry] {
        &self.entries
    }
}

/// The production starter catalog. Routes are split across upstream providers.
/// OpenRouter provides broad coverage; Together AI provides curated open-weight
/// models with direct pricing and context metadata.
pub fn starter_catalog() -> ModelAliasMap {
    ModelAliasMap::new(vec![
        ModelAliasEntry {
            alias: "llama-3.1-8b".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "meta-llama/llama-3.1-8b-instruct".to_string(),
            aliases: Some(vec!["llama3.1-8b".to_string()]),
            created: 1722470400, // 2024-08-01
            name: "Llama 3.1 8B Instruct".to_string(),
            prompt_price: 0.10,
            completion_price: 0.20,
            context_length: 128_000,
        },
        ModelAliasEntry {
            alias: "llama-3.1-70b".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "meta-llama/llama-3.1-70b-instruct".to_string(),
            aliases: Some(vec!["llama3.1-70b".to_string()]),
            created: 1722470400,
            name: "Llama 3.1 70B Instruct".to_string(),
            prompt_price: 0.30,
            completion_price: 0.40,
            context_length: 128_000,
        },
        // `anthropic/claude-sonnet-4` was retired on OpenRouter (2026-10).
        // Bots saved with `claude-sonnet-4` keep working through the alias.
        ModelAliasEntry {
            alias: "claude-sonnet-4.6".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "anthropic/claude-sonnet-4.6".to_string(),
            aliases: Some(vec!["claude-sonnet".to_string(), "claude-sonnet-4".to_string()]),
            created: 1771286400, // 2026-02-17
            name: "Claude Sonnet 4.6".to_string(),
            prompt_price: 3.00,
            completion_price: 15.00,
            context_length: 1_000_000,
        },
        ModelAliasEntry {
            alias: "gpt-4o".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "openai/gpt-4o".to_string(),
            aliases: Some(vec!["gpt4o".to_string()]),
            created: 1715731200, // 2024-05-15
            name: "GPT-4o".to_string(),
            prompt_price: 2.50,
            completion_price: 10.00,
            context_length: 128_000,
        },
        ModelAliasEntry {
            alias: "qwen-2.5-72b".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "qwen/qwen-2.5-72b-instruct".to_string(),
            aliases: Some(vec!["qwen2.5-72b".to_string()]),
            created: 1727740800, // 2024-10-01
            name: "Qwen 2.5 72B Instruct".to_string(),
            prompt_price: 0.30,
            completion_price: 0.40,
            context_length: 128_000,
        },
        ModelAliasEntry {
            alias: "mistral-large".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "mistralai/mistral-large".to_string(),
            aliases: Some(vec!["mistral".to_string()]),
            created: 1709251200, // 2024-03-01
            name: "Mistral Large".to_string(),
            prompt_price: 2.00,
            completion_price: 6.00,
            context_length: 128_000,
        },
        // Stock open-weight flash models (OpenRouter lists both, 2026-10-25).
        ModelAliasEntry {
            alias: "deepseek-v4.1-flash".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "deepseek/deepseek-v4.1-flash".to_string(),
            aliases: None,
            created: 1760659200, // 2026-10-17
            name: "DeepSeek V4.1 Flash".to_string(),
            prompt_price: 0.03,
            completion_price: 1.50,
            context_length: 1_048_576,
        },
        ModelAliasEntry {
            alias: "glm-5.3-flash".to_string(),
            provider: "openrouter".to_string(),
            upstream_id: "z-ai/glm-5.3-flash".to_string(),
            aliases: None,
            created: 1753929600,
            name: "GLM 5.3 Flash".to_string(),
            prompt_price: 0.23,
            completion_price: 0.75,
            context_length: 1_048_576,
        },
        // Together AI — curated open-weight models (prices per 1M tokens, USD).
        ModelAliasEntry {
            alias: "llama-3.3-70b-turbo".to_string(),
            provider: "together".to_string(),
            upstream_id: "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
            aliases: Some(vec!["llama-3.3-70b".to_string()]),
            created: 1733011200, // 2024-12-01
            name: "Llama 3.3 70B Instruct Turbo".to_string(),
            prompt_price: 1.04,
            completion_price: 1.04,
            context_length: 131_072,
        },
        ModelAliasEntry {
            alias: "deepseek-v4-pro".to_string(),
            provider: "together".to_string(),
            upstream_id: "deepseek-ai/DeepSeek-V4-Pro-0813".to_string(),
            aliases: Some(vec!["deepseek-v4".to_string()]),
            created: 1753929600, // 2025-07-31
            name: "DeepSeek V4 Pro".to_string(),
            prompt_price: 1.32,
            completion_price: 3.96,
            context_length: 1_048_576,
        },
        ModelAliasEntry {
            alias: "kimi-k3".to_string(),
            provider: "together".to_string(),
            upstream_id: "moonshotai/Kimi-K3".to_string(),
            aliases: None,
            created: 1753929600, // 2025-07-31
            name: "Kimi K3".to_string(),
            prompt_price: 3.00,
            completion_price: 15.00,
            context_length: 1_048_576,
        },
        ModelAliasEntry {
            alias: "glm-5.3".to_string(),
            provider: "together".to_string(),
            upstream_id: "zai-org/GLM-5.3".to_string(),
            aliases: None,
            created: 1753929600, // 2025-07-31
            name: "GLM 5.3".to_string(),
            prompt_price: 1.40,
            completion_price: 4.40,
            context_length: 1_048_575,
        },
        ModelAliasEntry {
            alias: "qwen3.8-2.4t-a95b".to_string(),
            provider: "together".to_string(),
            upstream_id: "Qwen/Qwen3.8-2.4T-A95B".to_string(),
            aliases: Some(vec!["qwen3.8-2.4t".to_string()]),
            created: 1753929600, // 2025-07-31
            name: "Qwen3.8 2.4T A95B".to_string(),
            prompt_price: 2.00,
            completion_price: 6.00,
            context_length: 1_010_000,
        },
        ModelAliasEntry {
            alias: "qwen2.5-72b-together".to_string(),
            provider: "together".to_string(),
            upstream_id: "Qwen/Qwen2.5-72B-Instruct".to_string(),
            aliases: Some(vec!["qwen2.5-72b-t".to_string()]),
            created: 1727740800, // 2024-10-01
            name: "Qwen 2.5 72B Instruct".to_string(),
            prompt_price: 1.20,
            completion_price: 1.20,
            context_length: 32_768,
        },
        // Fireworks AI — curated serverless models (prices per 1M tokens, USD,
        // Standard serving path; format is input / cached input / output).
        ModelAliasEntry {
            alias: "kimi-k3-fireworks".to_string(),
            provider: "fireworks".to_string(),
            upstream_id: "accounts/fireworks/models/kimi-k3".to_string(),
            aliases: None,
            created: 1753929600, // 2025-07-31
            name: "Kimi K3 (Fireworks)".to_string(),
            prompt_price: 3.00,
            completion_price: 15.00,
            context_length: 1_048_576,
        },
        ModelAliasEntry {
            alias: "deepseek-v4-pro-fireworks".to_string(),
            provider: "fireworks".to_string(),
            upstream_id: "accounts/fireworks/models/deepseek-v4-pro-0813".to_string(),
            aliases: None,
            created: 1753929600, // 2025-07-31
            name: "DeepSeek V4 Pro (Fireworks)".to_string(),
            prompt_price: 1.32,
            completion_price: 3.96,
            context_length: 1_048_576,
        },
        ModelAliasEntry {
            alias: "glm-5.3-fireworks".to_string(),
            provider: "fireworks".to_string(),
            upstream_id: "accounts/fireworks/models/glm-5p3".to_string(),
            aliases: None,
            created: 1753929600, // 2025-07-31
            name: "GLM 5.3 (Fireworks)".to_string(),
            prompt_price: 1.40,
            completion_price: 4.40,
            context_length: 1_048_576,
        },
        ModelAliasEntry {
            alias: "qwen3.8-max-fireworks".to_string(),
            provider: "fireworks".to_string(),
            upstream_id: "accounts/fireworks/models/qwen3p8-max".to_string(),
            aliases: None,
            created: 1753929600, // 2025-07-31
            name: "Qwen 3.8 Max (Fireworks)".to_string(),
            prompt_price: 2.00,
            completion_price: 6.00,
            context_length: 1_000_000,
        },
        // DeepInfra — cost-optimized open-weight inference (prices per 1M tokens, USD).
        ModelAliasEntry {
            alias: "llama-3.3-70b-deepinfra".to_string(),
            provider: "deepinfra".to_string(),
            upstream_id: "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
            aliases: None,
            created: 1733011200, // 2024-12-01
            name: "Llama 3.3 70B Instruct Turbo (DeepInfra)".to_string(),
            prompt_price: 0.49,
            completion_price: 0.73,
            context_length: 131_072,
        },
        ModelAliasEntry {
            alias: "qwen2.5-72b-deepinfra".to_string(),
            provider: "deepinfra".to_string(),
            upstream_id: "Qwen/Qwen2.5-72B-Instruct".to_string(),
            aliases: None,
            created: 1727740800, // 2024-10-01
            name: "Qwen 2.5 72B Instruct (DeepInfra)".to_string(),
            prompt_price: 0.49,
            completion_price: 0.49,
            context_length: 131_072,
        },
        ModelAliasEntry {
            alias: "deepseek-v3-deepinfra".to_string(),
            provider: "deepinfra".to_string(),
            upstream_id: "deepseek-ai/DeepSeek-V3".to_string(),
            aliases: None,
            created: 1735689600, // 2025-01-01
            name: "DeepSeek V3 (DeepInfra)".to_string(),
            prompt_price: 0.99,
            completion_price: 0.99,
            context_length: 64_000,
        },
        // Groq — ultra-low-latency inference (prices per 1M tokens, USD).
        ModelAliasEntry {
            alias: "qwen3.6-27b-groq".to_string(),
            provider: "groq".to_string(),
            upstream_id: "qwen/qwen3.6-27b".to_string(),
            aliases: None,
            created: 1756857600, // 2026-09-01
            name: "Qwen 3.6 27B (Groq)".to_string(),
            prompt_price: 0.60,
            completion_price: 3.00,
            context_length: 131_072,
        },
        ModelAliasEntry {
            alias: "qwen3.8-27b-groq".to_string(),
            provider: "groq".to_string(),
            upstream_id: "qwen/qwen3.8-27b".to_string(),
            aliases: None,
            created: 1756857600,
            name: "Qwen 3.8 27B (Groq)".to_string(),
            prompt_price: 0.80,
            completion_price: 4.00,
            context_length: 131_072,
        },
        ModelAliasEntry {
            alias: "gpt-oss-20b-groq".to_string(),
            provider: "groq".to_string(),
            upstream_id: "openai/gpt-oss-20b".to_string(),
            aliases: None,
            created: 1756857600,
            name: "GPT-OSS 20B (Groq)".to_string(),
            prompt_price: 0.075,
            completion_price: 0.30,
            context_length: 131_072,
        },
        ModelAliasEntry {
            alias: "gpt-oss-120b-groq".to_string(),
            provider: "groq".to_string(),
            upstream_id: "openai/gpt-oss-120b".to_string(),
            aliases: None,
            created: 1756857600,
            name: "GPT-OSS 120B (Groq)".to_string(),
            prompt_price: 0.15,
            completion_price: 0.60,
            context_length: 131_072,
        },
        // Open lane — open-weight models (stock or our own "-open" variants)
        // that answer many requests frontier models refuse. Served by the
        // `open_lane` provider (OPEN_LANE_BASE_URL / OPEN_LANE_API_KEY);
        // hidden from /v1/models while that provider is unset. Every request
        // passes the hard-line filter (services::hard_line) first.
        open_lane_entry(
            "deepseek-v4.1-flash-open",
            "DeepSeek V4.1 Flash (Open)",
            0.10,
            2.00,
            1_048_576,
        ),
        open_lane_entry("glm-5.3-flash-open", "GLM 5.3 Flash (Open)", 0.40, 1.20, 1_048_576),
    ])
}

/// Provider id of the open lane (the `OPEN_LANE_*` OpenAI-compatible upstream).
pub const OPEN_LANE_PROVIDER: &str = "open_lane";

/// Whether a catalog entry belongs to the open lane.
pub fn is_open_lane(entry: &ModelAliasEntry) -> bool {
    entry.provider == OPEN_LANE_PROVIDER
}

/// The upstream model id an open-lane alias is served as. Defaults to the
/// alias itself (how our own servers name them, e.g. vLLM
/// `--served-model-name`); `OPEN_LANE_MODEL_MAP` overrides per alias while a
/// third-party host serves them: `alias=upstream,alias=upstream`.
fn open_lane_upstream(alias: &str) -> String {
    std::env::var("OPEN_LANE_MODEL_MAP")
        .ok()
        .and_then(|map| {
            map.split(',').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key.trim() == alias && !value.trim().is_empty()).then(|| value.trim().to_string())
            })
        })
        .unwrap_or_else(|| alias.to_string())
}

fn open_lane_entry(
    alias: &str,
    name: &str,
    prompt_price: f64,
    completion_price: f64,
    context_length: u64,
) -> ModelAliasEntry {
    ModelAliasEntry {
        alias: alias.to_string(),
        provider: OPEN_LANE_PROVIDER.to_string(),
        upstream_id: open_lane_upstream(alias),
        aliases: None,
        created: 1761350400, // 2026-10-25
        name: name.to_string(),
        prompt_price,
        completion_price,
        context_length,
    }
}
