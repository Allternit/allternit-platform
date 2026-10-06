//! CLI-invocation MCP mechanics — lib.js:377-395 port (`grok`, `agy`,
//! `opencode`, `qwen`). `execFileSync(..., {stdio: "pipe"})`: no shell, no
//! inherited stdio.

use std::process::{Command, Stdio};

use super::Action;

fn command_line(bin: &str, args: &[String]) -> String {
    // JS: `${bin} ${args.join(" ")}` — naive space join, reproduced.
    if args.is_empty() {
        bin.to_string()
    } else {
        format!("{} {}", bin, args.join(" "))
    }
}

/// `cliMcpSync()`. A spawn failure throws in the JS (uncaught → exit 1); the
/// io::Error propagation mirrors that (the harness command exits 1 with an
/// error on stderr instead of a JS stack trace — stdout parity is unaffected
/// because nothing is printed before the throw).
pub(crate) fn cli_sync(bin: &str, add_args: &[String], dry_run: bool) -> std::io::Result<Vec<Action>> {
    let detail = command_line(bin, add_args);
    if dry_run {
        return Ok(vec![Action::detailed("would-run", detail)]);
    }
    Command::new(bin)
        .args(add_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    Ok(vec![Action::detailed("add", detail)])
}

/// `cliMcpRemove()`: exec failure — spawn error OR non-zero exit — maps to
/// `nothing (not registered)`.
pub(crate) fn cli_remove(bin: &str, remove_args: &[String], dry_run: bool) -> Vec<Action> {
    let detail = command_line(bin, remove_args);
    if dry_run {
        return vec![Action::detailed("would-run", detail)];
    }
    let outcome = Command::new(bin)
        .args(remove_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if outcome {
        vec![Action::detailed("remove", detail)]
    } else {
        vec![Action::nothing("not registered")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_bin(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let bin = dir.join(name);
        std::fs::write(&bin, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    #[test]
    fn dry_run_never_shells_out() {
        // A bin that does not exist still reports would-run in dry-run.
        let actions = cli_sync("definitely-not-a-real-bin-ao", &["mcp".to_string()], true).unwrap();
        assert_eq!(actions[0].kind, "would-run");
        assert_eq!(actions[0].detail.as_deref(), Some("definitely-not-a-real-bin-ao mcp"));
    }

    #[test]
    fn sync_reports_add_on_success() {
        let dir = std::env::temp_dir().join(format!("ao-harness-mcpcli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = echo_bin(&dir, "fake-mcp", "#!/bin/sh\nexit 0\n");
        let actions = cli_sync(bin.to_str().unwrap(), &["mcp".into(), "add".into()], false).unwrap();
        assert_eq!(actions[0].kind, "add");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_failure_maps_to_nothing() {
        let dir = std::env::temp_dir().join(format!("ao-harness-mcpcli-rm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let failing = echo_bin(&dir, "fake-mcp-fail", "#!/bin/sh\nexit 3\n");
        let actions = cli_remove(failing.to_str().unwrap(), &["mcp".into(), "remove".into()], false);
        assert_eq!(actions[0].kind, "nothing");
        assert_eq!(actions[0].detail.as_deref(), Some("not registered"));
        // Missing binary → same outcome.
        let actions = cli_remove("definitely-not-a-real-bin-ao", &["mcp".into()], false);
        assert_eq!(actions[0].kind, "nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
