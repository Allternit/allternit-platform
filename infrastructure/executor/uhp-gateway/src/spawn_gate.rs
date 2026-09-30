//! Spawn gate for UHP turns (audit S1 parts 2–3).
//!
//! Every turn launches a third-party CLI. Before launch:
//! * Claude Code (hooked) gets a session-scoped `--settings` file generated
//!   by `allternit-commrails hook claude-settings`, whose PreToolUse hook runs
//!   the hard floor, plus Gate 2 when the turn is bound to a WIH. No
//!   `--dangerously-skip-permissions`; a missing commrails binary refuses the
//!   turn instead of running unhooked.
//! * Codex (sandboxed) runs under its `workspace-write` OS sandbox.
//! * Everything else is `ungated`: allowed unbound, refused (via
//!   `allternit-commrails hook spawn-check`) on a WIH whose policy requires
//!   lease coverage for writes.
//!
//! The policy itself lives in `allternit-commrails` (`commrails/src/hook`);
//! this module only shells out to it so the gateway does not link CommRails.

use std::path::{Path, PathBuf};

use crate::drivers::{DriverKind, GateKind};

/// Env var naming the `allternit-commrails` binary.
pub const BIN_ENV: &str = "ALLTERNIT_COMMRAILS_BIN";
/// Env var naming the CommRails root that holds WIHs/leases/ledger.
pub const ROOT_ENV: &str = "ALLTERNIT_COMMRAILS_ROOT";

/// Locate `allternit-commrails`: `$ALLTERNIT_COMMRAILS_BIN`, then `PATH`.
pub fn commrails_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(BIN_ENV) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("allternit-commrails"))
        .find(|c| c.is_file())
}

/// What the turn needs to launch gated.
#[derive(Debug, Default)]
pub struct Prepared {
    /// Claude `--settings` file, when the driver is hooked.
    pub claude_settings: Option<PathBuf>,
}

/// Admit and prepare one turn. `Err` is a user-safe refusal message.
pub async fn prepare(
    driver: DriverKind,
    session_dir: &Path,
    workspace_dir: &Path,
    wih_id: Option<&str>,
) -> Result<Prepared, String> {
    let gate = driver.gate();
    // Unbound, non-hooked turns need nothing from CommRails.
    if wih_id.is_none() && gate != GateKind::Hook {
        return Ok(Prepared::default());
    }
    let bin = commrails_bin().ok_or_else(|| {
        format!(
            "spawn gate: allternit-commrails not found (set {BIN_ENV}); refusing to run {} without its gate",
            driver.binary()
        )
    })?;
    // A WIH lives in a CommRails root; unbound turns log floor denials into
    // the session dir.
    let root = match (wih_id, std::env::var_os(ROOT_ENV)) {
        (_, Some(root)) => PathBuf::from(root),
        (Some(_), None) => {
            return Err(format!("spawn gate: a WIH-bound turn needs {ROOT_ENV} set to the CommRails root"))
        }
        (None, None) => session_dir.to_path_buf(),
    };

    if let Some(wih) = wih_id {
        let out = tokio::process::Command::new(&bin)
            .arg("--root")
            .arg(&root)
            .args(["hook", "spawn-check", "--harness", driver.binary(), "--wih", wih])
            .output()
            .await
            .map_err(|err| format!("spawn gate: could not run allternit-commrails: {err}"))?;
        if !out.status.success() {
            let reason = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if reason.is_empty() {
                format!("spawn gate refused {} for WIH {wih}", driver.binary())
            } else {
                reason
            });
        }
    }

    if gate != GateKind::Hook {
        return Ok(Prepared::default());
    }
    let settings = session_dir.join("claude-settings.json");
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg("--root")
        .arg(&root)
        .args(["hook", "claude-settings", "--workspace"])
        .arg(workspace_dir)
        .arg("--out")
        .arg(&settings)
        .env_remove("ALLTERNIT_COMMRAILS_WIH");
    if let Some(wih) = wih_id {
        cmd.args(["--wih", wih]);
    }
    let out = cmd
        .output()
        .await
        .map_err(|err| format!("spawn gate: could not run allternit-commrails: {err}"))?;
    if !out.status.success() || !settings.is_file() {
        return Err(format!(
            "spawn gate: could not write claude hook settings: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Prepared {
        claude_settings: Some(settings),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // One test owns the env vars (they are process-global).
    #[tokio::test]
    async fn gate_refuses_and_prepares() {
        let tmp = tempfile::tempdir().unwrap();
        let session = tmp.path().join("session");
        let ws = session.join("workspace");
        std::fs::create_dir_all(&ws).unwrap();

        // No commrails binary: claude is refused, never run unhooked.
        std::env::set_var(BIN_ENV, tmp.path().join("missing"));
        std::env::remove_var(ROOT_ENV);
        let err = prepare(DriverKind::Claude, &session, &ws, None).await.unwrap_err();
        assert!(err.contains("refusing to run claude"), "{err}");
        // Unbound ungated drivers need nothing.
        assert!(prepare(DriverKind::Kimi, &session, &ws, None).await.unwrap().claude_settings.is_none());

        // Fake commrails: spawn-check refuses, claude-settings writes --out.
        let bin = tmp.path().join("allternit-commrails");
        std::fs::write(
            &bin,
            "#!/bin/sh\ncase \"$*\" in\n  *spawn-check*) echo 'refusing to spawn kimi: ungated' >&2; exit 3;;\n  *claude-settings*) while [ $# -gt 0 ]; do [ \"$1\" = --out ] && echo '{}' > \"$2\"; shift; done;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(BIN_ENV, &bin);

        // WIH-bound turns need the CommRails root.
        let err = prepare(DriverKind::Kimi, &session, &ws, Some("wih_1")).await.unwrap_err();
        assert!(err.contains(ROOT_ENV), "{err}");
        std::env::set_var(ROOT_ENV, tmp.path());
        let err = prepare(DriverKind::Kimi, &session, &ws, Some("wih_1")).await.unwrap_err();
        assert_eq!(err, "refusing to spawn kimi: ungated");

        let prepared = prepare(DriverKind::Claude, &session, &ws, None).await.unwrap();
        assert_eq!(prepared.claude_settings, Some(session.join("claude-settings.json")));

        std::env::remove_var(BIN_ENV);
        std::env::remove_var(ROOT_ENV);
    }
}
