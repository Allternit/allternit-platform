//! Model recommendation engine.
//!
//! Ranks models from the dynamic catalog by hardware fit and user intent
//! (balanced / smartest / fastest / lightweight), using the dynamic assessor
//! for per-model estimates.

use crate::assess::{AssessResponse, Assessor};
use crate::catalog::{CatalogEntry, CatalogService, CatalogSource};
use crate::hardware::HardwareProfile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Recommendation request.
#[derive(Debug, Clone, Deserialize)]
pub struct RecommendRequest {
    #[serde(default = "default_intent")]
    pub intent: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub backend_preference: Option<String>,
}

fn default_intent() -> String {
    "balanced".to_string()
}

fn default_limit() -> usize {
    5
}

/// A single recommendation.
#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    pub repo_id: String,
    pub fit: String,
    pub fit_reason: String,
    pub estimated_download_bytes: u64,
    pub estimated_loaded_bytes: u64,
    pub estimated_tok_per_second: crate::assess::TokPerSecondEstimates,
    pub recommended_backend: String,
    pub confidence: String,
    pub score: f64,
    pub explanation: String,
    pub downloads: u64,
    pub likes: u64,
}

/// Recommendation response.
#[derive(Debug, Clone, Serialize)]
pub struct RecommendResponse {
    pub recommendations: Vec<Recommendation>,
    pub hardware_id: String,
    pub timestamp: String,
}

/// Shared recommendation engine.
#[derive(Clone)]
pub struct Recommender {
    assessor: Assessor,
}

impl Default for Recommender {
    fn default() -> Self {
        Self::new()
    }
}

impl Recommender {
    pub fn new() -> Self {
        Self {
            assessor: Assessor::new(),
        }
    }

    pub async fn recommend(
        &self,
        request: RecommendRequest,
        catalog: &CatalogService,
        hardware: &HardwareProfile,
    ) -> RecommendResponse {
        let intent = request.intent.to_lowercase();
        let limit = request.limit.max(1).min(50);

        // Pull the merged catalog. Cap candidates so assessment stays fast.
        let candidates = catalog.catalog(CatalogSource::All, 100).await;

        let mut scored = Vec::with_capacity(candidates.len());
        for entry in candidates {
            let assess_req = crate::assess::AssessRequest {
                repo_id: entry.repo_id.clone(),
                quantization: None,
                context_length: Some(4096),
            };
            let assessment = self.assessor.assess(assess_req, hardware).await;

            // Skip models that definitely do not fit unless the intent is
            // "smartest", where we still want to show the largest option.
            if assessment.fit == "no" && intent != "smartest" {
                continue;
            }

            let (score, explanation) = score_candidate(&assessment, &entry, hardware, &intent);
            scored.push((score, assessment, entry, explanation));
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let recommendations = scored
            .into_iter()
            .map(|(score, a, entry, explanation)| Recommendation {
                repo_id: a.repo_id,
                fit: a.fit,
                fit_reason: a.fit_reason,
                estimated_download_bytes: a.estimated_download_bytes,
                estimated_loaded_bytes: a.estimated_loaded_bytes,
                estimated_tok_per_second: a.estimated_tok_per_second,
                recommended_backend: a.recommended_backend,
                confidence: a.confidence,
                score,
                explanation,
                downloads: entry.downloads,
                likes: entry.likes,
            })
            .collect();

        RecommendResponse {
            recommendations,
            hardware_id: hardware.hardware_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

fn score_candidate(
    a: &AssessResponse,
    entry: &CatalogEntry,
    hardware: &HardwareProfile,
    intent: &str,
) -> (f64, String) {
    let budget = hardware.memory_budget_bytes().max(1);
    let loaded_ratio = (a.estimated_loaded_bytes as f64 / budget as f64).min(1.0);
    let memory_score = 1.0 - loaded_ratio;

    // Parameter proxy for capability. Parse from the repo name if possible.
    let params_b = parse_params_b(&a.repo_id);
    let capability_score = ((params_b as f64 / 70.0).min(1.0)).max(0.05);

    // Speed score: log-scaled around 40 tok/s.
    let tok_s = a.estimated_tok_per_second.context_4k.max(1.0) as f64;
    let speed_score = (tok_s / (tok_s + 40.0)).min(1.0);

    // Fidelity score: higher bits → higher fidelity.
    let fidelity_score = (a.quantization_bits as f64 / 8.0).min(1.0);

    // Popularity signal (small nudge).
    let popularity_score = ((entry.downloads as f64).ln_1p() / 25.0).min(1.0);

    let weights: HashMap<&str, (f64, f64, f64, f64, f64)> = [
        ("balanced", (1.0, 1.0, 1.0, 1.0, 0.25)),
        ("smartest", (1.5, 0.5, 1.25, 0.5, 0.25)),
        ("fastest", (0.5, 2.0, 0.5, 1.0, 0.25)),
        ("lightweight", (0.5, 0.75, 0.5, 2.0, 0.25)),
    ]
    .iter()
    .copied()
    .collect();

    let (wc, ws, wf, wm, wp) = weights
        .get(intent)
        .copied()
        .unwrap_or((1.0, 1.0, 1.0, 1.0, 0.25));

    let score = (capability_score.powf(wc))
        * (speed_score.powf(ws))
        * (fidelity_score.powf(wf))
        * (memory_score.powf(wm))
        * (popularity_score.powf(wp));

    let explanation = build_explanation(intent, a, tok_s, params_b);
    (score, explanation)
}

fn build_explanation(intent: &str, a: &AssessResponse, tok_s: f64, params_b: f32) -> String {
    match intent {
        "smartest" => format!(
            "Largest model in the catalog (~{:.1}B params), estimated {:.1} tok/s at 4K context",
            params_b, tok_s
        ),
        "fastest" => format!(
            "Fastest fit on this machine: estimated {:.1} tok/s at 4K context",
            tok_s
        ),
        "lightweight" => format!(
            "Small download (~{:.1} GB) and fits comfortably",
            a.estimated_download_bytes as f64 / 1e9
        ),
        _ => format!(
            "Good balance: ~{:.1}B params, estimated {:.1} tok/s at 4K context",
            params_b, tok_s
        ),
    }
}

fn parse_params_b(repo_id: &str) -> f32 {
    let lower = repo_id.to_lowercase();
    let Some(re) = regex::Regex::new(r"(\d+(?:\.\d+)?)\s*b").ok() else {
        return 0.0;
    };
    re.captures(&lower)
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_parsing() {
        assert!((parse_params_b("bartowski/Llama-3.2-3B-Instruct-GGUF") - 3.0).abs() < 0.01);
        assert!((parse_params_b("unsloth/Qwen2.5-1.5B-Instruct-GGUF") - 1.5).abs() < 0.01);
    }
}
