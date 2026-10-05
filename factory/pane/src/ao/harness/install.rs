//! `ao harness install <tool>` — P7 port of HarnessRouter CE's version-pinned
//! per-backend entrypoint install scripts (spec binding 1: install = data +
//! logic, never a shipped Python/ shell layer; HR CE `docker/entrypoint.sh`
//! is the reference, vendored for P6, read-only here).
//!
//! Design rules carried over from the HR CE entrypoint:
//! - **The executable IS the definition of "installed"** — an installer that
//!   exits 0 without producing `<managed>/bin/<tool>` is a failed install
//!   (HR's own comment, verbatim rationale).
//! - **Pins are explicit data, never "latest"** — bumping a pin is a manifest
//!   edit + re-run install; doctor surfaces the drift.
//! - **One managed dir** — `AO_HARNESS_HOME` or `~/.ao/harness`. PATH gains
//!   the managed `bin/` only inside ao-spawned subprocesses (the harness
//!   `FsCtx` probes and the verify runs), never the user's shell rc.
//! - **License gate (plan §8, hard)** — only apache/mit/bsd-class tools
//!   install without `--accept-terms <tool>`. proprietary-terms (claude et
//!   al.) and undeclared (hermes, dsh) refuse without it; acceptance is
//!   recorded in `<managed>/accepted-terms.json` (tool, license class, pin,
//!   timestamp) and RE-FLAGGED when the pin or class changes.
//! - **Network behind a trait** — [`InstallBackend`] is the only fetch
//!   surface; unit tests inject [`FakeBackend`] fixtures. Real npm/pip runs
//!   happen only in the hard-gate demo.
//!
//! Executor registration after install is deliberately NOT new code (spec
//! binding 5): the P4 driver probe (`installed()`) sees the managed binary
//! through `FsCtx`, so `ao harness sync` picks the tool up unchanged.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{Driver, LicenseClass, Manifest};

/// Terms acceptance record — one row per tool in `accepted-terms.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TermsRecord {
    pub(crate) tool: String,
    pub(crate) license_class: String,
    pub(crate) pin: Option<String>,
    pub(crate) accepted_at: String,
}

/// Whether the recorded acceptance still covers the manifest's pin/class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcceptState {
    /// apache/mit/bsd — no acceptance needed.
    NotRequired,
    /// Recorded, and pin + class match the manifest.
    Current { accepted_at: String },
    /// Recorded, but the pin or class moved — re-acceptance required.
    Stale {
        recorded_class: String,
        recorded_pin: Option<String>,
        accepted_at: String,
    },
    /// Gated class with no recorded acceptance.
    Missing,
}

// ---------------------------------------------------------------------------
// Managed dir
// ---------------------------------------------------------------------------

/// One managed dir for everything P7 installs (spec binding 3).
pub(crate) fn managed_root() -> PathBuf {
    if let Ok(root) = std::env::var("AO_HARNESS_HOME") {
        return PathBuf::from(root);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join(".ao").join("harness")
}

pub(crate) fn managed_bin_dir() -> PathBuf {
    managed_root().join("bin")
}

fn terms_path(root: &Path) -> PathBuf {
    root.join("accepted-terms.json")
}

/// Tolerant load: a missing file means "nothing accepted yet"; a corrupt file
/// is treated the same way (the gate re-asks rather than crashing on a
/// hand-edited state file).
pub(crate) fn load_terms(root: &Path) -> Vec<TermsRecord> {
    let Ok(text) = std::fs::read_to_string(terms_path(root)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub(crate) fn save_terms(root: &Path, records: &[TermsRecord]) -> io::Result<()> {
    std::fs::create_dir_all(root)?;
    let mut text = serde_json::to_string_pretty(records)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    text.push('\n');
    std::fs::write(terms_path(root), text)
}

/// Upsert the acceptance record for `tool` (pin/class/timestamp refreshed).
pub(crate) fn record_acceptance(root: &Path, records: &mut Vec<TermsRecord>, tool: &str, class: LicenseClass, pin: &Option<String>) -> io::Result<()> {
    records.retain(|r| r.tool != tool);
    records.push(TermsRecord {
        tool: tool.to_string(),
        license_class: class.as_str().to_string(),
        pin: pin.clone(),
        accepted_at: super::iso8601_now(),
    });
    save_terms(root, records)
}

/// Compare the manifest's class+pin against what was accepted.
pub(crate) fn accept_state(records: &[TermsRecord], tool: &str, class: LicenseClass, pin: &Option<String>) -> AcceptState {
    if !class.requires_acceptance() {
        return AcceptState::NotRequired;
    }
    match records.iter().find(|r| r.tool == tool) {
        None => AcceptState::Missing,
        Some(record) if record.license_class == class.as_str() && record.pin == *pin => {
            AcceptState::Current { accepted_at: record.accepted_at.clone() }
        }
        Some(record) => AcceptState::Stale {
            recorded_class: record.license_class.clone(),
            recorded_pin: record.pin.clone(),
            accepted_at: record.accepted_at.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// Install backend (the network/exec surface — everything behind this trait)
// ---------------------------------------------------------------------------

/// The only place real installs happen. Unit tests substitute
/// [`FakeBackend`]; production uses [`SystemBackend`].
pub(crate) trait InstallBackend {
    /// `npm install -g --prefix <root> --no-audit --no-fund <args…>` — the
    /// HR CE entrypoint form (`-g` is what creates `<root>/bin/<cmd>`).
    fn npm_global(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()>;
    /// Isolated venv + pinned pip, then a `<root>/bin/<tool>` shim onto the
    /// venv entry point (HR CE form for hermes/dsh — own venv so dependency
    /// trees never fight; the shim only after pip succeeded).
    fn venv_pip(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()>;
    /// Vendor bootstrap script: `curl -fsSL <url> | bash -s -- <rest…>`.
    /// `{bin}` in rest args is replaced with `<root>/bin`. After the script
    /// runs, the tool key must exist as `<root>/bin/<tool>` (copied/shimmed
    /// from `~/.local/bin` when the vendor hardcodes that path).
    fn script(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()>;
}

/// Run `cmd`, capture combined output, and fail with the tail of the log on
/// non-zero exit — the HR `try_install` rule that silence cost a day:
/// an install failure must SAY WHY, where the person looking will find it.
fn run_logged(cmd: &mut Command, what: &str) -> io::Result<()> {
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(());
    }
    let mut log = String::from_utf8_lossy(&output.stdout).into_owned();
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    let tail: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = tail.iter().rev().take(12).rev().copied().collect::<Vec<_>>().join("\n  ");
    Err(io::Error::new(
        io::ErrorKind::Other,
        format!("{what} failed (exit {}):\n  {tail}", output.status),
    ))
}

fn path_env_with(root: &Path) -> io::Result<std::ffi::OsString> {
    let bin = root.join("bin");
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

pub(crate) struct SystemBackend;

impl InstallBackend for SystemBackend {
    fn npm_global(&self, root: &Path, _tool: &str, install_args: &[String]) -> io::Result<()> {
        std::fs::create_dir_all(root)?;
        let mut cmd = Command::new("npm");
        cmd.args(["install", "-g", "--prefix"])
            .arg(root)
            .args(["--no-audit", "--no-fund"])
            .args(install_args)
            .env("PATH", path_env_with(root)?)
            // npm's update notifier writes cache/state under $HOME; harmless,
            // but an interactive prompt would hang an install — disable it.
            .env("NO_UPDATE_NOTIFIER", "1");
        run_logged(&mut cmd, "npm install")
    }

    fn venv_pip(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()> {
        std::fs::create_dir_all(root)?;
        let venv = root.join(format!("venv-{tool}"));
        if venv.exists() {
            std::fs::remove_dir_all(&venv)?;
        }
        run_logged(
            Command::new("python3").args(["-m", "venv"]).arg(&venv),
            "python3 -m venv",
        )?;
        run_logged(
            Command::new(venv.join("bin").join("pip"))
                .args(["install", "--no-cache-dir", "-q"])
                .args(install_args),
            "pip install",
        )?;
        // The shim only exists after pip succeeded (HR dsh-ready pattern):
        // a venv whose pip half-failed must not report the tool available.
        let entry = venv.join("bin").join(tool);
        if !entry.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("venv install produced no {tool} entry point at {}", entry.display()),
            ));
        }
        write_shim(&root.join("bin").join(tool), &entry)
    }

    fn script(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()> {
        let url = install_args.first().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "script install needs a URL")
        })?;
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        let bin_str = bin_dir.to_string_lossy().into_owned();
        let extra: Vec<String> = install_args
            .iter()
            .skip(1)
            .map(|arg| arg.replace("{bin}", &bin_str))
            .collect();
        let curl = Command::new("curl")
            .args(["-fsSL", url])
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let curl_out = curl.stdout.ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "curl produced no stdout")
        })?;
        let mut bash = Command::new("bash");
        bash.args(["-s", "--"]).args(&extra).stdin(curl_out);
        run_logged(&mut bash, &format!("install script {url}"))?;
        let dest = bin_dir.join(tool);
        if dest.exists() {
            return Ok(());
        }
        // Vendor scripts that ignore --dir (Cursor) land in ~/.local/bin.
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let local = home.join(".local").join("bin");
        for name in [tool, "cursor-agent", "agent", "agy"] {
            let src = local.join(name);
            if src.exists() {
                return write_shim(&dest, &src);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("script install of {tool} produced no {}", dest.display()),
        ))
    }
}

#[cfg(unix)]
fn write_shim(shim: &Path, target: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(shim.parent().unwrap_or_else(|| Path::new(".")))?;
    std::fs::write(shim, format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", target.display()))?;
    std::fs::set_permissions(shim, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn write_shim(shim: &Path, target: &Path) -> io::Result<()> {
    // Windows install methods are out of scope for P7 (spec non-goal); the
    // venv-pip method is POSIX-only and says so when asked.
    let _ = (shim, target);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "venv-pip install method is POSIX-only in P7 (spec non-goal: Windows)",
    ))
}

/// Test fixture backend: no network, no npm/pip — writes an executable
/// `<root>/bin/<tool>` that echoes its pinned version, so the real verify
/// path (run verifyCmd through PATH) exercises end to end.
#[cfg(test)]
pub(crate) struct FakeBackend {
    /// Set by the last npm_global call: the package argument.
    pub(crate) last_npm_args: std::cell::RefCell<Vec<String>>,
    pub(crate) calls: std::cell::RefCell<Vec<String>>,
}

#[cfg(test)]
impl FakeBackend {
    pub(crate) fn new() -> Self {
        FakeBackend {
            last_npm_args: std::cell::RefCell::new(Vec::new()),
            calls: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn write_fake_tool(root: &Path, name: &str, version_echo: &str) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin)?;
        let tool = bin.join(name);
        std::fs::write(&tool, format!("#!/bin/sh\necho \"{version_echo}\"\n"))?;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
    }

    /// `name@version` → (name, version). A bare name (no @) gets no version.
    fn split_pkg(spec: &str) -> (String, Option<String>) {
        match spec.rsplit_once('@') {
            Some((name, version)) if !name.is_empty() && !version.is_empty() => {
                (name.rsplit('/').next().unwrap_or(name).to_string(), Some(version.to_string()))
            }
            _ => (spec.rsplit('/').next().unwrap_or(spec).to_string(), None),
        }
    }
}

#[cfg(test)]
impl InstallBackend for FakeBackend {
    fn npm_global(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()> {
        *self.last_npm_args.borrow_mut() = install_args.to_vec();
        self.calls.borrow_mut().push(format!("npm:{install_args:?}"));
        // The binary name is the TOOL KEY, not the npm package name
        // (@moonshot-ai/kimi-code installs a `kimi` binary; @openai/codex a
        // `codex` one). The version echo comes from the package pin so the
        // real verify path (verifyCmd through PATH) checks out end to end.
        let spec = install_args
            .first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "npm install needs a package"))?;
        let (_name, version) = Self::split_pkg(spec);
        Self::write_fake_tool(root, tool, version.as_deref().unwrap_or(tool))
    }

    fn venv_pip(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()> {
        self.calls.borrow_mut().push(format!("venv:{tool}:{install_args:?}"));
        let pin = install_args
            .iter()
            .find_map(|arg| arg.split_once("==").map(|(_, v)| v.to_string()));
        Self::write_fake_tool(root, tool, pin.as_deref().unwrap_or(tool))
    }

    fn script(&self, root: &Path, tool: &str, install_args: &[String]) -> io::Result<()> {
        self.calls.borrow_mut().push(format!("script:{tool}:{install_args:?}"));
        Self::write_fake_tool(root, tool, tool)
    }
}

// ---------------------------------------------------------------------------
// Verify (the doctor-side check shares this)
// ---------------------------------------------------------------------------

/// Run the manifest's `verifyCmd` with the managed bin dir on PATH; returns
/// the combined output's first line. `None` when the command cannot run at
/// all (missing binary, spawn failure).
pub(crate) fn run_verify(root: &Path, verify_cmd: &str) -> io::Result<Option<String>> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(verify_cmd)
        .env("PATH", path_env_with(root)?)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let Ok(output) = cmd.output() else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().next().map(str::to_string))
}

pub(crate) fn pin_matches(output: &str, pin: &Option<String>) -> bool {
    match pin {
        None => true,
        Some(pin) => output.contains(pin.as_str()),
    }
}

// ---------------------------------------------------------------------------
// Install flow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    /// Binary present, verifyCmd matches the manifest pin — no work done.
    AlreadyInstalled,
    /// Installed (or would be, in dry-run) successfully.
    Installed,
}

#[derive(Debug)]
pub(crate) struct InstallReport {
    pub(crate) tool: String,
    pub(crate) label: String,
    pub(crate) outcome: InstallOutcome,
    /// Human lines, printed under the `== label ==` header.
    pub(crate) lines: Vec<String>,
}

/// The license gate. Returns `Ok(())` when the install may proceed, with any
/// acceptance recorded (unless dry-run). Returns `Err(message)` naming the
/// exact terms and the required flag otherwise.
pub(crate) fn check_gate(
    root: &Path,
    tool: &str,
    class: LicenseClass,
    pin: &Option<String>,
    explicit_accept: bool,
    dry_run: bool,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let records = load_terms(root);
    match accept_state(&records, tool, class, pin) {
        AcceptState::NotRequired => {
            lines.push(format!("license: {} — no acceptance required", class.as_str()));
        }
        AcceptState::Current { accepted_at } => {
            lines.push(format!(
                "license: {} — terms accepted {accepted_at} (pin {})",
                class.as_str(),
                pin.as_deref().unwrap_or("none")
            ));
        }
        AcceptState::Stale { recorded_class, recorded_pin, accepted_at } => {
            if !explicit_accept {
                return Err(format!(
                    "{tool} is {cls} — terms were accepted on {accepted_at} for \
                     class '{recorded_class}' pin {old_pin}, but the manifest now says \
                     pin {new_pin}. The pin changed, so the acceptance does not carry over. \
                     Re-review the vendor terms, then re-run with --accept-terms {tool}",
                    cls = class.as_str(),
                    old_pin = recorded_pin.as_deref().unwrap_or("none"),
                    new_pin = pin.as_deref().unwrap_or("none"),
                ));
            }
            lines.push(format!(
                "license: {} — terms RE-accepted for pin {} (was accepted {accepted_at} for class '{recorded_class}' pin {})",
                class.as_str(),
                pin.as_deref().unwrap_or("none"),
                recorded_pin.as_deref().unwrap_or("none"),
            ));
            if !dry_run {
                let mut records = records;
                record_acceptance(root, &mut records, tool, class, pin)
                    .map_err(|err| format!("recording terms acceptance: {err}"))?;
            }
        }
        AcceptState::Missing => {
            if !explicit_accept {
                return Err(format!(
                    "{tool} is {cls} — installing it means fetching and running the \
                     vendor's code under the vendor's terms (see the tool's _licenseNote \
                     in Ops/harness.json for the evidence URL). This class installs only \
                     with an explicit opt-in: re-run with --accept-terms {tool}",
                    cls = class.as_str(),
                ));
            }
            lines.push(format!(
                "license: {} — terms accepted via --accept-terms (pin {})",
                class.as_str(),
                pin.as_deref().unwrap_or("none"),
            ));
            if !dry_run {
                let mut records = records;
                record_acceptance(root, &mut records, tool, class, pin)
                    .map_err(|err| format!("recording terms acceptance: {err}"))?;
            }
        }
    }
    Ok(lines)
}

/// Install one tool into `root`. All filesystem and process effects go
/// through `backend` (network behind the trait — spec binding 4).
pub(crate) fn install_tool(
    manifest: &Manifest,
    driver: &Driver,
    root: &Path,
    tool: &str,
    explicit_accept: bool,
    dry_run: bool,
    backend: &dyn InstallBackend,
) -> Result<InstallReport, String> {
    let cfg = manifest
        .tools
        .get(tool)
        .ok_or_else(|| format!("unknown tool '{tool}' (not in the 16-tool manifest)"))?;
    let block = cfg
        .install
        .as_ref()
        .ok_or_else(|| format!("{tool} has no install block in the manifest"))?;
    let class = cfg.license.unwrap_or(LicenseClass::Undeclared);

    let mut lines = Vec::new();
    if block.method == "unsupported" {
        return Err(format!(
            "{} has no version-pinned install channel ao can drive (method 'unsupported' — \
             see its _licenseNote in Ops/harness.json). Install it by its vendor's own \
             means; `ao harness` will pick it up via the normal installed() probes.",
            driver.label,
        ));
    }
    lines.extend(check_gate(root, tool, class, &block.pinned_version, explicit_accept, dry_run)?);

    let bin = root.join("bin").join(tool);
    if bin.exists() {
        if let Some(version_line) = run_verify(root, block.verify_cmd.as_deref().unwrap_or(tool))
            .map_err(|err| err.to_string())?
        {
            if pin_matches(&version_line, &block.pinned_version) {
                lines.push(format!("already installed — {} (pin match)", version_line));
                lines.push(format!("binary: {}", bin.display()));
                return Ok(InstallReport {
                    tool: tool.to_string(),
                    label: driver.label.to_string(),
                    outcome: InstallOutcome::AlreadyInstalled,
                    lines,
                });
            }
            lines.push(format!(
                "existing binary reports '{version_line}' — manifest pin is {} ; reinstalling",
                block.pinned_version.as_deref().unwrap_or("none")
            ));
        }
    }

    let command_desc = match block.method.as_str() {
        "npm" => format!("npm install -g --prefix {} --no-audit --no-fund {}", root.display(), block.install_args.join(" ")),
        "venv-pip" => format!(
            "python3 -m venv {}/venv-{tool} && pip install --no-cache-dir {}",
            root.display(),
            block.install_args.join(" ")
        ),
        "script" => format!(
            "curl -fsSL {} | bash -s -- {}",
            block.install_args.first().map(String::as_str).unwrap_or(""),
            block
                .install_args
                .iter()
                .skip(1)
                .map(|arg| arg.replace("{bin}", &root.join("bin").display().to_string()))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        other => {
            return Err(format!(
                "{tool}: unknown install method '{other}' (expected npm | venv-pip | script | unsupported)"
            ))
        }
    };
    lines.push(format!("install: {command_desc}"));
    if dry_run {
        lines.push("dry run — nothing fetched, nothing written".to_string());
        return Ok(InstallReport {
            tool: tool.to_string(),
            label: driver.label.to_string(),
            outcome: InstallOutcome::Installed,
            lines,
        });
    }

    match block.method.as_str() {
        "npm" => backend.npm_global(root, tool, &block.install_args).map_err(|err| err.to_string())?,
        "venv-pip" => backend.venv_pip(root, tool, &block.install_args).map_err(|err| err.to_string())?,
        "script" => backend.script(root, tool, &block.install_args).map_err(|err| err.to_string())?,
        _ => unreachable!("method validated above"),
    }

    // The executable IS the definition of "installed" (HR entrypoint rule).
    if !bin.exists() {
        return Err(format!(
            "install of {tool} exited without producing {} — treating as failed (HR rule: \
             an installer that exits 0 without the binary is a failed install)",
            bin.display()
        ));
    }

    let version_line = run_verify(root, block.verify_cmd.as_deref().unwrap_or(tool))
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            format!(
                "verifyCmd '{}' could not run after install — refusing to report success",
                block.verify_cmd.as_deref().unwrap_or(tool)
            )
        })?;
    if !pin_matches(&version_line, &block.pinned_version) {
        return Err(format!(
            "pin mismatch after install: verifyCmd reports '{version_line}' but the manifest \
             pins {} — the managed binary is NOT what the manifest says; investigate before use",
            block.pinned_version.as_deref().unwrap_or("none")
        ));
    }
    lines.push(format!("verify: {} (pin match)", version_line));
    lines.push(format!("binary: {}", bin.display()));

    // Install receipt — what ao installed, at what pin, when.
    std::fs::create_dir_all(root.join("installed"))
        .map_err(|err| format!("writing install receipt: {err}"))?;
    let receipt = serde_json::json!({
        "tool": tool,
        "pinnedVersion": block.pinned_version,
        "method": block.method,
        "installedAt": super::iso8601_now(),
    });
    std::fs::write(
        root.join("installed").join(format!("{tool}.json")),
        format!("{}\n", serde_json::to_string_pretty(&receipt).unwrap()),
    )
    .map_err(|err| format!("writing install receipt: {err}"))?;

    Ok(InstallReport {
        tool: tool.to_string(),
        label: driver.label.to_string(),
        outcome: InstallOutcome::Installed,
        lines,
    })
}

// ---------------------------------------------------------------------------
// `ao harness install` CLI
// ---------------------------------------------------------------------------

pub(crate) fn cmd_install(
    manifest: &Manifest,
    tools: &[&str],
    accepts_terms: &[String],
    dry_run: bool,
) -> io::Result<i32> {
    if tools.is_empty() {
        eprintln!("usage: ao harness install <tool…> [--accept-terms <tool>] [--dry-run]");
        return Ok(2);
    }
    let root = managed_root();
    println!("Harness install — managed dir: {}", root.display());
    if dry_run {
        println!("Dry run — no fetches, no writes.");
    }
    let backend = SystemBackend;
    let mut failures = 0usize;
    let mut already = 0usize;
    for tool in tools {
        let Some(driver) = driver_by_key(tool) else {
            eprintln!("== {tool} ==\n   ERROR: unknown tool (expected one of: {})", DRIVER_KEYS.join(", "));
            failures += 1;
            continue;
        };
        println!("\n== {} ==", driver.label);
        match install_tool(manifest, driver, &root, tool, accepts_terms.iter().any(|t| t == tool), dry_run, &backend) {
            Ok(report) => {
                if report.outcome == InstallOutcome::AlreadyInstalled {
                    already += 1;
                }
                for line in &report.lines {
                    println!("   {line}");
                }
            }
            Err(message) => {
                println!("   ERROR: {message}");
                failures += 1;
            }
        }
    }
    let unused: Vec<&str> = accepts_terms
        .iter()
        .map(String::as_str)
        .filter(|t| !tools.contains(t))
        .collect();
    if !unused.is_empty() {
        println!("\nwarning: --accept-terms given for tools not being installed: {}", unused.join(", "));
    }
    if failures > 0 {
        println!("\n{failures} tool(s) failed.");
        return Ok(1);
    }
    println!(
        "\nDone.{} The managed bin dir is on PATH for ao-spawned subprocesses only — run `ao doctor` to verify.",
        if already > 0 { format!(" {already} already at pin.") } else { String::new() }
    );
    Ok(0)
}

fn driver_by_key(key: &str) -> Option<&'static Driver> {
    super::DRIVERS.iter().find(|d| d.key == key)
}

const DRIVER_KEYS: &[&str] = &[
    "claude", "codex", "kimi", "grok", "cursor", "gizzi", "agy", "opencode",
    "antigravity", "qwen", "codebuddy", "workbuddy", "openclaw", "hermes", "dsh", "qoder",
];

// ---------------------------------------------------------------------------
// `ao doctor` harness section (spec binding 7)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct DoctorRow {
    pub(crate) tool: String,
    pub(crate) status: &'static str,
    pub(crate) detail: String,
}

#[derive(Debug)]
pub(crate) struct HarnessDoctor {
    pub(crate) root: PathBuf,
    pub(crate) rows: Vec<DoctorRow>,
    /// Green when nothing ao-managed is broken (an absent managed dir is
    /// not a problem — it just means ao installed nothing yet).
    pub(crate) ok: bool,
}

/// Aggregate managed-dir health, per-tool binary+pin match, license state,
/// and sync reachability of ao-managed tools. `root` is the managed dir
/// (injected for tests; production passes `managed_root()`). `ctx` is the
/// harness probe context (managed bin already joined — see
/// `FsCtx::from_env`).
pub(crate) fn doctor(manifest: &Manifest, root: &Path, ctx: &super::FsCtx) -> HarnessDoctor {
    let mut rows = Vec::new();
    let mut ok = true;

    if !root.exists() {
        rows.push(DoctorRow {
            tool: "managed dir".to_string(),
            status: "absent",
            detail: format!("{} — ao has installed nothing (run `ao harness install <tool>`)", root.display()),
        });
        return HarnessDoctor { root: root.to_path_buf(), rows, ok: true };
    }

    let terms = load_terms(&root);
    for driver in super::DRIVERS {
        let Some(cfg) = manifest.tools.get(driver.key) else { continue };
        let Some(block) = &cfg.install else { continue };
        if block.method == "unsupported" {
            rows.push(DoctorRow {
                tool: driver.label.to_string(),
                status: "external",
                detail: "not ao-managed (no pinned install channel)".to_string(),
            });
            continue;
        }
        let bin = root.join("bin").join(driver.key);
        if !bin.exists() {
            rows.push(DoctorRow {
                tool: driver.label.to_string(),
                status: "not installed",
                detail: String::new(),
            });
            continue;
        }
        let class = cfg.license.unwrap_or(LicenseClass::Undeclared);
        let mut problems: Vec<String> = Vec::new();
        let mut detail = String::new();

        match run_verify(&root, block.verify_cmd.as_deref().unwrap_or(driver.key)) {
            Ok(Some(version_line)) if pin_matches(&version_line, &block.pinned_version) => {
                detail.push_str(&version_line);
            }
            Ok(Some(version_line)) => {
                problems.push(format!(
                    "pin mismatch (manifest {} vs binary '{version_line}')",
                    block.pinned_version.as_deref().unwrap_or("none")
                ));
            }
            _ => problems.push("verifyCmd failed or binary not runnable".to_string()),
        }

        match accept_state(&terms, driver.key, class, &block.pinned_version) {
            AcceptState::NotRequired => detail.push_str(&format!(" · license {}", class.as_str())),
            AcceptState::Current { accepted_at } => {
                detail.push_str(&format!(" · license {} · terms accepted {accepted_at}", class.as_str()));
            }
            AcceptState::Stale { recorded_pin, accepted_at, .. } => {
                ok = false;
                problems.push(format!(
                    "terms STALE — accepted {accepted_at} for pin {}, manifest now {}",
                    recorded_pin.as_deref().unwrap_or("none"),
                    block.pinned_version.as_deref().unwrap_or("none")
                ));
            }
            AcceptState::Missing => {
                ok = false;
                problems.push(format!(
                    "terms NOT ACCEPTED ({} @ {}) — run: ao harness install {} --accept-terms {}",
                    class.as_str(),
                    block.pinned_version.as_deref().unwrap_or("none"),
                    driver.key,
                    driver.key,
                ));
            }
        }

        if !driver.installed(ctx) {
            ok = false;
            problems.push("unreachable by `ao harness sync` (managed bin not on probe PATH)".to_string());
        }

        let status = if problems.is_empty() { "ok" } else {
            ok = false;
            "PROBLEM"
        };
        if !problems.is_empty() {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&problems.join(" ; "));
        }
        rows.push(DoctorRow { tool: driver.label.to_string(), status, detail });
    }

    HarnessDoctor { root: root.to_path_buf(), rows, ok }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ao::harness::{FsCtx, DRIVERS};

    /// Minimal fixture manifest exercising every gate branch. Shape matches
    /// the real manifest's ToolCfg (serde permissive — only what we use).
    const FIXTURE_MANIFEST: &str = r#"{
      "source": {
        "skillsDir": "/nonexistent-skills",
        "rulesFile": "/nonexistent-rules",
        "mcpServer": { "name": "t", "command": "node", "args": [] }
      },
      "tools": {
        "kimi": {
          "license": "mit",
          "install": {
            "method": "npm",
            "pinnedVersion": "0.42.0",
            "installArgs": ["@moonshot-ai/kimi-code@0.42.0"],
            "verifyCmd": "kimi --version"
          }
        },
        "codex": {
          "license": "apache",
          "install": {
            "method": "npm",
            "pinnedVersion": "0.154.0",
            "installArgs": ["@openai/codex@0.154.0"],
            "verifyCmd": "codex --version"
          }
        },
        "bsdtool": {
          "license": "bsd",
          "install": {
            "method": "npm",
            "pinnedVersion": "1.0.0",
            "installArgs": ["bsdtool@1.0.0"],
            "verifyCmd": "bsdtool --version"
          }
        },
        "claude": {
          "license": "proprietary-terms",
          "install": {
            "method": "npm",
            "pinnedVersion": "2.1.267",
            "installArgs": ["@anthropic-ai/claude-code@2.1.267"],
            "verifyCmd": "claude --version"
          }
        },
        "hermes": {
          "license": "undeclared",
          "install": {
            "method": "venv-pip",
            "pinnedVersion": "0.19.0",
            "installArgs": ["hermes-agent==0.19.0", "mcp>=1.9,<2"],
            "verifyCmd": "hermes --version"
          }
        },
        "grok": {
          "license": "apache",
          "install": {
            "method": "unsupported",
            "pinnedVersion": null,
            "installArgs": [],
            "verifyCmd": "grok --version"
          }
        }
      }
    }"#;

    fn fixture_manifest() -> Manifest {
        serde_json::from_str(FIXTURE_MANIFEST).expect("fixture manifest parses")
    }

    fn driver(key: &str) -> &'static Driver {
        // The real driver table is keyed to the real manifest; tests build
        // their own lookup over the 16 real drivers and reuse entries whose
        // keys overlap the fixture, synthesizing the rest through the table
        // minimum (key + label are all install_tool uses).
        DRIVERS.iter().find(|d| d.key == key).unwrap_or_else(|| {
            // Leak a synthetic driver — tests only need key/label.
            let driver = Driver {
                key: match key {
                    "bsdtool" => "bsdtool",
                    _ => panic!("no driver for {key}"),
                },
                label: "BSD Tool",
                probes: &[],
            };
            Box::leak(Box::new(driver))
        })
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("harness");
        (tmp, root)
    }

    #[test]
    fn free_class_installs_without_acceptance() {
        for key in ["kimi", "codex", "bsdtool"] {
            let (_tmp, root) = scratch();
            let manifest = fixture_manifest();
            let backend = FakeBackend::new();
            let report = install_tool(&manifest, driver(key), &root, key, false, false, &backend)
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert_eq!(report.outcome, InstallOutcome::Installed);
            assert!(root.join("bin").join(key).exists(), "{key} binary in managed bin");
            assert!(!terms_path(&root).exists(), "no acceptance recorded for free class");
            assert!(root.join("installed").join(format!("{key}.json")).exists());
        }
    }

    #[test]
    fn gated_classes_refuse_without_accept_terms() {
        for key in ["claude", "hermes"] {
            let (_tmp, root) = scratch();
            let manifest = fixture_manifest();
            let backend = FakeBackend::new();
            let err = install_tool(&manifest, driver(key), &root, key, false, false, &backend)
                .unwrap_err();
            assert!(err.contains("--accept-terms"), "{key} error names the flag: {err}");
            assert!(err.contains(key), "{key} error names the tool: {err}");
            assert!(!root.join("bin").join(key).exists(), "{key} must not install");
            assert!(!terms_path(&root).exists(), "{key} no acceptance recorded");
        }
    }

    #[test]
    fn gated_classes_install_with_acceptance_and_record_it() {
        for key in ["claude", "hermes"] {
            let (_tmp, root) = scratch();
            let manifest = fixture_manifest();
            let backend = FakeBackend::new();
            let report = install_tool(&manifest, driver(key), &root, key, true, false, &backend)
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert!(report.lines.iter().any(|l| l.contains("--accept-terms")));
            let terms = load_terms(&root);
            assert_eq!(terms.len(), 1, "{key} one acceptance recorded");
            let record = &terms[0];
            assert_eq!(record.tool, key);
            assert!(record.accepted_at.ends_with('Z'));
            let expected_class = if key == "claude" { "proprietary-terms" } else { "undeclared" };
            assert_eq!(record.license_class, expected_class);
            let expected_pin = if key == "claude" { "2.1.267" } else { "0.19.0" };
            assert_eq!(record.pin.as_deref(), Some(expected_pin));
        }
    }

    #[test]
    fn acceptance_is_invalidated_when_the_pin_changes() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        install_tool(&manifest, driver("claude"), &root, "claude", true, false, &backend).unwrap();

        // Manifest pin moves 2.1.267 -> 2.1.300: the old acceptance must NOT
        // carry over — install refuses until re-acceptance.
        let bumped = serde_json::from_str::<Manifest>(&FIXTURE_MANIFEST.replace("2.1.267", "2.1.300")).unwrap();
        let err = install_tool(&bumped, driver("claude"), &root, "claude", false, false, &backend)
            .unwrap_err();
        assert!(err.contains("2.1.300"), "names the new pin: {err}");
        assert!(err.contains("2.1.267"), "names the accepted pin: {err}");
        assert!(err.contains("--accept-terms claude"), "names the remedy: {err}");

        // Re-acceptance records the new pin.
        install_tool(&bumped, driver("claude"), &root, "claude", true, false, &backend).unwrap();
        let terms = load_terms(&root);
        assert_eq!(terms.len(), 1, "upsert, not append");
        assert_eq!(terms[0].pin.as_deref(), Some("2.1.300"));
    }

    #[test]
    fn unsupported_method_refuses_with_guidance() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        let err = install_tool(&manifest, driver("grok"), &root, "grok", false, false, &backend)
            .unwrap_err();
        assert!(err.contains("unsupported"), "{err}");
        assert!(!root.join("bin").join("grok").exists());
    }

    #[test]
    fn unknown_tool_and_missing_block_error() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        let err = install_tool(&manifest, driver("kimi"), &root, "nope", false, false, &backend).unwrap_err();
        assert!(err.contains("unknown tool"), "{err}");
    }

    #[test]
    fn install_is_idempotent_at_matching_pin() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        install_tool(&manifest, driver("kimi"), &root, "kimi", false, false, &backend).unwrap();
        let second = install_tool(&manifest, driver("kimi"), &root, "kimi", false, false, &backend).unwrap();
        assert_eq!(second.outcome, InstallOutcome::AlreadyInstalled);
        assert_eq!(backend.calls.borrow().len(), 1, "second call must not re-fetch");
    }

    /// A backend that exits 0 but produces no executable must fail (the HR
    /// "executable IS installed" rule).
    struct EmptyBackend;
    impl InstallBackend for EmptyBackend {
        fn npm_global(&self, _root: &Path, _tool: &str, _args: &[String]) -> io::Result<()> {
            Ok(())
        }
        fn venv_pip(&self, _root: &Path, _tool: &str, _args: &[String]) -> io::Result<()> {
            Ok(())
        }
        fn script(&self, _root: &Path, _tool: &str, _args: &[String]) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn installer_without_binary_is_a_failure() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let err = install_tool(&manifest, driver("kimi"), &root, "kimi", false, false, &EmptyBackend)
            .unwrap_err();
        assert!(err.contains("without producing"), "{err}");
    }

    /// A backend whose binary reports the WRONG version must fail the pin check.
    struct WrongVersionBackend;
    impl InstallBackend for WrongVersionBackend {
        fn npm_global(&self, root: &Path, _tool: &str, _args: &[String]) -> io::Result<()> {
            FakeBackend::write_fake_tool(root, "kimi", "0.0.0-test")
        }
        fn venv_pip(&self, _root: &Path, _tool: &str, _args: &[String]) -> io::Result<()> {
            Ok(())
        }
        fn script(&self, _root: &Path, _tool: &str, _args: &[String]) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pin_mismatch_after_install_is_a_failure() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let err = install_tool(&manifest, driver("kimi"), &root, "kimi", false, false, &WrongVersionBackend)
            .unwrap_err();
        assert!(err.contains("pin mismatch"), "{err}");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        let report = install_tool(&manifest, driver("claude"), &root, "claude", true, true, &backend).unwrap();
        assert!(report.lines.iter().any(|l| l.contains("dry run")));
        assert!(!root.exists(), "managed root must not be created by a dry run");
        // And nothing anywhere else in the scratch tree.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().filter_map(|e| e.ok()).collect();
        assert!(leftovers.is_empty(), "no files written outside the (uncreated) root");
    }

    #[test]
    fn everything_lives_under_the_managed_root() {
        let (_tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        install_tool(&manifest, driver("claude"), &root, "claude", true, false, &backend).unwrap();
        install_tool(&manifest, driver("hermes"), &root, "hermes", true, false, &backend).unwrap();
        for file in ["accepted-terms.json", "bin/claude", "bin/hermes", "installed/claude.json"] {
            assert!(root.join(file).exists(), "{file} under managed root");
        }
        // The venv-pip tool lands in the same bin/ and reports its pin
        // (the HR-isolation venv + shim wrapper is SystemBackend behavior,
        // exercised by the real hermes/dsh path only in the hard-gate demo).
        let out = run_verify(&root, "hermes --version").unwrap();
        assert_eq!(out.as_deref(), Some("0.19.0"));
    }

    #[test]
    fn terms_file_survives_corrupt_json() {
        let (_tmp, root) = scratch();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(terms_path(&root), "{ not json").unwrap();
        assert!(load_terms(&root).is_empty(), "corrupt terms file reads as nothing accepted");
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        let err = install_tool(&manifest, driver("claude"), &root, "claude", false, false, &backend).unwrap_err();
        assert!(err.contains("--accept-terms"), "gate re-asks instead of crashing: {err}");
    }

    // ---- doctor ----

    fn doctor_ctx(home: &Path, path_dirs: Vec<PathBuf>) -> FsCtx {
        FsCtx { home: home.to_path_buf(), path_dirs, app_root: home.to_path_buf() }
    }

    #[test]
    fn doctor_is_green_after_gated_install() {
        let (tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        install_tool(&manifest, driver("kimi"), &root, "kimi", false, false, &backend).unwrap();
        install_tool(&manifest, driver("claude"), &root, "claude", true, false, &backend).unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let ctx = doctor_ctx(&home, vec![root.join("bin")]);
        let report = doctor(&manifest, &root, &ctx);
        assert!(report.ok, "doctor must be green: {:?}", report.rows);
        let kimi_row = report.rows.iter().find(|r| r.tool.contains("Kimi")).unwrap();
        assert_eq!(kimi_row.status, "ok");
        assert!(kimi_row.detail.contains("0.42.0"));
        assert!(kimi_row.detail.contains("license mit"));
        let claude_row = report.rows.iter().find(|r| r.tool.contains("Claude")).unwrap();
        assert_eq!(claude_row.status, "ok");
        assert!(claude_row.detail.contains("terms accepted"));
    }

    #[test]
    fn doctor_reflags_stale_terms_on_pin_change() {
        let (tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        install_tool(&manifest, driver("claude"), &root, "claude", true, false, &backend).unwrap();
        let bumped = serde_json::from_str::<Manifest>(&FIXTURE_MANIFEST.replace("2.1.267", "2.1.300")).unwrap();
        // The old binary still reports 2.1.267: two problems — pin drift and
        // stale terms — and doctor must NOT be green.
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let ctx = doctor_ctx(&home, vec![root.join("bin")]);
        let report = doctor(&bumped, &root, &ctx);
        assert!(!report.ok, "stale terms must break green");
        let claude_row = report.rows.iter().find(|r| r.tool.contains("Claude")).unwrap();
        assert_eq!(claude_row.status, "PROBLEM");
        assert!(claude_row.detail.contains("STALE"), "{}", claude_row.detail);
        assert!(claude_row.detail.contains("pin mismatch"), "{}", claude_row.detail);
    }

    #[test]
    fn doctor_flags_managed_binary_unreachable_by_sync() {
        let (tmp, root) = scratch();
        let manifest = fixture_manifest();
        let backend = FakeBackend::new();
        install_tool(&manifest, driver("kimi"), &root, "kimi", false, false, &backend).unwrap();
        // Probe PATH WITHOUT the managed bin: installed() cannot see the tool.
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let ctx = doctor_ctx(&home, vec![]);
        let report = doctor(&manifest, &root, &ctx);
        assert!(!report.ok);
        let row = report.rows.iter().find(|r| r.tool.contains("Kimi")).unwrap();
        assert!(row.detail.contains("unreachable"), "{}", row.detail);
    }

    #[test]
    fn absent_managed_dir_is_not_a_problem() {
        let (_tmp, root) = scratch();
        assert!(!root.exists());
        let manifest = fixture_manifest();
        let ctx = doctor_ctx(&root, vec![]);
        let report = doctor(&manifest, &root, &ctx);
        assert!(report.ok, "nothing installed yet must stay green");
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].status, "absent");
    }
}
