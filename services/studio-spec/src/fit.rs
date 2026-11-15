//! Can an engine train on this computer: per engine, per method, per output.
//!
//! The host puts [`Fit`] on every `GET /engines` row (`fit`) and refuses a job
//! with [`check_job`]. Engines run the same check on the hardware id the host
//! hands them at launch (`ALLTERNIT_STUDIO_HARDWARE`). `fixtures/fit-table.json`
//! pins the results; Rust and Python tests both read it.

use crate::engines::EngineInfo;
use crate::hardware::{Hardware, HostHardware};
use crate::spec::{BackendChoice, JobSpec, ValidationError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The `fit` object on an engine card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fit {
    /// false = the engine does not run on this computer at all
    /// (`hardware_not_supported`).
    pub ok: bool,
    pub hardware: Hardware,
    /// Backend the engine's first available method uses, e.g. `mlx`,
    /// `torch-mps`, `torch-cuda`, `torch-rocm`, `torch-xpu`, `torch-cpu`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Plain words, shown to the person as is.
    pub reason: String,
    /// Shown before start when the job can run but slowly (the CPU).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Methods the engine has but this computer cannot run, with the reason
    /// (`method_not_supported`).
    #[serde(default)]
    pub methods_unavailable: BTreeMap<String, String>,
    /// Outputs this engine cannot make here, with the reason
    /// (`output_not_supported`). Keys: `adapter`, `merged`, `gguf`, `mlx`.
    #[serde(default)]
    pub outputs_unavailable: BTreeMap<String, String>,
}

/// What [`check_job`] picked for an accepted job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selected {
    pub hardware: Hardware,
    pub backend: String,
}

pub const CPU_WARNING: &str =
    "Training on the CPU is slow. Use a small base model; even a small LoRA run can take hours.";

fn method_name(m: &str) -> &str {
    match m {
        "lora" => "LoRA",
        "qlora" => "QLoRA",
        "full" => "full fine-tuning",
        "dpo" => "DPO",
        other => other,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// "a, b or c" / "a and b".
fn join(items: &[&str], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., tail] => format!("{} {last} {tail}", rest.join(", ")),
    }
}

/// Engines whose MLX backend is available (allternit-tune).
fn has_mlx(engine: &EngineInfo) -> bool {
    engine.backends.iter().any(|b| b.id == "mlx" && b.status == "available")
}

/// Methods the engine's MLX backend does not train (`limits.mlx_unsupported_methods`).
fn mlx_unsupported(engine: &EngineInfo) -> Vec<String> {
    engine
        .limits
        .get("mlx_unsupported_methods")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn runs_on(engine: &EngineInfo, hw: Hardware) -> bool {
    engine.hardware.iter().any(|h| h == hw.id())
}

/// Why the whole engine is refused on `hw`, or None when it runs there.
pub fn engine_refusal(engine: &EngineInfo, hw: Hardware) -> Option<String> {
    if runs_on(engine, hw) {
        return None;
    }
    let labels: Vec<&str> = Hardware::ALL
        .into_iter()
        .filter(|h| runs_on(engine, *h))
        .map(Hardware::label)
        .collect();
    Some(format!(
        "{} runs only on {}, not on {}.",
        engine.name,
        join(&labels, "or"),
        hw.label()
    ))
}

/// Why `method` cannot run with this engine on `hw`, or None when it can.
/// Assumes the engine itself runs on `hw` and lists the method.
pub fn method_refusal(_engine: &EngineInfo, hw: Hardware, method: &str) -> Option<String> {
    if hw == Hardware::Cpu && method != "lora" {
        return Some(format!(
            "{} needs a GPU. Only LoRA runs on the CPU.",
            capitalize(method_name(method))
        ));
    }
    None
}

/// The backend a method runs with, given the spec's `resources.backend`.
/// Err = `backend_not_supported` with the reason.
pub fn backend_for(
    engine: &EngineInfo,
    hw: Hardware,
    method: &str,
    choice: BackendChoice,
) -> Result<String, String> {
    let mlx_here = hw == Hardware::AppleSilicon && has_mlx(engine);
    let mlx_method = !mlx_unsupported(engine).iter().any(|m| m == method);
    match choice {
        BackendChoice::Mlx if hw != Hardware::AppleSilicon => {
            Err("MLX runs only on a Mac with Apple Silicon.".into())
        }
        BackendChoice::Mlx if !mlx_here => Err(format!("{} has no MLX backend.", engine.name)),
        BackendChoice::Mlx if !mlx_method => Err(format!(
            "MLX does not run {} here. Pick the automatic or PyTorch backend.",
            method_name(method)
        )),
        BackendChoice::Mlx => Ok("mlx".into()),
        BackendChoice::Auto if mlx_here && mlx_method => Ok("mlx".into()),
        BackendChoice::Auto | BackendChoice::Torch => Ok(hw.torch_backend().into()),
    }
}

/// Why this engine cannot make `output` on `hw`, or None when it can.
pub fn output_refusal(engine: &EngineInfo, hw: Hardware, output: &str) -> Option<String> {
    if output != "mlx" {
        return None;
    }
    if hw != Hardware::AppleSilicon {
        return Some("MLX files are made only on a Mac with Apple Silicon.".into());
    }
    if !has_mlx(engine) {
        return Some(format!("{} cannot make MLX files. Allternit Tune can.", engine.name));
    }
    None
}

fn ok_reason(engine: &EngineInfo, hw: Hardware) -> String {
    match hw {
        Hardware::AppleSilicon if has_mlx(engine) => {
            let off = mlx_unsupported(engine);
            let (mlx, torch): (Vec<&str>, Vec<&str>) = engine
                .methods
                .iter()
                .map(String::as_str)
                .partition(|m| !off.iter().any(|o| o == m));
            let mlx: Vec<&str> = mlx.into_iter().map(method_name).collect();
            let torch: Vec<&str> = torch.into_iter().map(method_name).collect();
            if torch.is_empty() {
                format!("Runs on this Mac's GPU with MLX ({}).", join(&mlx, "and"))
            } else {
                format!(
                    "Runs on this Mac's GPU: {} with MLX, {} with PyTorch (MPS).",
                    join(&mlx, "and"),
                    join(&torch, "and")
                )
            }
        }
        Hardware::AppleSilicon => "Runs on this Mac's GPU with PyTorch (MPS).".into(),
        Hardware::CudaLinux | Hardware::CudaWindows => "Runs on this computer's NVIDIA GPU.".into(),
        Hardware::RocmLinux => "Runs on this computer's AMD GPU (ROCm).".into(),
        Hardware::XpuLinux | Hardware::XpuWindows => "Runs on this computer's Intel GPU (XPU).".into(),
        Hardware::Cpu => "Runs on this computer's CPU.".into(),
    }
}

/// The `fit` object for one engine on this computer.
pub fn fit(engine: &EngineInfo, host: &HostHardware) -> Fit {
    let hw = host.hardware;
    let with_host = |r: String| match &host.reason {
        Some(why) => format!("{r} {why}"),
        None => r,
    };
    let uses_torch = engine.backends.iter().any(|b| b.id == "torch");
    if let (true, Some(r)) = (uses_torch, &host.torch_unavailable) {
        return Fit {
            ok: false,
            hardware: hw,
            backend: None,
            reason: r.clone(),
            warning: None,
            methods_unavailable: BTreeMap::new(),
            outputs_unavailable: BTreeMap::new(),
        };
    }
    if let Some(r) = engine_refusal(engine, hw) {
        return Fit {
            ok: false,
            hardware: hw,
            backend: None,
            reason: with_host(r),
            warning: None,
            methods_unavailable: BTreeMap::new(),
            outputs_unavailable: BTreeMap::new(),
        };
    }
    let mut methods_unavailable = BTreeMap::new();
    let mut backend = None;
    for m in &engine.methods {
        match method_refusal(engine, hw, m) {
            Some(r) => {
                methods_unavailable.insert(m.clone(), r);
            }
            None if backend.is_none() => {
                backend = backend_for(engine, hw, m, BackendChoice::Auto).ok();
            }
            None => {}
        }
    }
    let outputs_unavailable = ["adapter", "merged", "gguf", "mlx"]
        .into_iter()
        .filter_map(|o| output_refusal(engine, hw, o).map(|r| (o.to_string(), r)))
        .collect();
    Fit {
        ok: true,
        hardware: hw,
        backend,
        reason: with_host(ok_reason(engine, hw)),
        warning: (hw == Hardware::Cpu).then(|| CPU_WARNING.to_string()),
        methods_unavailable,
        outputs_unavailable,
    }
}

fn refusal(code: &str, message: String) -> ValidationError {
    ValidationError { code: code.into(), message }
}

/// Check a job against this computer, after `JobSpec::validate` passed.
/// Codes: `hardware_not_supported` (the engine does not run here),
/// `method_not_supported`, `backend_not_supported`, `output_not_supported`.
/// Messages are the reasons from [`fit`], shown as is.
pub fn check_job(spec: &JobSpec, host: &HostHardware) -> Result<Selected, Vec<ValidationError>> {
    let Some(engine) = crate::engines::find(&spec.engine) else {
        return Err(vec![refusal(
            "unknown_engine",
            format!("engine '{}' is not in the catalog", spec.engine),
        )]);
    };
    let card = fit(&engine, host);
    if !card.ok {
        return Err(vec![refusal("hardware_not_supported", card.reason)]);
    }
    let hw = host.hardware;
    let mut errs = Vec::new();
    if let Some(r) = card.methods_unavailable.get(&spec.method) {
        errs.push(refusal("method_not_supported", r.clone()));
    }
    let backend = match backend_for(&engine, hw, &spec.method, spec.resources.backend.unwrap_or_default()) {
        Ok(b) => Some(b),
        Err(r) => {
            errs.push(refusal("backend_not_supported", r));
            None
        }
    };
    let o = &spec.outputs;
    let asked = [("adapter", o.adapter), ("merged", o.merged), ("gguf", !o.gguf.is_empty()), ("mlx", o.mlx)];
    for (name, on) in asked {
        if let (true, Some(r)) = (on, card.outputs_unavailable.get(name)) {
            errs.push(refusal("output_not_supported", r.clone()));
        }
    }
    match (errs.is_empty(), backend) {
        (true, Some(backend)) => Ok(Selected { hardware: hw, backend }),
        _ => Err(errs),
    }
}
