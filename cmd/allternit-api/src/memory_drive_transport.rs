//! Git transport for Memory Drives: the pre-receive hook that validates every
//! pushed commit BEFORE git publishes the ref, and the hook installer. The
//! hook runs this same API binary in a narrow validation mode that touches no
//! database, config or provider secret (see `run_pre_receive`).
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::memory_drive::MemoryDrive;

/// argv[1] that turns the API binary into the pre-receive validator.
pub const HOOK_ARG: &str = "memory-drive-pre-receive";
/// CGI variable carrying the drive's marker owner into the hook.
pub const OWNER_ENV: &str = "ALLTERNIT_DRIVE_OWNER";

/// Entry point for the hook. Reads `<old> <new> <ref>` lines from stdin and
/// exits non-zero (refusing the whole push) on the first invalid update.
pub fn run_pre_receive() -> i32 {
    let refuse = |msg: String| {
        eprintln!("Memory Drive refused this push: {msg}");
        1
    };
    let Ok(owner) = std::env::var(OWNER_ENV) else {
        return refuse("missing drive owner".into());
    };
    let repo = match std::env::var_os("GIT_DIR")
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) })
        .or_else(|| std::env::current_dir().ok())
        .and_then(|p| p.canonicalize().ok())
    {
        Some(p) => p,
        None => return refuse("repository not found".into()),
    };
    let drive = match MemoryDrive::new(repo, &owner, "main") {
        Ok(d) => d.with_quarantine_from_env(),
        Err(e) => return refuse(e.to_string()),
    };
    let stdin = std::io::stdin();
    let mut updates = 0;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return refuse("unreadable ref update".into()) };
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [old, new, refname] = parts[..] else { return refuse("malformed ref update".into()) };
        updates += 1;
        if updates > 1 {
            return refuse("push one branch (main) at a time".into());
        }
        if let Err(e) = drive.validate_push(old, new, refname) {
            return refuse(e.to_string());
        }
    }
    0
}

fn hook_script(exe: &Path) -> String {
    let exe = exe.to_string_lossy().replace('\\', "/").replace('\'', r"'\''");
    format!("#!/bin/sh\n# Installed by Allternit: validates Memory Drive pushes.\nexec '{exe}' {HOOK_ARG}\n")
}

/// Install (or refresh after an update moved the binary) the drive's
/// pre-receive hook. Fails closed: without a hook the caller must refuse
/// pushes.
pub fn ensure_hook(repo: &Path) -> std::io::Result<()> {
    ensure_hook_with(repo, &std::env::current_exe()?)
}

pub fn ensure_hook_with(repo: &Path, exe: &Path) -> std::io::Result<()> {
    let hooks = repo.join("hooks");
    std::fs::create_dir_all(&hooks)?;
    let path = hooks.join("pre-receive");
    let script = hook_script(exe);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
        let tmp = hooks.join(format!(".pre-receive.{}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// Whether a smart-HTTP request is a push (needs write access).
pub fn is_push(git_path: &str, query: &str) -> bool {
    git_path.ends_with("git-receive-pack")
        || query.split('&').any(|kv| kv == "service=git-receive-pack")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_detection_covers_advertisement_and_pack() {
        assert!(is_push("info/refs", "service=git-receive-pack"));
        assert!(is_push("git-receive-pack", ""));
        assert!(!is_push("info/refs", "service=git-upload-pack"));
        assert!(!is_push("git-upload-pack", ""));
    }

    #[test]
    fn hook_is_written_once_and_refreshed_when_binary_moves() {
        let dir = tempfile::tempdir().unwrap();
        ensure_hook_with(dir.path(), Path::new("/opt/a/allternit-api")).unwrap();
        let first = std::fs::read_to_string(dir.path().join("hooks/pre-receive")).unwrap();
        assert!(first.contains("'/opt/a/allternit-api' memory-drive-pre-receive"));
        ensure_hook_with(dir.path(), Path::new("/opt/b/it's/allternit-api")).unwrap();
        let second = std::fs::read_to_string(dir.path().join("hooks/pre-receive")).unwrap();
        assert!(second.contains(r"'/opt/b/it'\''s/allternit-api'"));
    }
}
