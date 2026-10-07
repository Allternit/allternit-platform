//! Gateway-side cost recomputation (B4 pricing hardening).
//!
//! Gizzi reports token counts and a dollar cost on every assistant message
//! (`message.updated`); the gateway stores that figure verbatim. This module
//! independently recomputes the cost from the same models.dev cache Gizzi
//! prices from (`~/.cache/gizzi-code/models.json`, refreshed hourly by
//! `cmd/gizzi-code/src/runtime/providers/adapters/models.ts`), applying the
//! exact `Session.getUsage` formula
//! (`cmd/gizzi-code/src/runtime/session/index.ts:873-945`):
//!
//! ```text
//! cost = input*in/1M + output*out/1M + cache_read*cache_read/1M
//!      + cache_write*cache_write/1M + reasoning*out/1M
//! ```
//!
//! with the `context_over_200k` rates substituted when
//! `input + cache_read > 200_000`. The token breakdown consumed here is
//! Gizzi's *already-normalized* one (cache tokens split out of `input`), so
//! no provider-specific adjustment is needed on this side.
//!
//! Both figures are stored on the usage row; a |Δ| > 1% is flagged
//! (`cost_mismatch`) and logged. When the cache file is missing, unreadable,
//! or lacks the model, the recompute is skipped (NULL) and the Gizzi-reported
//! cost stands alone.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use tracing::warn;

/// Per-1M-token dollar rates for one model.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    /// Rates applied when input + cache_read exceeds 200k tokens.
    pub context_over_200k: Option<Box<ModelPricing>>,
}

/// Token breakdown of one request, Gizzi-normalized (`message.updated`).
#[derive(Debug, Clone, Default)]
pub struct TokenBreakdown {
    pub input: i64,
    pub output: i64,
    pub reasoning: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

/// Pricing map keyed by `"provider/model"`.
pub type PricingMap = HashMap<String, ModelPricing>;

// ─── models.dev cache parsing ────────────────────────────────────────────────

/// Raw `cost` block of a models.dev cache entry. Unknown fields (`tiers`,
/// ...) are ignored; absent rates default to 0 (free/unknown, same as Gizzi).
#[derive(Debug, Deserialize)]
struct RawCost {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
    #[serde(default)]
    context_over_200k: Option<Box<RawCostTier>>,
}

/// A `context_over_200k` block never nests further.
#[derive(Debug, Deserialize)]
struct RawCostTier {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    #[serde(default)]
    cost: Option<RawCost>,
}

#[derive(Debug, Deserialize)]
struct RawProvider {
    #[serde(default)]
    models: HashMap<String, RawModel>,
}

impl From<RawCostTier> for ModelPricing {
    fn from(tier: RawCostTier) -> Self {
        Self {
            input: tier.input,
            output: tier.output,
            cache_read: tier.cache_read,
            cache_write: tier.cache_write,
            context_over_200k: None,
        }
    }
}

impl From<RawCost> for ModelPricing {
    fn from(cost: RawCost) -> Self {
        Self {
            input: cost.input,
            output: cost.output,
            cache_read: cost.cache_read,
            cache_write: cost.cache_write,
            context_over_200k: cost
                .context_over_200k
                .map(|tier| Box::new(ModelPricing::from(*tier))),
        }
    }
}

/// Parse the models.dev cache JSON into a flat pricing map. Models without a
/// `cost` block are skipped (their cost is unknown, matching Gizzi's 0-rate
/// default would record misleading zeros).
pub fn parse_catalog(json_str: &str) -> Result<PricingMap, serde_json::Error> {
    let providers: HashMap<String, RawProvider> = serde_json::from_str(json_str)?;
    let mut map = PricingMap::new();
    for (provider_id, provider) in providers {
        for (model_id, model) in provider.models {
            if let Some(cost) = model.cost {
                map.insert(format!("{provider_id}/{model_id}"), ModelPricing::from(cost));
            }
        }
    }
    Ok(map)
}

/// Look up pricing for a model: exact `provider/model` first, then any
/// provider offering the bare model id (Gizzi provider ids match the cache,
/// but a bare-id fallback keeps the recompute working for custom providers).
pub fn find_pricing<'a>(
    map: &'a PricingMap,
    provider_id: &str,
    model_id: &str,
) -> Option<&'a ModelPricing> {
    if let Some(pricing) = map.get(&format!("{provider_id}/{model_id}")) {
        return Some(pricing);
    }
    let suffix = format!("/{model_id}");
    map.iter()
        .find(|(key, _)| key.ends_with(&suffix))
        .map(|(_, pricing)| pricing)
}

/// Apply the `Session.getUsage` formula. Reasoning tokens are billed at the
/// output rate (industry standard, same as Gizzi). Rates are $/1M tokens, so
/// `Σ(tokens × rate)` is already the cost in microdollars — computing in
/// microdollars directly (rather than dollars ÷ 1e6 × 1e6) avoids a
/// floating-point round trip.
pub fn cost_microdollars(pricing: &ModelPricing, tokens: &TokenBreakdown) -> i64 {
    let over_200k = tokens.input + tokens.cache_read > 200_000;
    let rates = match (over_200k, &pricing.context_over_200k) {
        (true, Some(tier)) => tier.as_ref(),
        _ => pricing,
    };
    let microdollars = tokens.input.max(0) as f64 * rates.input
        + tokens.output.max(0) as f64 * rates.output
        + tokens.cache_read.max(0) as f64 * rates.cache_read
        + tokens.cache_write.max(0) as f64 * rates.cache_write
        + tokens.reasoning.max(0) as f64 * rates.output;
    microdollars.round() as i64
}

/// [`cost_microdollars`] split into its input side (input + cache read +
/// cache write) and output side (output + reasoning), under the same
/// context tier. The two parts add up to the whole, give or take rounding.
pub fn cost_split_microdollars(pricing: &ModelPricing, tokens: &TokenBreakdown) -> (i64, i64) {
    let over_200k = tokens.input + tokens.cache_read > 200_000;
    let rates = match (over_200k, &pricing.context_over_200k) {
        (true, Some(tier)) => tier.as_ref(),
        _ => pricing,
    };
    let input = tokens.input.max(0) as f64 * rates.input
        + tokens.cache_read.max(0) as f64 * rates.cache_read
        + tokens.cache_write.max(0) as f64 * rates.cache_write;
    let output = (tokens.output.max(0) + tokens.reasoning.max(0)) as f64 * rates.output;
    (input.round() as i64, output.round() as i64)
}

/// [`cost_split_microdollars`] at list price from the models.dev cache; None
/// when the cache or the model is unavailable.
pub fn list_cost_split_microdollars(provider_id: &str, model_id: &str, tokens: &TokenBreakdown) -> Option<(i64, i64)> {
    let map = current_pricing()?;
    let pricing = find_pricing(&map, provider_id, model_id)?;
    Some(cost_split_microdollars(pricing, tokens))
}

// ─── Lazy, mtime-checked process-wide cache ──────────────────────────────────

/// Environment override for the cache location (tests, non-standard installs).
const MODELS_PATH_ENV: &str = "GIZZI_MODELS_PATH";

fn models_path() -> Option<PathBuf> {
    std::env::var(MODELS_PATH_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            dirs::home_dir().map(|home| home.join(".cache/gizzi-code/models.json"))
        })
}

struct PricingCache {
    /// mtime of the file the map was loaded from; None when the last attempt
    /// found no readable file (map is empty in that case).
    mtime: Option<SystemTime>,
    map: PricingMap,
}

static CACHE: Lazy<Mutex<Option<PricingCache>>> = Lazy::new(|| Mutex::new(None));

/// Load (or reload) the pricing map when the file changed since the last
/// load. Returns None when the file is absent or undecodable — callers then
/// skip the recompute entirely.
fn current_pricing() -> Option<PricingMap> {
    let path = models_path()?;
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

    let mut guard = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = guard.as_ref() {
        if cached.mtime == mtime && mtime.is_some() {
            return Some(cached.map.clone());
        }
    }

    let loaded: Option<PricingMap> = (|| {
        let contents = std::fs::read_to_string(&path).ok()?;
        match parse_catalog(&contents) {
            Ok(map) => Some(map),
            Err(err) => {
                warn!(error = %err, path = %path.display(), "models.dev cache not decodable; cost recompute disabled");
                None
            }
        }
    })();

    *guard = Some(PricingCache {
        mtime,
        map: loaded.clone().unwrap_or_default(),
    });
    loaded
}

/// Return the current pricing map snapshot. Returns an empty map when the
/// models.dev cache is missing or undecodable.
pub fn pricing_snapshot() -> PricingMap {
    current_pricing().unwrap_or_default()
}

/// Recompute the cost of one request in microdollars. Returns None when the
/// pricing cache or the model is unavailable — the caller then stores NULL
/// and bills off the Gizzi-reported figure alone.
pub fn recompute_cost_microdollars(
    provider_id: &str,
    model_id: &str,
    tokens: &TokenBreakdown,
) -> Option<i64> {
    let map = current_pricing()?;
    let pricing = find_pricing(&map, provider_id, model_id)?;
    Some(cost_microdollars(pricing, tokens))
}

/// Estimate the input-cost savings from provider-reported cache-read tokens
/// (task G11): what the cached tokens *would* have cost at the model's full
/// input rate had they not been served from cache. Same lookup and over-200k
/// tier logic as the cost recompute; rates are $/1M tokens so
/// `tokens × rate` is already microdollars. Returns None when the pricing
/// cache or model is unavailable (callers report savings as unknown, never
/// as zero).
pub fn estimated_cache_savings_microdollars(
    provider_id: &str,
    model_id: &str,
    cached_tokens: i64,
) -> Option<i64> {
    let map = current_pricing()?;
    let pricing = find_pricing(&map, provider_id, model_id)?;
    Some(cache_savings_microdollars(pricing, cached_tokens))
}

/// Savings math against an already-resolved rate card (kept pure so the
/// estimate is unit-testable without the models.dev file).
pub(crate) fn cache_savings_microdollars(pricing: &ModelPricing, cached_tokens: i64) -> i64 {
    let rates = match (cached_tokens > 200_000, &pricing.context_over_200k) {
        (true, Some(tier)) => tier.as_ref(),
        _ => pricing,
    };
    (cached_tokens.max(0) as f64 * rates.input).round() as i64
}

/// Whether the recomputed and Gizzi-reported costs disagree by more than 1%
/// (relative to the larger of the two). Both-zero is never a mismatch.
pub fn is_mismatch(recomputed: i64, reported: i64) -> bool {
    let denominator = recomputed.abs().max(reported.abs()) as f64;
    if denominator == 0.0 {
        return false;
    }
    (recomputed - reported).abs() as f64 / denominator > 0.01
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture mirroring the real models.dev cache shape (subset).
    fn fixture() -> PricingMap {
        let json = r#"{
            "anthropic": {
                "id": "anthropic",
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "cost": {
                            "input": 3.0,
                            "output": 15.0,
                            "cache_read": 0.3,
                            "cache_write": 3.75
                        }
                    },
                    "claude-opus-4-1": {
                        "id": "claude-opus-4-1",
                        "cost": {
                            "input": 15.0,
                            "output": 75.0,
                            "cache_read": 1.5,
                            "cache_write": 18.75,
                            "context_over_200k": {
                                "input": 30.0,
                                "output": 150.0,
                                "cache_read": 3.0,
                                "cache_write": 37.5
                            }
                        }
                    }
                }
            },
            "openai": {
                "id": "openai",
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o",
                        "cost": { "input": 2.5, "output": 10.0, "cache_read": 1.25 }
                    },
                    "free-local": { "id": "free-local" }
                }
            }
        }"#;
        parse_catalog(json).expect("fixture parses")
    }

    #[test]
    fn parses_models_dev_cache_shape() {
        let map = fixture();
        let pricing = map.get("anthropic/claude-sonnet-4-5").unwrap();
        assert_eq!(pricing.input, 3.0);
        assert_eq!(pricing.output, 15.0);
        assert_eq!(pricing.cache_read, 0.3);
        assert_eq!(pricing.cache_write, 3.75);
        assert!(pricing.context_over_200k.is_none());
        // Models without a cost block are skipped entirely.
        assert!(!map.contains_key("openai/free-local"));
    }

    #[test]
    fn getusage_formula_basic() {
        let map = fixture();
        let pricing = find_pricing(&map, "anthropic", "claude-sonnet-4-5").unwrap();
        // 1000 in, 500 out, 200 reasoning (output rate), 100 cache_read, 50 cache_write:
        // microdollars = 1000*3 + 500*15 + 200*15 + 100*0.3 + 50*3.75 = 13_717.5
        let tokens = TokenBreakdown {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 100,
            cache_write: 50,
        };
        assert_eq!(cost_microdollars(pricing, &tokens), 13_718);
    }

    #[test]
    fn getusage_formula_zero_usage_is_zero() {
        let map = fixture();
        let pricing = find_pricing(&map, "openai", "gpt-4o").unwrap();
        assert_eq!(cost_microdollars(pricing, &TokenBreakdown::default()), 0);
    }

    #[test]
    fn over_200k_tier_applies_only_above_threshold() {
        let map = fixture();
        let pricing = find_pricing(&map, "anthropic", "claude-opus-4-1").unwrap();

        // Below threshold (input + cache_read = 150_000 + 40_000 <= 200_000):
        // dollars = 150000*15/1e6 + 1000*75/1e6 + 40000*1.5/1e6 = 2.385
        let below = TokenBreakdown {
            input: 150_000,
            output: 1_000,
            reasoning: 0,
            cache_read: 40_000,
            cache_write: 0,
        };
        assert_eq!(cost_microdollars(pricing, &below), 2_385_000);

        // Above threshold: over-200k rates kick in for every component:
        // dollars = 150000*30/1e6 + 1000*150/1e6 + 60000*3.0/1e6 = 4.83
        let above = TokenBreakdown {
            input: 150_000,
            output: 1_000,
            reasoning: 0,
            cache_read: 60_000,
            cache_write: 0,
        };
        assert_eq!(cost_microdollars(pricing, &above), 4_830_000);
    }

    #[test]
    fn find_pricing_falls_back_to_bare_model_id() {
        let map = fixture();
        let pricing = find_pricing(&map, "custom-gateway", "gpt-4o").unwrap();
        assert_eq!(pricing.input, 2.5);
        assert!(find_pricing(&map, "openai", "no-such-model").is_none());
    }

    #[test]
    fn cache_savings_estimate_uses_input_rate() {
        let map = fixture();
        let sonnet = map.get("anthropic/claude-sonnet-4-5").unwrap();
        // 10_000 cached tokens × $3.0/1M input = 30_000 microdollars ($0.03).
        assert_eq!(cache_savings_microdollars(sonnet, 10_000), 30_000);

        // Negative/garbage token counts clamp to zero, never negative savings.
        assert_eq!(cache_savings_microdollars(sonnet, -5), 0);
        assert_eq!(cache_savings_microdollars(sonnet, 0), 0);
    }

    #[test]
    fn cache_savings_estimate_applies_over_200k_tier() {
        let map = fixture();
        let opus = map.get("anthropic/claude-opus-4-1").unwrap();
        // Above the 200k threshold the over-200k input rate applies.
        assert_eq!(cache_savings_microdollars(opus, 300_000), 9_000_000);
        // Below it the standard rate applies.
        assert_eq!(cache_savings_microdollars(opus, 100_000), 1_500_000);
    }

    #[test]
    fn mismatch_threshold() {
        assert!(!is_mismatch(0, 0));
        assert!(!is_mismatch(10_000, 10_050)); // 0.5% apart
        assert!(is_mismatch(10_000, 10_200)); // 2% apart
        assert!(is_mismatch(0, 1_000)); // one side missing entirely
        assert!(!is_mismatch(100, 101)); // exactly 1% is not a mismatch
    }
}
