//! Dynamic model assessment for arbitrary Hugging Face repos.
//!
//! Parses the repo name, optionally fetches the actual GGUF file tree from HF,
//! and estimates download size, loaded size, memory fit, and decode tok/s for
//! the current hardware profile.

use crate::hardware::HardwareProfile;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Assessment request.
#[derive(Debug, Clone, Deserialize)]
pub struct AssessRequest {
    pub repo_id: String,
    pub quantization: Option<String>,
    pub context_length: Option<usize>,
}

/// Tok/s estimates at common context lengths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokPerSecondEstimates {
    pub context_4k: f32,
    pub context_8k: f32,
    pub context_16k: f32,
    pub context_32k: f32,
}

/// Assessment response.
#[derive(Debug, Clone, Serialize)]
pub struct AssessResponse {
    pub repo_id: String,
    pub fit: String,
    pub fit_reason: String,
    pub estimated_download_bytes: u64,
    pub estimated_loaded_bytes: u64,
    pub estimated_tok_per_second: TokPerSecondEstimates,
    pub recommended_backend: String,
    pub confidence: String,
    pub quantization_bits: f32,
    pub hardware_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChipsFile {
    chips: Vec<ChipEntry>,
    cpu_fallback_base_tok_per_sec_q4_7b_4k: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChipEntry {
    pattern: String,
    base_tok_per_sec_q4_7b_4k: f32,
}

/// Shared assessor.
#[derive(Clone)]
pub struct Assessor {
    client: Client,
    chips: ChipsFile,
}

impl Default for Assessor {
    fn default() -> Self {
        Self::new()
    }
}

impl Assessor {
    pub fn new() -> Self {
        const CHIPS_JSON: &str = include_str!("../../assess/chips.json");
        let chips: ChipsFile = serde_json::from_str(CHIPS_JSON).unwrap_or(ChipsFile {
            chips: vec![],
            cpu_fallback_base_tok_per_sec_q4_7b_4k: 5.0,
        });
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .unwrap_or_default(),
            chips,
        }
    }

    /// Assess a repo against the provided hardware profile.
    pub async fn assess(
        &self,
        request: AssessRequest,
        hardware: &HardwareProfile,
    ) -> AssessResponse {
        let repo_id = request.repo_id.trim().to_string();
        let parsed = parse_repo_id(&repo_id);
        let quant = parse_quantization(
            request.quantization.as_deref().unwrap_or(""),
            &parsed.quant_from_name,
        );

        let tree = self.fetch_tree(&repo_id).await;
        let has_tree = tree.is_some();
        let gguf_files: Vec<TreeFile> = tree
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.path.to_lowercase().ends_with(".gguf"))
            .collect();

        let file_size_bytes: Option<u64> = if gguf_files.is_empty() {
            None
        } else {
            Some(gguf_files.iter().map(|f| f.size).sum())
        };

        let (estimated_download_bytes, confidence) = if let Some(size) = file_size_bytes {
            (size, "inferred")
        } else if let (Some(params), Some(bits)) = (parsed.total_params_b, quant.bits_per_param) {
            let bytes = estimate_bytes_from_params(params, bits);
            (bytes, "guess")
        } else {
            (0, "guess")
        };

        let active_params_b = parsed
            .active_params_b
            .or(parsed.total_params_b)
            .unwrap_or(0.0);
        let total_params_b = parsed.total_params_b.unwrap_or(active_params_b);

        let context = request.context_length.unwrap_or(4096).max(1);
        let loaded_bytes = estimate_loaded_bytes(
            estimated_download_bytes,
            total_params_b,
            active_params_b,
            context,
        );

        let memory_budget = hardware.memory_budget_bytes();
        let (fit, fit_reason) = compute_fit(loaded_bytes, memory_budget);

        let tok_estimates = estimate_tok_per_second(
            hardware,
            &self.chips,
            active_params_b,
            quant.bits_per_param.unwrap_or(4.0),
        );

        let recommended_backend = if hardware.backends.metal {
            "mlx".to_string()
        } else {
            "llama_cpp".to_string()
        };

        AssessResponse {
            repo_id,
            fit,
            fit_reason,
            estimated_download_bytes,
            estimated_loaded_bytes: loaded_bytes,
            estimated_tok_per_second: tok_estimates,
            recommended_backend,
            confidence: if has_tree { "inferred" } else { confidence }.to_string(),
            quantization_bits: quant.bits_per_param.unwrap_or(4.0),
            hardware_id: hardware.hardware_id.clone(),
        }
    }

    async fn fetch_tree(&self, repo_id: &str) -> Option<Vec<TreeFile>> {
        for revision in ["main", "master"] {
            let url = format!(
                "https://huggingface.co/api/models/{}/tree/{}",
                urlencoding::encode(repo_id),
                revision
            );
            match self.client.get(&url).send().await {
                Ok(res) if res.status().is_success() => {
                    if let Ok(rows) = res.json::<Vec<serde_json::Value>>().await {
                        let files: Vec<TreeFile> = rows
                            .into_iter()
                            .filter_map(|row| {
                                let path = row["path"].as_str()?.to_string();
                                let size = row["size"].as_u64()?;
                                let kind = row["type"].as_str()?;
                                if kind == "file" {
                                    Some(TreeFile { path, size })
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !files.is_empty() {
                            return Some(files);
                        }
                    }
                }
                _ => continue,
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
struct TreeFile {
    path: String,
    size: u64,
}

#[derive(Debug, Clone, Default)]
struct ParsedRepo {
    total_params_b: Option<f32>,
    active_params_b: Option<f32>,
    quant_from_name: Option<String>,
}

fn parse_repo_id(repo_id: &str) -> ParsedRepo {
    let lower = repo_id.to_lowercase();
    let mut parsed = ParsedRepo::default();

    // Active params first: e.g. "A3B" or "16x3B".
    if let Some(caps) = regex_captures(r"[\-_.](\d+)x(\d+(?:\.\d+)?)b[\-_.]", &lower) {
        if let Some(active) = caps.get(2) {
            parsed.active_params_b = active.as_str().parse().ok();
        }
    } else if let Some(caps) = regex_captures(r"[\-_.]a(\d+(?:\.\d+)?)b[\-_.]", &lower) {
        if let Some(active) = caps.get(1) {
            parsed.active_params_b = active.as_str().parse().ok();
        }
    }

    // Total params: e.g. "7B", "1.5B".
    if let Some(caps) = regex_captures(r"(\d+(?:\.\d+)?)\s*b", &lower) {
        if let Some(total) = caps.get(1) {
            parsed.total_params_b = total.as_str().parse().ok();
        }
    }

    // Quantization from name.
    for q in [
        "q2_k", "q3_k", "q4_k", "q5_k", "q6_k", "q8_0", "fp16", "bf16", "fp32",
    ] {
        if lower.contains(q) {
            parsed.quant_from_name = Some(q.to_string());
            break;
        }
    }

    parsed
}

#[derive(Debug, Clone, Default)]
struct QuantInfo {
    bits_per_param: Option<f32>,
}

fn parse_quantization(input: &str, from_name: &Option<String>) -> QuantInfo {
    let lower = input.to_lowercase();
    let source = if !lower.is_empty() {
        Some(lower)
    } else {
        from_name.as_ref().map(|s| s.to_lowercase())
    };

    let bits = match source.as_deref() {
        Some("q2_k") => Some(2.0),
        Some("q3_k") => Some(3.0),
        Some("q4_k") => Some(4.0),
        Some("q5_k") => Some(5.0),
        Some("q6_k") => Some(6.0),
        Some("q8_0") => Some(8.0),
        Some("fp16") | Some("bf16") => Some(16.0),
        Some("fp32") => Some(32.0),
        _ => Some(4.0),
    };
    QuantInfo {
        bits_per_param: bits,
    }
}

fn estimate_bytes_from_params(params_b: f32, bits_per_param: f32) -> u64 {
    (params_b * 1_000_000_000.0 * bits_per_param / 8.0) as u64
}

fn estimate_loaded_bytes(
    file_size_bytes: u64,
    total_params_b: f32,
    active_params_b: f32,
    context_length: usize,
) -> u64 {
    let weights = file_size_bytes.max(estimate_bytes_from_params(total_params_b, 4.0));

    // Estimate KV-cache overhead for the requested context.
    // Heuristic: layers ≈ params_b * 4, hidden_size ≈ params_b * 512,
    // dtype f16 (2 bytes), K+V => 2 * layers * hidden_size * 2 bytes/token.
    let layers = (active_params_b * 4.0).round().max(1.0) as usize;
    let hidden_size = (active_params_b * 512.0).round().max(512.0) as usize;
    let kv_bytes_per_token = 2 * layers * hidden_size * 2;
    let kv_bytes = context_length.saturating_mul(kv_bytes_per_token);

    weights + kv_bytes as u64
}

fn compute_fit(loaded_bytes: u64, budget_bytes: u64) -> (String, String) {
    if budget_bytes == 0 {
        return ("no".to_string(), "Hardware memory not detected".to_string());
    }
    // Require 1.5x headroom for weights + activations + OS/services.
    let required = (loaded_bytes as f64 * 1.5) as u64;
    let ratio = required as f64 / budget_bytes as f64;

    if ratio <= 0.6 {
        (
            "fits".to_string(),
            format!(
                "Fits comfortably (requires ~{:.1} GB of {:.1} GB)",
                required as f64 / 1e9,
                budget_bytes as f64 / 1e9
            ),
        )
    } else if ratio <= 0.9 {
        (
            "tight".to_string(),
            format!(
                "Tight fit (requires ~{:.1} GB of {:.1} GB); reduce context if needed",
                required as f64 / 1e9,
                budget_bytes as f64 / 1e9
            ),
        )
    } else {
        (
            "no".to_string(),
            format!(
                "Likely exceeds available memory (requires ~{:.1} GB of {:.1} GB)",
                required as f64 / 1e9,
                budget_bytes as f64 / 1e9
            ),
        )
    }
}

fn estimate_tok_per_second(
    hardware: &HardwareProfile,
    chips: &ChipsFile,
    active_params_b: f32,
    bits_per_param: f32,
) -> TokPerSecondEstimates {
    let gpu_name = hardware.gpu_name.as_deref().unwrap_or("").to_lowercase();
    let base = chips
        .chips
        .iter()
        .find(|c| gpu_name.contains(&c.pattern.to_lowercase()))
        .map(|c| c.base_tok_per_sec_q4_7b_4k)
        .unwrap_or_else(|| {
            if hardware.backends.cuda {
                40.0
            } else {
                chips.cpu_fallback_base_tok_per_sec_q4_7b_4k
            }
        });

    // Scale by quantization relative to Q4.
    let quant_factor = match bits_per_param {
        b if b <= 2.0 => 1.35,
        b if b <= 3.0 => 1.15,
        b if b <= 4.0 => 1.0,
        b if b <= 5.0 => 0.85,
        b if b <= 6.0 => 0.75,
        b if b <= 8.0 => 0.6,
        b if b <= 16.0 => 0.3,
        _ => 0.15,
    };

    // Scale by active params relative to 7B.
    let params_factor = if active_params_b > 0.0 {
        (7.0 / active_params_b).sqrt().min(3.0)
    } else {
        1.0
    };

    let base_for_model = base * quant_factor * params_factor;

    TokPerSecondEstimates {
        context_4k: apply_context_factor(base_for_model, 4096),
        context_8k: apply_context_factor(base_for_model, 8192),
        context_16k: apply_context_factor(base_for_model, 16384),
        context_32k: apply_context_factor(base_for_model, 32768),
    }
}

fn apply_context_factor(base: f32, context: usize) -> f32 {
    if context <= 4096 {
        return base;
    }
    let ratio = (context as f32 / 4096.0).log2().max(0.0);
    let factor = 1.0 - (ratio * 0.1);
    (base * factor).max(1.0)
}

fn regex_captures<'t>(pattern: &str, text: &'t str) -> Option<regex::Captures<'t>> {
    let re = regex::Regex::new(pattern).ok()?;
    re.captures(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_parsing() {
        let p = parse_repo_id("bartowski/Llama-3.2-3B-Instruct-GGUF");
        assert_eq!(p.total_params_b, Some(3.0));
        assert_eq!(p.quant_from_name, None);

        let p2 = parse_repo_id("unsloth/DeepSeek-R1-Distill-Qwen-32B-GGUF");
        assert_eq!(p2.total_params_b, Some(32.0));

        let p3 = parse_repo_id("some/Qwen2.5-7B-Instruct-1M");
        assert_eq!(p3.total_params_b, Some(7.0));

        let p4 = parse_repo_id("bartowski/Llama-3.2-3B-Instruct-Q4_K_M-GGUF");
        assert_eq!(p4.quant_from_name, Some("q4_k".to_string()));
    }

    #[test]
    fn quant_parsing() {
        let q = parse_quantization("Q8_0", &None);
        assert_eq!(q.bits_per_param, Some(8.0));
        let q2 = parse_quantization("", &Some("q4_k".to_string()));
        assert_eq!(q2.bits_per_param, Some(4.0));
    }

    #[test]
    fn fit_computation() {
        let (fit, reason) = compute_fit(4_000_000_000, 36_000_000_000);
        assert_eq!(fit, "fits");
        assert!(!reason.is_empty());
    }
}
