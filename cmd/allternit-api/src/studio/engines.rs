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
//!
//! # Third-party engines (studio-engines kit, Decision 9)
//!
//! Axolotl, LLaMA-Factory and torchtune run through the
//! public engine kit at <https://github.com/Allternit/studio-engines>, pinned to
//! [`STUDIO_ENGINES_SHA`]. Each engine gets its own venv
//! (`<engines>/<id>/venv`, Python at least [`KitEngine::min_python`]) holding:
//!
//! 1. the kit, `studio-engines @ <repo>/archive/<sha>.tar.gz` (standard
//!    library only; an archive URL so the computer needs no `git`);
//! 2. the upstream package, unmodified, at the `pins.json` version: first the
//!    wheel alone, hash-checked against the `pins.json` wheel sha256
//!    (`pip install --no-deps --only-binary :all: --require-hashes -r
//!    <engines>/<id>/upstream-pin.txt`), then its dependencies resolved by
//!    version (`pip install <package>==<version>`, with this hardware's
//!    PyTorch index; torchtune also gets `torch` and `torchao`, which it
//!    needs but does not declare). Only the upstream wheel is hash-pinned.
//!
//! `.allternit-installed` holds the sha256 of (kit sha, package, version,
//! wheel sha256, hardware id), so a pin change or a different computer
//! reinstalls. The host starts
//!
//! ```text
//! <venv>/bin/python -m studio_engine_kit --engine <id> --port <port> --data-dir <outputs dir>
//! ```
//!
//! with the same environment as above plus `STUDIO_ENGINE_TOKEN` (the token)
//! and `STUDIO_ENGINE_BIN_<ID>` (the upstream CLI in that venv). The kit runs
//! the upstream CLI as its own process; no Allternit code imports an engine.
//! No engine may be AGPL or LGPL, even behind the process boundary, which is
//! why Unsloth is not an engine (allternit-tune covers LoRA, QLoRA and GGUF).

use allternit_studio_spec::{EngineInfo, Fit, Hardware, InstallState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Commit of <https://github.com/Allternit/studio-engines> the kit engines
/// install from. The only place the pin lives: the install marker includes it,
/// so changing it reinstalls every kit engine.
pub const STUDIO_ENGINES_SHA: &str = "e520474f67ec93888fcd07716f313ad5dbda3db9";
pub const STUDIO_ENGINES_REPO: &str = "https://github.com/Allternit/studio-engines";

const TUNE_MODULE: &str = "allternit_tune.server";
const KIT_MODULE: &str = "studio_engine_kit";

/// A third-party engine run by the studio-engines kit. Package, version and
/// wheel sha256 are copied from studio-engines `pins.json` at
/// [`STUDIO_ENGINES_SHA`]; the version must match `engines.json`.
#[derive(Debug)]
pub struct KitEngine {
    pub id: &'static str,
    pub package: &'static str,
    pub version: &'static str,
    pub wheel_sha256: &'static str,
    /// The upstream CLI the kit runs, in the engine's venv.
    pub cli: &'static str,
    /// Lowest Python 3 minor version the upstream package supports (and at
    /// least 10, the host's floor).
    pub min_python: u32,
    /// Packages the upstream wheel needs but does not declare.
    pub extra: &'static [&'static str],
}

pub const KIT_ENGINES: &[KitEngine] = &[
    KitEngine {
        id: "axolotl",
        package: "axolotl",
        version: "0.21.1",
        wheel_sha256: "3c0af0b1e381617c0df7c97a456ebccb4f1746116e5419c6601d9d15507c10d2",
        cli: "axolotl",
        min_python: 12,
        extra: &[],
    },
    KitEngine {
        id: "llama-factory",
        package: "llamafactory",
        version: "0.9.5",
        wheel_sha256: "10776e9b259798bf65f6c5343f6298f0302e92e9cd47472abe29eef69e286c6a",
        cli: "llamafactory-cli",
        min_python: 11,
        extra: &[],
    },
    KitEngine {
        id: "torchtune",
        package: "torchtune",
        version: "0.6.1",
        wheel_sha256: "bcbc4ee9774b5c5829a5ff13df851e0b0e32416c018af8db53faa3472ac256e7",
        cli: "tune",
        min_python: 10,
        // torchtune leaves PyTorch to the user and refuses to import without torchao.
        extra: &["torch", "torchao"],
    },
];

pub fn kit_engine(engine_id: &str) -> Option<&'static KitEngine> {
    KIT_ENGINES.iter().find(|k| k.id == engine_id)
}

/// The Python module that starts the engine, for engines this build can
/// install and run; `None` = not available in this build.
pub fn engine_module(engine_id: &str) -> Option<&'static str> {
    if engine_id == "allternit-tune" {
        Some(TUNE_MODULE)
    } else {
        kit_engine(engine_id).map(|_| KIT_MODULE)
    }
}

/// Arguments after `<venv python>` that start an engine (module docs).
pub fn launch_args(engine_id: &str, port: u16, job_dir: &Path, output_dir: &Path) -> Option<Vec<String>> {
    if engine_id == "allternit-tune" {
        return Some(vec![
            "-m".into(),
            TUNE_MODULE.into(),
            "--port".into(),
            port.to_string(),
            "--data-dir".into(),
            job_dir.join("engine").to_string_lossy().into_owned(),
        ]);
    }
    let k = kit_engine(engine_id)?;
    Some(vec![
        "-m".into(),
        KIT_MODULE.into(),
        "--engine".into(),
        k.id.into(),
        "--port".into(),
        port.to_string(),
        "--data-dir".into(),
        output_dir.to_string_lossy().into_owned(),
    ])
}

/// Environment a kit engine needs on top of the shared launch env: the token
/// under the kit's name and the upstream CLI next to `venv_python`.
pub fn kit_launch_env(engine_id: &str, venv_python: &Path, token: &str) -> Vec<(String, std::ffi::OsString)> {
    let Some(k) = kit_engine(engine_id) else {
        return Vec::new();
    };
    let cli = if cfg!(windows) { format!("{}.exe", k.cli) } else { k.cli.to_string() };
    vec![
        ("STUDIO_ENGINE_TOKEN".into(), token.into()),
        (
            format!("STUDIO_ENGINE_BIN_{}", k.id.to_ascii_uppercase().replace('-', "_")),
            venv_python.with_file_name(cli).into_os_string(),
        ),
    ]
}

/// The pip requirement that installs the kit at [`STUDIO_ENGINES_SHA`].
pub fn kit_requirement(sha: &str) -> String {
    format!("studio-engines @ {STUDIO_ENGINES_REPO}/archive/{sha}.tar.gz")
}

/// The hash-checked line for the upstream wheel (`upstream-pin.txt`).
pub fn upstream_pin_line(k: &KitEngine) -> String {
    format!("{}=={} --hash=sha256:{}\n", k.package, k.version, k.wheel_sha256)
}

/// PyTorch wheel index for this hardware, the same one
/// `services/model-training/requirements-<hardware>.txt` uses.
pub fn torch_index(hw: Hardware) -> Option<&'static str> {
    match hw {
        Hardware::AppleSilicon => None,
        Hardware::CudaLinux | Hardware::CudaWindows => Some("https://download.pytorch.org/whl/cu134"),
        Hardware::RocmLinux => Some("https://download.pytorch.org/whl/rocm7.2"),
        Hardware::XpuLinux | Hardware::XpuWindows => Some("https://download.pytorch.org/whl/xpu"),
        Hardware::Cpu => Some("https://download.pytorch.org/whl/cpu"),
    }
}

/// What `.allternit-installed` holds for a kit engine: changes with the kit
/// pin, the upstream pin, or the hardware.
pub fn kit_marker(sha: &str, k: &KitEngine, hw: Hardware) -> String {
    use sha2::{Digest, Sha256};
    let key = format!(
        "studio-engines@{sha}\n{}=={}\nsha256:{}\n{}\n",
        k.package,
        k.version,
        k.wheel_sha256,
        hw.id()
    );
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// Runs one install command. The real one spawns the process; tests pass a
/// fake that records the command line.
#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, program: &std::ffi::OsStr, args: &[String], what: &str) -> Result<(), String>;
}

pub struct ProcessRunner;

#[async_trait::async_trait]
impl CommandRunner for ProcessRunner {
    async fn run(&self, program: &std::ffi::OsStr, args: &[String], what: &str) -> Result<(), String> {
        run(program, args, what).await
    }
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

    /// What the marker must hold for the engine to count as installed.
    fn expected_marker(&self, id: &str) -> Option<String> {
        if id == "allternit-tune" {
            // No source folder means the engine cannot start either.
            tune_source().and_then(|src| requirements_hash(&requirements_file(&src, self.hardware)))
        } else {
            kit_engine(id).map(|k| kit_marker(STUDIO_ENGINES_SHA, k, self.hardware))
        }
    }

    /// Venv python present and installed from the current pins/requirements.
    pub fn venv_installed(&self, id: &str) -> bool {
        if !self.python(id).exists() {
            return false;
        }
        let Ok(marker) = std::fs::read_to_string(self.marker(id)) else {
            return false;
        };
        self.expected_marker(id).is_some_and(|current| marker.trim() == current)
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

    /// Create the venv and pip-install the engine. Returns a plain message on failure.
    pub async fn install(&self, id: &str) -> Result<(), String> {
        let min_python = match kit_engine(id) {
            Some(k) => k.min_python,
            None if id == "allternit-tune" => 10,
            None => return Err(format!("{id} cannot be installed by this build.")),
        };
        let python = find_python(min_python).await.ok_or_else(|| {
            format!(
                "Python 3.{min_python} or newer is needed to install this training engine, and none was found on this computer."
            )
        })?;
        self.install_with(id, &python, &ProcessRunner).await
    }

    /// [`Self::install`] with the base Python chosen and commands run by `runner`.
    pub async fn install_with(
        &self,
        id: &str,
        python: &str,
        runner: &dyn CommandRunner,
    ) -> Result<(), String> {
        // Work out the marker first: a missing engine folder fails before any venv is made.
        let (marker, kit, requirements) = match kit_engine(id) {
            Some(k) => (kit_marker(STUDIO_ENGINES_SHA, k, self.hardware), Some(k), None),
            None if id == "allternit-tune" => {
                let source = tune_source().ok_or_else(|| {
                    "This build does not include the Allternit Tune engine files. \
                     Set ALLTERNIT_TUNE_SOURCE to its folder and try again."
                        .to_string()
                })?;
                let requirements = requirements_file(&source, self.hardware);
                let hash = requirements_hash(&requirements).ok_or_else(|| {
                    format!("The engine's {} could not be read.", requirements.display())
                })?;
                (hash, None, Some(requirements))
            }
            None => return Err(format!("{id} cannot be installed by this build.")),
        };
        let dir = self.dir(id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        // A failed reinstall must never count as installed.
        let _ = std::fs::remove_file(self.marker(id));
        self.make_venv(id, python, runner).await?;
        let py = self.python(id);
        let py = py.as_os_str();
        let pip = |rest: &[&str]| -> Vec<String> {
            ["-m", "pip", "install"].iter().chain(rest).map(|s| s.to_string()).collect()
        };
        match kit {
            None => {
                let requirements = requirements.unwrap_or_default();
                runner
                    .run(py, &pip(&["-r", &requirements.to_string_lossy()]), "install the engine's packages")
                    .await?;
            }
            Some(k) => {
                runner
                    .run(py, &pip(&[&kit_requirement(STUDIO_ENGINES_SHA)]), "install the Studio engine kit")
                    .await?;
                let pin_file = dir.join("upstream-pin.txt");
                std::fs::write(&pin_file, upstream_pin_line(k))
                    .map_err(|e| format!("Could not write {}: {e}", pin_file.display()))?;
                runner
                    .run(
                        py,
                        &pip(&[
                            "--no-deps",
                            "--only-binary",
                            ":all:",
                            "--require-hashes",
                            "-r",
                            &pin_file.to_string_lossy(),
                        ]),
                        &format!("install {} {} (hash-checked)", k.package, k.version),
                    )
                    .await?;
                let spec = format!("{}=={}", k.package, k.version);
                let mut rest: Vec<&str> = Vec::new();
                if let Some(index) = torch_index(self.hardware) {
                    rest.extend(["--extra-index-url", index]);
                }
                rest.push(&spec);
                rest.extend(k.extra);
                runner
                    .run(py, &pip(&rest), &format!("install the packages {} needs", k.package))
                    .await?;
            }
        }
        let _ = std::fs::create_dir_all(self.venv(id));
        std::fs::write(self.marker(id), format!("{marker}\n"))
            .map_err(|e| format!("Could not finish the install: {e}"))?;
        Ok(())
    }

    /// Make the venv with an up-to-date pip. A normal `-m venv` when the base
    /// Python has `ensurepip`; otherwise (Desktop's bundled python-build-standalone
    /// strips it) or when that fails, `-m venv --without-pip` and pip installed
    /// into it by the base Python's own pip (`pip --python <venv python>`).
    async fn make_venv(&self, id: &str, python: &str, runner: &dyn CommandRunner) -> Result<(), String> {
        let base = std::ffi::OsStr::new(python);
        let venv = self.venv(id).to_string_lossy().into_owned();
        let py = self.python(id).to_string_lossy().into_owned();
        let args = |a: &[&str]| -> Vec<String> { a.iter().map(|s| s.to_string()).collect() };
        let has_ensurepip = runner
            .run(base, &args(&["-c", "import ensurepip"]), "check for ensurepip")
            .await
            .is_ok();
        if has_ensurepip
            && runner
                .run(base, &args(&["-m", "venv", &venv]), "create the engine's Python environment")
                .await
                .is_ok()
        {
            return runner
                .run(
                    self.python(id).as_os_str(),
                    &args(&["-m", "pip", "install", "--upgrade", "pip"]),
                    "update pip",
                )
                .await;
        }
        runner
            .run(
                base,
                &args(&["-m", "venv", "--without-pip", &venv]),
                "create the engine's Python environment",
            )
            .await?;
        runner
            .run(
                base,
                &args(&["-m", "pip", "--python", &py, "install", "--upgrade", "pip"]),
                "install pip into the engine's Python environment",
            )
            .await
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

async fn find_python(min_minor: u32) -> Option<String> {
    if let Some(p) = studio_python() {
        // An explicit Python is the answer, right or wrong: never swap in another.
        return python_ok(&p, min_minor).await.then_some(p);
    }
    for cand in [
        "python3.13",
        "python3.12",
        "python3.11",
        "python3.10",
        "python3",
        "python",
    ] {
        if python_ok(cand, min_minor).await {
            return Some(cand.to_string());
        }
    }
    None
}

async fn python_ok(program: &str, min_minor: u32) -> bool {
    let out = tokio::process::Command::new(program)
        .args(["-c", &format!("import sys; print(sys.version_info >= (3, {min_minor}))")])
        .output()
        .await;
    matches!(out, Ok(out) if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "True")
}

async fn run(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[String],
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
        let axolotl = allternit_studio_spec::engines::find("axolotl").unwrap();

        let mac = fit(&tune, &HostHardware::plain(Hardware::AppleSilicon));
        assert!(mac.ok);
        assert!(mac.methods_unavailable.is_empty());

        let mac_axolotl = fit(&axolotl, &HostHardware::plain(Hardware::AppleSilicon));
        assert!(!mac_axolotl.ok);
        assert!(!mac_axolotl.reason.is_empty());

        // CPU is a real device: allternit-tune runs, LoRA only, with a warning.
        let cpu = fit(&tune, &crate::studio::hardware::classify_probe("linux", "x86_64", &Default::default()));
        assert!(cpu.ok, "{}", cpu.reason);
        assert_eq!(cpu.hardware, Hardware::Cpu);
        assert!(cpu.warning.is_some());
        assert!(cpu.methods_unavailable.contains_key("qlora"));
        assert!(!cpu.methods_unavailable.contains_key("lora"));
    }

    /// The edge: records every command line; `fail` picks the ones that exit non-zero.
    struct FakeRunner {
        calls: std::sync::Mutex<Vec<Vec<String>>>,
        fail: fn(&[String]) -> bool,
    }

    impl FakeRunner {
        fn new(fail: fn(&[String]) -> bool) -> Self {
            Self { calls: Default::default(), fail }
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, program: &std::ffi::OsStr, args: &[String], what: &str) -> Result<(), String> {
            let mut line = vec![program.to_string_lossy().into_owned()];
            line.extend(args.iter().cloned());
            let failed = (self.fail)(&line);
            self.calls.lock().unwrap().push(line);
            if failed { Err(format!("Could not {what}.")) } else { Ok(()) }
        }
    }

    fn line(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    const KIT_URL: &str = "studio-engines @ https://github.com/Allternit/studio-engines/archive/e520474f67ec93888fcd07716f313ad5dbda3db9.tar.gz";

    #[tokio::test]
    async fn kit_engines_install_from_the_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = EngineDirs::new(tmp.path(), Hardware::CudaLinux);
        let cases = [
            ("axolotl", "axolotl==0.21.1", "3c0af0b1e381617c0df7c97a456ebccb4f1746116e5419c6601d9d15507c10d2", vec![]),
            ("llama-factory", "llamafactory==0.9.5", "10776e9b259798bf65f6c5343f6298f0302e92e9cd47472abe29eef69e286c6a", vec![]),
            ("torchtune", "torchtune==0.6.1", "bcbc4ee9774b5c5829a5ff13df851e0b0e32416c018af8db53faa3472ac256e7", vec!["torch", "torchao"]),
        ];
        for (id, spec, wheel, extra) in cases {
            let runner = FakeRunner::new(|_| false);
            dirs.install_with(id, "python3.12", &runner).await.unwrap();
            let venv = dirs.venv(id).to_string_lossy().into_owned();
            let py = dirs.python(id).to_string_lossy().into_owned();
            let pin = dirs.dir(id).join("upstream-pin.txt");
            let pin_s = pin.to_string_lossy().into_owned();
            let mut deps = line(&[&py, "-m", "pip", "install", "--extra-index-url", "https://download.pytorch.org/whl/cu134", spec]);
            deps.extend(extra.iter().map(|s| s.to_string()));
            assert_eq!(
                runner.calls(),
                vec![
                    line(&["python3.12", "-c", "import ensurepip"]),
                    line(&["python3.12", "-m", "venv", &venv]),
                    line(&[&py, "-m", "pip", "install", "--upgrade", "pip"]),
                    line(&[&py, "-m", "pip", "install", KIT_URL]),
                    line(&[&py, "-m", "pip", "install", "--no-deps", "--only-binary", ":all:", "--require-hashes", "-r", &pin_s]),
                    deps,
                ],
                "{id}"
            );
            assert_eq!(std::fs::read_to_string(&pin).unwrap(), format!("{spec} --hash=sha256:{wheel}\n"));
            // Installed once the venv python exists and the marker matches.
            std::fs::create_dir_all(dirs.python(id).parent().unwrap()).unwrap();
            std::fs::write(dirs.python(id), "").unwrap();
            assert!(dirs.venv_installed(id), "{id}");
        }
        assert!(dirs.install_with("nope", "python3.12", &FakeRunner::new(|_| false)).await.is_err());
    }

    #[tokio::test]
    async fn failed_install_leaves_no_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = EngineDirs::new(tmp.path(), Hardware::CudaLinux);
        dirs.install_with("axolotl", "python3.12", &FakeRunner::new(|_| false)).await.unwrap();
        std::fs::create_dir_all(dirs.python("axolotl").parent().unwrap()).unwrap();
        std::fs::write(dirs.python("axolotl"), "").unwrap();
        assert!(dirs.venv_installed("axolotl"));
        // A reinstall whose hash check fails must not keep the old marker.
        let bad = FakeRunner::new(|l| l.iter().any(|a| a == "--require-hashes"));
        assert!(dirs.install_with("axolotl", "python3.12", &bad).await.is_err());
        assert!(!dirs.venv_installed("axolotl"));
    }

    #[tokio::test]
    async fn venv_without_ensurepip_bootstraps_pip() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = EngineDirs::new(tmp.path(), Hardware::AppleSilicon);
        let venv = dirs.venv("torchtune").to_string_lossy().into_owned();
        let py = dirs.python("torchtune").to_string_lossy().into_owned();
        let bootstrap = vec![
            line(&["/bundled/python3", "-m", "venv", "--without-pip", &venv]),
            line(&["/bundled/python3", "-m", "pip", "--python", &py, "install", "--upgrade", "pip"]),
        ];

        // No ensurepip (python-build-standalone): straight to the bootstrap.
        let runner = FakeRunner::new(|l| l.iter().any(|a| a == "import ensurepip"));
        dirs.install_with("torchtune", "/bundled/python3", &runner).await.unwrap();
        let calls = runner.calls();
        assert_eq!(calls[0], line(&["/bundled/python3", "-c", "import ensurepip"]));
        assert_eq!(calls[1..3], bootstrap[..]);
        // Then the kit, hash-checked wheel and deps (no torch index on a Mac).
        assert_eq!(calls[3], line(&[&py, "-m", "pip", "install", KIT_URL]));
        assert_eq!(calls[5], line(&[&py, "-m", "pip", "install", "torchtune==0.6.1", "torch", "torchao"]));
        assert_eq!(calls.len(), 6);

        // ensurepip imports but `-m venv` still fails: same bootstrap.
        let runner = FakeRunner::new(|l| l.len() == 4 && l[1] == "-m" && l[2] == "venv");
        dirs.install_with("torchtune", "/bundled/python3", &runner).await.unwrap();
        let calls = runner.calls();
        assert_eq!(calls[1], line(&["/bundled/python3", "-m", "venv", &venv]));
        assert_eq!(calls[2..4], bootstrap[..]);
        assert!(!calls.iter().any(|l| l[1..] == line(&["-m", "pip", "install", "--upgrade", "pip"])[..]));
    }

    #[test]
    fn kit_marker_changes_with_pin_and_hardware() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = EngineDirs::new(tmp.path(), Hardware::CudaLinux);
        let k = kit_engine("llama-factory").unwrap();
        std::fs::create_dir_all(dirs.python(k.id).parent().unwrap()).unwrap();
        std::fs::write(dirs.python(k.id), "").unwrap();
        let current = kit_marker(STUDIO_ENGINES_SHA, k, Hardware::CudaLinux);
        for stale in [
            kit_marker("0000000000000000000000000000000000000000", k, Hardware::CudaLinux),
            kit_marker(STUDIO_ENGINES_SHA, k, Hardware::CudaWindows),
            kit_marker(STUDIO_ENGINES_SHA, &KitEngine { version: "0.9.4", ..*k }, Hardware::CudaLinux),
        ] {
            assert_ne!(stale, current);
            std::fs::write(dirs.marker(k.id), &stale).unwrap();
            assert!(!dirs.venv_installed(k.id));
        }
        std::fs::write(dirs.marker(k.id), format!("{current}\n")).unwrap();
        assert!(dirs.venv_installed(k.id));
    }

    #[test]
    fn launch_command_per_engine() {
        let job = Path::new("/data/studio/jobs/j1");
        let out = Path::new("/data/studio/outputs/j1");
        assert_eq!(
            launch_args("allternit-tune", 4100, job, out).unwrap(),
            line(&["-m", "allternit_tune.server", "--port", "4100", "--data-dir", "/data/studio/jobs/j1/engine"])
        );
        for (id, bin_var, cli) in [
            ("axolotl", "STUDIO_ENGINE_BIN_AXOLOTL", "axolotl"),
            ("llama-factory", "STUDIO_ENGINE_BIN_LLAMA_FACTORY", "llamafactory-cli"),
            ("torchtune", "STUDIO_ENGINE_BIN_TORCHTUNE", "tune"),
        ] {
            assert_eq!(
                launch_args(id, 4100, job, out).unwrap(),
                line(&["-m", "studio_engine_kit", "--engine", id, "--port", "4100", "--data-dir", "/data/studio/outputs/j1"])
            );
            let env = kit_launch_env(id, Path::new("/e/venv/bin/python"), "tok");
            assert_eq!(env[0], ("STUDIO_ENGINE_TOKEN".to_string(), "tok".into()));
            assert_eq!(env[1].0, bin_var);
            assert_eq!(Path::new(&env[1].1), Path::new("/e/venv/bin").join(cli));
        }
        assert!(launch_args("nope", 1, job, out).is_none());
        assert!(kit_launch_env("allternit-tune", Path::new("/p"), "t").is_empty());
    }

    #[test]
    fn kit_pins_match_the_catalog() {
        for k in KIT_ENGINES {
            let card = allternit_studio_spec::engines::find(k.id).unwrap();
            assert_eq!(card.version, k.version, "{}", k.id);
            assert!(card.published, "{}", k.id);
            let src = card.source.unwrap();
            assert_eq!(src.repo, STUDIO_ENGINES_REPO);
            assert_eq!(src.sha, STUDIO_ENGINES_SHA, "{}: engines.json source.sha", k.id);
            assert_eq!(k.wheel_sha256.len(), 64);
        }
    }

    #[test]
    fn torch_index_matches_the_tune_requirements() {
        let tune = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../services/model-training");
        for hw in &Hardware::ALL {
            let text = std::fs::read_to_string(requirements_file(&tune, *hw)).unwrap();
            let index = text
                .lines()
                .find_map(|l| l.strip_prefix("--extra-index-url "))
                .map(str::trim);
            assert_eq!(torch_index(*hw), index, "{}", hw.id());
        }
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
