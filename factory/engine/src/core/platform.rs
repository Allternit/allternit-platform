//! Small OS seams the engine and the pane engine share, so Windows support
//! lives in one place instead of a `cfg` at every call site.

use std::path::PathBuf;

/// The user's home folder: `$HOME` when set and non-empty (every platform,
/// so tests and wrappers that set it keep working), else on Windows
/// `%USERPROFILE%`, then `%HOMEDRIVE%%HOMEPATH%`. `None` when nothing is set.
/// Unix behaviour is exactly the old `HOME` read.
pub fn user_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(home));
    }
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()) {
            return Some(PathBuf::from(profile));
        }
        if let (Some(drive), Some(path)) = (
            std::env::var_os("HOMEDRIVE").filter(|h| !h.is_empty()),
            std::env::var_os("HOMEPATH").filter(|h| !h.is_empty()),
        ) {
            let mut joined = drive;
            joined.push(path);
            return Some(PathBuf::from(joined));
        }
    }
    None
}

/// [`user_home`], or `fallback` when no home is known.
pub fn user_home_or(fallback: impl Into<PathBuf>) -> PathBuf {
    user_home().unwrap_or_else(|| fallback.into())
}

/// A shell that runs one command line: `/bin/sh -c <script>` on Unix,
/// `cmd.exe /d /s /c <script>` on Windows (`%COMSPEC%` when set).
pub fn shell_command(script: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let comspec = std::env::var_os("COMSPEC")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "cmd.exe".into());
        let mut command = std::process::Command::new(comspec);
        command.args(["/d", "/s", "/c", script]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg(script);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_command_runs_a_line() {
        let out = shell_command("echo hi").output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }
}
