//! Allternit Factory host glue (an Allternit addition beside the herdr code).
//!
//! The pane engine is a library. The product binary `allternit-factory` runs it
//! as `allternit-factory pane …` and calls [`set_argv_prefix`] with `["pane"]`
//! first. Every place the engine re-executes itself (server daemon, client,
//! handoff import, sentinel watch) builds its command with [`self_command`], so
//! the prefix lands in front of herdr's own argv. Every place it hands its own
//! path to a child that will call it back with herdr's argv (`HERDR_BIN_PATH`
//! for agent hooks, plugins and custom commands) uses [`bin_path`], which, under
//! a prefixed host, points at a small launcher that adds the prefix.
//!
//! With no prefix set (the `allternit-factory-pane` dev/test binary), all of
//! this is exactly the upstream behavior: `current_exe` with herdr's argv.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static PREFIX: OnceLock<Vec<String>> = OnceLock::new();

/// File name of the launcher written under a prefixed host.
pub const LAUNCHER_NAME: &str = if cfg!(windows) {
    "allternit-factory-pane.cmd"
} else {
    "allternit-factory-pane"
};

/// Set the argv prefix the host binary needs before herdr's own argv
/// (`["pane"]` for `allternit-factory`). First call wins.
pub fn set_argv_prefix<I, S>(prefix: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let _ = PREFIX.set(prefix.into_iter().map(Into::into).collect());
}

pub(crate) fn argv_prefix() -> &'static [String] {
    PREFIX.get().map(Vec::as_slice).unwrap_or(&[])
}

/// `Command` for `exe` (this engine's binary) with the host prefix applied.
pub(crate) fn self_command(exe: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(exe);
    command.args(argv_prefix());
    command
}

/// The program string shown/used to re-run this engine from a shell
/// (`allternit-factory pane`), given argv[0].
pub(crate) fn shell_program(argv0: &str) -> String {
    let mut program = argv0.to_string();
    for part in argv_prefix() {
        program.push(' ');
        program.push_str(part);
    }
    program
}

/// The command a person types to reach this engine: `allternit-factory pane`
/// (also with no prefix: the `allternit-factory-pane` dev/test binary is never
/// shipped, so hints name the shipped command). Used in "start/attach/stop it
/// with …" hints so they name a command that exists.
pub(crate) fn program_name() -> String {
    if argv_prefix().is_empty() {
        "allternit-factory pane".to_string()
    } else {
        shell_program("allternit-factory")
    }
}

/// `<Factory home>/<rel>` (`~/.allternit/factory`, or `$ALLTERNIT_FACTORY_HOME`).
/// While only `legacy` exists (state written before the Factory, not moved yet
/// by the engine's one-time home move) that path is returned instead, so a
/// paired identity or an installed harness keeps working.
pub fn factory_path(rel: &str, legacy: Option<PathBuf>) -> PathBuf {
    let new = allternit_factory_engine::registry::factory_home().join(rel);
    match legacy {
        Some(old) if !new.exists() && old.exists() => old,
        _ => new,
    }
}

/// `$HOME/<rel>` (for the pre-Factory locations [`factory_path`] falls back to).
pub fn home_path(rel: &str) -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(rel)
}

/// A path a child process can execute with herdr's argv to reach this engine.
pub(crate) fn bin_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if argv_prefix().is_empty() {
        return Ok(exe);
    }
    let dir = launcher_dir()?;
    ensure_launcher(&dir.join(LAUNCHER_NAME), &exe, argv_prefix())
}

/// `~/.allternit/factory/bin`, the engine's state root (API.md §1).
fn launcher_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .ok_or_else(|| io::Error::other("HOME is not set; cannot place the pane launcher"))?;
    Ok(PathBuf::from(home).join(".allternit").join("factory").join("bin"))
}

fn launcher_script(exe: &Path, prefix: &[String]) -> String {
    if cfg!(windows) {
        let mut line = format!("@\"{}\"", exe.display());
        for part in prefix {
            line.push_str(&format!(" \"{part}\""));
        }
        format!("@echo off\r\n{line} %*\r\n")
    } else {
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let mut line = format!("exec {}", quote(&exe.display().to_string()));
        for part in prefix {
            line.push(' ');
            line.push_str(&quote(part));
        }
        format!("#!/bin/sh\n# Written by allternit-factory: runs its pane engine with herdr's argv.\n{line} \"$@\"\n")
    }
}

/// Write the launcher if it is missing or points at a different binary.
fn ensure_launcher(path: &Path, exe: &Path, prefix: &[String]) -> io::Result<PathBuf> {
    let script = launcher_script(exe, prefix);
    if std::fs::read_to_string(path).ok().as_deref() == Some(script.as_str()) {
        return Ok(path.to_path_buf());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, &script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn launcher_runs_the_host_with_its_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LAUNCHER_NAME);
        let exe = Path::new("/opt/it's here/allternit-factory");
        let out = ensure_launcher(&path, exe, &["pane".to_string()]).unwrap();
        let text = std::fs::read_to_string(out).unwrap();
        assert!(text.contains("exec '/opt/it'\\''s here/allternit-factory' 'pane' \"$@\""));
        // Rewriting with the same content is a no-op; a moved binary rewrites it.
        ensure_launcher(&path, exe, &["pane".to_string()]).unwrap();
        ensure_launcher(&path, Path::new("/new/allternit-factory"), &["pane".to_string()]).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("/new/allternit-factory"));
    }

    #[test]
    fn shell_program_without_prefix_is_argv0() {
        // PREFIX is process-global and unset in unit tests.
        assert_eq!(shell_program("ao"), "ao");
    }
}
