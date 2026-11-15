//! Engines on this computer: install state, hardware fit, how to launch one,
//! and the install task (Decision 4: engines run as venv processes).
//!
//! # Launch contract (engine side)
//!
//! Allternit Tune (`services/model-training`, PR #2379) is a source folder
//! with a `requirements.txt`, not a pip package. The host finds the folder
//! ([`tune_source`]), installs its requirements into
//! `<data dir>/studio/engines/<engine id>/venv` and writes
//! `.allternit-installed` holding the sha256 of the requirements file it
//! installed: `requirements-<hardware id>.txt` when the folder has one for
//! this computer's hardware, else `requirements.txt`. The venv is made with
//! `ALLTERNIT_STUDIO_PYTHON` when set (Desktop's bundled CPython), else the
//! first Python 3.10+ on `PATH`. The engine counts as installed while the
//! venv python exists and that hash matches the current file. The host then starts
//!
//! ```text
//! <venv>/bin/python -m allternit_tune.server --port <port> --data-dir <job dir>/engine
//! ```
//!
//! with the folder on `PYTHONPATH` and these environment variables, then
//! speaks engine API v1 (`services/studio-spec/openapi/engine-api.yaml`):
//!
//! - `ALLTERNIT_TUNE_TOKEN` and `ALLTERNIT_ENGINE_TOKEN`: the per-launch
//!   bearer token (env, not argv, so it never shows in `ps`)
//! - `ALLTERNIT_ENGINE_PORT`: same as `--port`
//! - `ALLTERNIT_STUDIO_JOB_DIR`, `ALLTERNIT_STUDIO_OUTPUT_DIR`: the job folder
//!   and its outputs folder, both absolute; the engine reports absolute output
//!   paths inside the outputs folder (relative ones are still read against it)
//! - `ALLTERNIT_STUDIO_HARDWARE`: the hardware id the host detected
//!   ([`super::hardware`]); the engine does not probe
//! - `ALLTERNIT_STUDIO_PYTHON`: passed through when the host has it set
//!
//! The job spec the host posts has `base.repo` replaced by the local folder the
//! host fetched (Decision 5), or `base.path` expanded to an absolute folder for
//! a local base, and `dataset.path` set to an absolute local path.
//!
//! Developer override (also used by the smoke test with the contract crate's
//! fake engine): `ALLTERNIT_STUDIO_ENGINE_CMD_<ID>` (id upper-cased, `-` to
//! `_`, e.g. `ALLTERNIT_STUDIO_ENGINE_CMD_ALLTERNIT_TUNE`) is a whitespace
//! separated command line in which `{port}` and `{token}` are replaced. When
//! set, the engine counts as installed.

use allternit_studio_spec::{EngineInfo, Fit, Hardware, InstallState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Engines this build can install and run.
const AVAILABLE: &[(&str, &str)] = &[("allternit-tune", "allternit_tune.server")];

pub fn engine_module(engine_id: &str) -> Option<&'static str> {
    AVAILABLE
        .iter()
        .find(|(id, _)| *id == engine_id)
        .map(|(_, m)| *m)
}

/// `GET /engines` row: the catalog entry plus what this computer says about it.
#[derive(Debug, Clone, Serialize)]
pub struct EngineView {
    #[serde(flatten)]
    pub info: EngineInfo,
    pub fit: Fit,
    /// false = this build cannot install the engine (POST install returns 501).
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_message: Option<String>,
}

/// Last install attempt, `<engines>/<id>/install.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallRecord {
    pub state: Option<InstallState>,
    pub message: Option<String>,
}

pub fn override_env_name(engine_id: &str) -> String {
    format!(
        "ALLTERNIT_STUDIO_ENGINE_CMD_{}",
        engine_id.to_ascii_uppercase().replace('-', "_")
    )
}

pub fn override_command(engine_id: &str) -> Option<String> {
    std::env::var(override_env_name(engine_id))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

pub struct EngineDirs {
    root: PathBuf,
    hardware: Hardware,
}

impl EngineDirs {
    pub fn new(studio_dir: &Path, hardware: Hardware) -> Self {
        Self {
            root: studio_dir.join("engines"),
            hardware,
        }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn venv(&self, id: &str) -> PathBuf {
        self.dir(id).join("venv")
    }

    pub fn python(&self, id: &str) -> PathBuf {
        let venv = self.venv(id);
        if cfg!(windows) {
            venv.join("Scripts").join("python.exe")
        } else {
            venv.join("bin").join("python")
        }
    }

    fn marker(&self, id: &str) -> PathBuf {
        self.venv(id).join(".allternit-installed")
    }

    /// Venv python present and installed from the current requirements.
    pub fn venv_installed(&self, id: &str) -> bool {
        if !self.python(id).exists() {
            return false;
        }
        let Ok(marker) = std::fs::read_to_string(self.marker(id)) else {
            return false;
        };
        match tune_source().and_then(|src| requirements_hash(&requirements_file(&src, self.hardware))) {
            Some(current) => marker.trim() == current,
            // No source folder means the engine cannot start either.
            None => false,
        }
    }

    pub fn read_install(&self, id: &str) -> InstallRecord {
        std::fs::read(self.dir(id).join("install.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn write_install(&self, id: &str, rec: &InstallRecord) {
        let dir = self.dir(id);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(b) = serde_json::to_vec_pretty(rec) {
            let _ = std::fs::write(dir.join("install.json"), b);
        }
    }

    /// Create the venv and pip-install the engine's requirements. Returns a
    /// plain message on failure.
    pub async fn install(&self, id: &str) -> Result<(), String> {
        if id != "allternit-tune" {
            return Err(format!("{id} cannot be installed by this build."));
        }
        let source = tune_source().ok_or_else(|| {
            "This build does not include the Allternit Tune engine files. \
             Set ALLTERNIT_TUNE_SOURCE to its folder and try again."
                .to_string()
        })?;
        let requirements = requirements_file(&source, self.hardware);
        let hash = requirements_hash(&requirements).ok_or_else(|| {
            format!("The engine's {} could not be read.", requirements.display())
        })?;
        let python = find_python().await.ok_or_else(|| {
            "Python 3.10 or newer is needed to install a training engine, and none was found on this computer."
                .to_string()
        })?;
        let venv = self.venv(id);
        if let Some(parent) = venv.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        run(
            &python,
            &["-m", "venv", &venv.to_string_lossy()],
            "create the engine's Python environment",
        )
        .await?;
        let py = self.python(id);
        run(
            &py,
            &["-m", "pip", "install", "--upgrade", "pip"],
            "update pip",
        )
        .await?;
        run(
            &py,
            &[
                "-m",
                "pip",
                "install",
                "-r",
                &requirements.to_string_lossy(),
            ],
            "install the engine's packages",
        )
        .await?;
        std::fs::write(self.marker(id), format!("{hash}\n"))
            .map_err(|e| format!("Could not finish the install: {e}"))?;
        Ok(())
    }
}

/// The Allternit Tune source folder (`services/model-training`, source only).
///
/// 1. `ALLTERNIT_TUNE_SOURCE` when set and non-empty (Desktop sets it to the
///    active runtime package's `model-training/`, else the app's bundled
///    `Resources/model-training`; see backend-manager.ts).
/// 2. Next to the binary: `<exe dir>/model-training`, or one level up
///    (`Resources/bin/allternit-api` → `Resources/model-training` in the app
///    bundle; `versions/<v>/bin/allternit-api` → `versions/<v>/model-training`
///    in a runtime package), or `<exe dir>/../resources/model-training`.
/// 3. Dev and `cargo run`: the repo copy this binary was built from.
pub fn tune_source() -> Option<PathBuf> {
    let env = std::env::var_os("ALLTERNIT_TUNE_SOURCE").filter(|v| !v.is_empty());
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    tune_source_from(env.map(PathBuf::from), exe_dir.as_deref())
}

fn is_tune_source(p: &Path) -> bool {
    p.join("allternit_tune").is_dir() && p.join("requirements.txt").is_file()
}

fn tune_source_from(env: Option<PathBuf>, exe_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = env {
        // An explicit folder is the answer, right or wrong: never silently use another copy.
        return is_tune_source(&p).then_some(p);
    }
    let repo_copy = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("services")
        .join("model-training");
    exe_dir
        .into_iter()
        .flat_map(|d| {
            [
                d.join("model-training"),
                d.join("..").join("model-training"),
                d.join("..").join("resources").join("model-training"),
            ]
        })
        .chain(std::iter::once(repo_copy))
        .find(|p| is_tune_source(p))
}

/// `requirements-<hardware id>.txt` when the engine folder has one for this
/// hardware (torch wheel index, mlx-lm on a Mac), else `requirements.txt`.
pub fn requirements_file(source: &Path, hw: Hardware) -> PathBuf {
    let specific = source.join(format!("requirements-{}.txt", hw.id()));
    if specific.is_file() {
        specific
    } else {
        source.join("requirements.txt")
    }
}

fn requirements_hash(file: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(file).ok()?;
    Some(hex::encode(Sha256::digest(bytes)))
}

/// The host's `ALLTERNIT_STUDIO_PYTHON` (Desktop's bundled CPython), if set.
pub fn studio_python() -> Option<String> {
    std::env::var("ALLTERNIT_STUDIO_PYTHON")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

async fn find_python() -> Option<String> {
    if let Some(p) = studio_python() {
        // An explicit Python is the answer, right or wrong: never swap in another.
        return python_ok(&p).await.then_some(p);
    }
    for cand in [
        "python3.12",
        "python3.11",
        "python3.10",
        "python3",
        "python",
    ] {
        if python_ok(cand).await {
            return Some(cand.to_string());
        }
    }
    None
}

async fn python_ok(program: &str) -> bool {
    let out = tokio::process::Command::new(program)
        .args(["-c", "import sys; print(sys.version_info >= (3, 10))"])
        .output()
        .await;
    matches!(out, Ok(out) if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "True")
}

async fn run(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
    what: &str,
) -> Result<(), String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Could not {what}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    Err(format!("Could not {what}. {tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_refuses_with_reason() {
        use allternit_studio_spec::{fit, HostHardware};
        let tune = allternit_studio_spec::engines::find("allternit-tune").unwrap();
        let unsloth = allternit_studio_spec::engines::find("unsloth-upstream").unwrap();

        let mac = fit(&tune, &HostHardware::plain(Hardware::AppleSilicon));
        assert!(mac.ok);
        assert!(mac.methods_unavailable.is_empty());

        let mac_unsloth = fit(&unsloth, &HostHardware::plain(Hardware::AppleSilicon));
        assert!(!mac_unsloth.ok);
        assert!(!mac_unsloth.reason.is_empty());

        // CPU is a real device: allternit-tune runs, LoRA only, with a warning.
        let cpu = fit(&tune, &crate::studio::hardware::classify_probe("linux", "x86_64", &Default::default()));
        assert!(cpu.ok, "{}", cpu.reason);
        assert_eq!(cpu.hardware, Hardware::Cpu);
        assert!(cpu.warning.is_some());
        assert!(cpu.methods_unavailable.contains_key("qlora"));
        assert!(!cpu.methods_unavailable.contains_key("lora"));
    }

    #[test]
    fn requirements_file_prefers_the_hardware_one() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("requirements.txt"), "torch\n").unwrap();
        std::fs::write(tmp.path().join("requirements-apple-silicon.txt"), "mlx-lm\n").unwrap();
        assert!(requirements_file(tmp.path(), Hardware::AppleSilicon).ends_with("requirements-apple-silicon.txt"));
        assert!(requirements_file(tmp.path(), Hardware::CudaLinux).ends_with("requirements.txt"));
    }

    #[test]
    fn tune_source_finds_bundled_then_repo_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let make = |dir: &Path| {
            std::fs::create_dir_all(dir.join("allternit_tune")).unwrap();
            std::fs::write(dir.join("requirements.txt"), "torch\n").unwrap();
        };
        // Packaged app: Resources/bin/allternit-api finds Resources/model-training.
        let resources = tmp.path().join("Resources");
        let bin = resources.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        make(&resources.join("model-training"));
        let found = tune_source_from(None, Some(&bin)).unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            resources.join("model-training").canonicalize().unwrap()
        );

        // Dev: no bundled copy next to the binary, so the repo's own folder.
        let elsewhere = tmp.path().join("target").join("debug");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let dev = tune_source_from(None, Some(&elsewhere)).unwrap();
        assert!(
            dev.ends_with("services/model-training"),
            "{}",
            dev.display()
        );
        assert!(is_tune_source(&dev));

        // An explicit folder wins and is never swapped for another copy.
        let explicit = tmp.path().join("custom");
        make(&explicit);
        assert_eq!(
            tune_source_from(Some(explicit.clone()), Some(&bin)),
            Some(explicit)
        );
        assert_eq!(
            tune_source_from(Some(tmp.path().join("nope")), Some(&bin)),
            None
        );
    }
}
