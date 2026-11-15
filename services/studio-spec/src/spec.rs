use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SPEC_VERSION: u32 = 1;

/// Job spec v1: one file, every engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSpec {
    pub spec_version: u32,
    pub engine: String,
    #[serde(default)]
    pub engine_version: Option<String>,
    pub base: Base,
    pub dataset: Dataset,
    /// `lora`, `qlora`, `full`, `dpo`; checked per engine in `validate`.
    pub method: String,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    pub resources: Resources,
    #[serde(default)]
    pub outputs: Outputs,
    #[serde(default)]
    pub eval: Option<Eval>,
    #[serde(default)]
    pub publish: Option<Publish>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Base {
    /// Hub repo (exclusive with `path`). Empty when `path` is set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// Absolute path (or `~/...`) to a model folder on the computer that runs
    /// the job (exclusive with `repo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub manifest_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dataset {
    /// Hub repo (exclusive with `path`).
    #[serde(default)]
    pub repo: Option<String>,
    /// Path on the computer that runs the job (exclusive with `repo`).
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    /// `jsonl`, `chat`, `alpaca`, `dpo`, ...
    pub format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Local,
    Paired,
    Compute,
    Rented,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resources {
    pub place: Place,
    #[serde(default)]
    pub gpu: Option<String>,
    #[serde(default)]
    pub max_hours: Option<f64>,
    #[serde(default)]
    pub max_usd: Option<f64>,
    /// `auto` (default), `torch` or `mlx`. On a Mac, auto trains LoRA and
    /// QLoRA with MLX and full/DPO with PyTorch (MPS). `mlx` anywhere else is
    /// refused (`backend_not_supported`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendChoice>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    #[default]
    Auto,
    Torch,
    Mlx,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Outputs {
    #[serde(default)]
    pub adapter: bool,
    #[serde(default)]
    pub merged: bool,
    /// Quant names, e.g. `q4_k_m`.
    #[serde(default)]
    pub gguf: Vec<String>,
    /// MLX files: a Mac with Apple Silicon only (`output_not_supported` elsewhere).
    #[serde(default)]
    pub mlx: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Eval {
    #[serde(default)]
    pub sets: Vec<String>,
    #[serde(default)]
    pub baseline: Option<String>,
}

/// Present only so it can be refused: publishing is not available in this phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Publish {
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub card: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ValidationError {
    /// Stable machine code: `rented_not_available`, `publish_not_available`,
    /// `unknown_engine`, `unknown_method`, `unsupported_spec_version`,
    /// `invalid_base`, `invalid_dataset`, `invalid_spec`. Hardware checks
    /// (`crate::fit::check_job`) add `hardware_not_supported`,
    /// `method_not_supported`, `backend_not_supported`, `output_not_supported`.
    pub code: String,
    pub message: String,
}

impl ValidationError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

impl JobSpec {
    /// Parse JSON and validate. Returns every problem found; a JSON syntax or
    /// shape error comes back as one `invalid_spec` entry.
    pub fn parse(json: &str) -> Result<JobSpec, Vec<ValidationError>> {
        let spec: JobSpec = serde_json::from_str(json)
            .map_err(|e| vec![ValidationError::new("invalid_spec", e.to_string())])?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errs = Vec::new();
        if self.spec_version != SPEC_VERSION {
            errs.push(ValidationError::new(
                "unsupported_spec_version",
                format!("spec_version {} is not supported (expected {SPEC_VERSION})", self.spec_version),
            ));
        }
        match crate::engines::find(&self.engine) {
            None => errs.push(ValidationError::new(
                "unknown_engine",
                format!("engine '{}' is not in the catalog", self.engine),
            )),
            Some(e) => {
                if !e.methods.iter().any(|m| m == &self.method) {
                    errs.push(ValidationError::new(
                        "unknown_method",
                        format!("engine '{}' does not support method '{}'", self.engine, self.method),
                    ));
                }
            }
        }
        if self.base.repo.is_empty() == self.base.path.is_none() {
            errs.push(ValidationError::new(
                "invalid_base",
                "base needs exactly one of repo or path",
            ));
        }
        if self.dataset.repo.is_some() == self.dataset.path.is_some() {
            errs.push(ValidationError::new(
                "invalid_dataset",
                "dataset needs exactly one of repo or path",
            ));
        }
        if self.resources.place == Place::Rented {
            errs.push(ValidationError::new(
                "rented_not_available",
                "rented GPUs are not available yet",
            ));
        }
        if self.publish.is_some() {
            errs.push(ValidationError::new(
                "publish_not_available",
                "publishing from a job is not available yet",
            ));
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}
