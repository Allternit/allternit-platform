//! Curated hardware requirements for catalog entries.
//!
//! The generic assessor estimates size from the repo name and the sum of
//! every GGUF in a repo, which is wrong for multi-quant repos of large or
//! MoE models (unsloth's Qwen3.8-Flash-Next repo holds ~1.9 TB of quants).
//! Seed entries that carry `requirements` (Agency's local S2 class,
//! `mc.local`) are judged by these measured numbers instead, and `/recommend`
//! only offers them when one of the memory paths below is satisfied, whatever
//! the intent.

use crate::hardware::HardwareProfile;
use serde::{Deserialize, Serialize};

/// Measured footprint of one quantization and the memory it needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareRequirements {
    /// The GGUF quantization these numbers describe (e.g. `UD-Q2_K_XL`).
    pub quantization: String,
    /// Size of that quantization's GGUF file(s).
    pub download_bytes: u64,
    pub total_params_b: f32,
    pub active_params_b: f32,
    /// Apple Silicon: unified memory needed.
    pub min_unified_bytes: u64,
    /// CUDA: VRAM needed to hold everything on the GPU.
    pub min_vram_bytes: u64,
    /// CPU only: system RAM needed.
    pub min_ram_bytes: u64,
    /// CUDA with partial (MoE expert / n-gram embedding) offload to RAM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offload: Option<OffloadRequirement>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffloadRequirement {
    pub min_vram_bytes: u64,
    pub min_ram_bytes: u64,
}

/// How a model would run on a machine.
#[derive(Debug, Clone, PartialEq)]
pub struct RequirementFit {
    /// `fits` (>= 25% headroom over the minimum) or `tight`.
    pub fit: String,
    pub reason: String,
    /// `unified`, `gpu`, `offload` or `cpu`.
    pub path: &'static str,
}

fn gb(b: u64) -> f64 {
    b as f64 / 1e9
}

fn label(have: u64, need: u64) -> String {
    if have as f64 >= need as f64 * 1.25 { "fits" } else { "tight" }.to_string()
}

impl HardwareRequirements {
    /// Bits per parameter of the measured quantization.
    pub fn bits_per_param(&self) -> f32 {
        if self.total_params_b <= 0.0 {
            return 4.0;
        }
        (self.download_bytes as f64 * 8.0 / (self.total_params_b as f64 * 1e9)) as f32
    }

    /// The best path this machine satisfies, or why none is.
    pub fn check(&self, hw: &HardwareProfile) -> Result<RequirementFit, String> {
        let q = &self.quantization;
        if hw.unified_memory {
            if hw.ram_total_bytes >= self.min_unified_bytes {
                return Ok(RequirementFit {
                    fit: label(hw.ram_total_bytes, self.min_unified_bytes),
                    reason: format!("{q} in unified memory: needs {:.0} GB, have {:.0} GB", gb(self.min_unified_bytes), gb(hw.ram_total_bytes)),
                    path: "unified",
                });
            }
            return Err(format!("needs {:.0} GB unified memory, have {:.0} GB", gb(self.min_unified_bytes), gb(hw.ram_total_bytes)));
        }
        let vram = if hw.backends.cuda { hw.gpu_memory_total_bytes.unwrap_or(0) } else { 0 };
        if vram > 0 && vram >= self.min_vram_bytes {
            return Ok(RequirementFit {
                fit: label(vram, self.min_vram_bytes),
                reason: format!("{q} fully on the GPU: needs {:.0} GB VRAM, have {:.0} GB", gb(self.min_vram_bytes), gb(vram)),
                path: "gpu",
            });
        }
        if let Some(off) = &self.offload {
            if vram >= off.min_vram_bytes && hw.ram_total_bytes >= off.min_ram_bytes {
                return Ok(RequirementFit {
                    fit: label(hw.ram_total_bytes, off.min_ram_bytes),
                    reason: format!(
                        "{q} with offload to RAM: needs {:.0} GB VRAM + {:.0} GB RAM, have {:.0} + {:.0} GB",
                        gb(off.min_vram_bytes),
                        gb(off.min_ram_bytes),
                        gb(vram),
                        gb(hw.ram_total_bytes)
                    ),
                    path: "offload",
                });
            }
        }
        if hw.ram_total_bytes >= self.min_ram_bytes {
            return Ok(RequirementFit {
                fit: label(hw.ram_total_bytes, self.min_ram_bytes),
                reason: format!("{q} on the CPU: needs {:.0} GB RAM, have {:.0} GB (slow)", gb(self.min_ram_bytes), gb(hw.ram_total_bytes)),
                path: "cpu",
            });
        }
        let mut why = format!("needs {:.0} GB RAM (CPU) or {:.0} GB VRAM", gb(self.min_ram_bytes), gb(self.min_vram_bytes));
        if let Some(off) = &self.offload {
            why.push_str(&format!(" or {:.0} GB VRAM + {:.0} GB RAM with offload", gb(off.min_vram_bytes), gb(off.min_ram_bytes)));
        }
        why.push_str(&format!("; have {:.0} GB RAM, {:.0} GB VRAM", gb(hw.ram_total_bytes), gb(vram)));
        Err(why)
    }
}
