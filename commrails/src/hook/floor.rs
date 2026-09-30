//! Hard permission floor: commands that are denied for every spawned harness,
//! with or without a WIH, in every permission mode (bypass included).
//!
//! Same catastrophic list as the gizzi floor (audit S1): recursive rm of `/`,
//! `~` or `$HOME`; mkfs; dd onto a disk device; fork bombs; shutdown/reboot;
//! `git push --force` (or delete) to main/master; keychain dumps.

use std::path::Path;

use super::shell::{basename, effective_words, inner_script, parse, resolve, Target};

/// Returns a deny reason when `command` hits the hard floor.
pub fn check(command: &str, home: Option<&Path>) -> Option<String> {
    if let Some(reason) = fork_bomb(command) {
        return Some(reason);
    }
    check_depth(command, home, 0)
}

fn check_depth(command: &str, home: Option<&Path>, depth: usize) -> Option<String> {
    for seg in parse(command) {
        for r in &seg.redirects {
            if is_disk_device(r) {
                return Some(format!("hard floor: redirect onto raw disk device {r}"));
            }
        }
        let words = effective_words(&seg.words);
        let Some(first) = words.first() else { continue };
        if let Some(inner) = inner_script(words) {
            if depth < 4 {
                if let Some(reason) = check_depth(&inner, home, depth + 1) {
                    return Some(reason);
                }
            }
            continue;
        }
        let name = basename(first);
        let args = &words[1..];
        let reason = match name {
            "rm" => rm_floor(args, home),
            "find" => find_delete_floor(args, home),
            n if n == "mkfs" || n.starts_with("mkfs.") || n.starts_with("newfs") || n == "wipefs" => {
                Some(format!("hard floor: {n} formats a filesystem"))
            }
            "diskutil" => args
                .iter()
                .find(|a| {
                    let a = a.to_ascii_lowercase();
                    matches!(
                        a.as_str(),
                        "erasedisk" | "erasevolume" | "zerodisk" | "randomdisk" | "partitiondisk" | "secureerase" | "reformat"
                    )
                })
                .map(|verb| format!("hard floor: diskutil {verb} erases a disk")),
            "dd" => args
                .iter()
                .filter_map(|a| a.strip_prefix("of="))
                .find(|of| is_disk_device(of))
                .map(|of| format!("hard floor: dd onto raw disk device {of}")),
            "shutdown" | "reboot" | "halt" | "poweroff" => Some(format!("hard floor: {name} powers off the host")),
            "init" | "telinit" if args.first().map(|a| a == "0" || a == "6").unwrap_or(false) => {
                Some(format!("hard floor: {name} {} changes the host runlevel", args[0]))
            }
            "systemctl" | "loginctl"
                if args.iter().any(|a| matches!(a.as_str(), "poweroff" | "reboot" | "halt" | "kexec")) =>
            {
                Some(format!("hard floor: {name} powers off the host"))
            }
            "launchctl" if args.first().map(|a| a == "reboot").unwrap_or(false) => {
                Some("hard floor: launchctl reboot".to_string())
            }
            "osascript" if args.join(" ").to_ascii_lowercase().contains("shut down")
                || args.join(" ").to_ascii_lowercase().contains("restart") =>
            {
                Some("hard floor: osascript shut down/restart".to_string())
            }
            "git" => git_push_floor(args),
            "security" => args
                .iter()
                .find(|a| {
                    matches!(
                        a.as_str(),
                        "find-generic-password" | "find-internet-password" | "dump-keychain" | "export" | "find-key"
                    )
                })
                .map(|verb| format!("hard floor: security {verb} reads keychain secrets")),
            _ => None,
        };
        if reason.is_some() {
            return reason;
        }
    }
    None
}

fn fork_bomb(command: &str) -> Option<String> {
    let compact: String = command.chars().filter(|c| !c.is_whitespace()).collect();
    // name(){ name|name& };name  — any function name.
    let mut search = compact.as_str();
    while let Some(pos) = search.find("(){") {
        let name_start = search[..pos]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c == '.'))
            .map(|p| p + 1)
            .unwrap_or(0);
        let name = &search[name_start..pos];
        if !name.is_empty() {
            let body = format!("{name}(){{{name}|{name}&}};{name}");
            if compact.contains(&body) {
                return Some("hard floor: fork bomb".to_string());
            }
        }
        search = &search[pos + 3..];
    }
    None
}

fn is_disk_device(path: &str) -> bool {
    let Some(dev) = path.strip_prefix("/dev/") else { return false };
    ["disk", "rdisk", "sd", "hd", "nvme", "mmcblk", "xvd", "vd", "md"]
        .iter()
        .any(|p| dev.starts_with(p))
}

fn is_recursive_flag(arg: &str) -> bool {
    if arg == "--recursive" {
        return true;
    }
    arg.starts_with('-') && !arg.starts_with("--") && (arg.contains('r') || arg.contains('R'))
}

/// Paths whose recursive removal is catastrophic: `/`, the home dir, and the
/// top-level system/user roots.
fn is_catastrophic_root(word: &str, home: Option<&Path>) -> bool {
    let trimmed = word.trim_end_matches("/*").trim_end_matches('*');
    let literal = trimmed.trim_end_matches('/');
    if matches!(literal, "" | "~" | "$HOME" | "${HOME}" | "/." | "~/." ) {
        return true;
    }
    // Relative operands are scoped to the call's cwd, never a root.
    let absolute_like = trimmed.starts_with('/')
        || trimmed.starts_with('~')
        || trimmed.starts_with("$HOME")
        || trimmed.starts_with("${HOME}");
    if !absolute_like {
        return false;
    }
    let resolved = match resolve(if trimmed.is_empty() { "/" } else { trimmed }, Path::new("/"), home) {
        Target::Path(p) => p,
        Target::Unresolved(_) => return false,
    };
    if resolved == Path::new("/") {
        return true;
    }
    if let Some(h) = home {
        if resolved == h {
            return true;
        }
    }
    const ROOTS: &[&str] = &[
        "/bin", "/sbin", "/etc", "/usr", "/var", "/lib", "/opt", "/boot", "/dev", "/proc", "/sys", "/root",
        "/home", "/Users", "/System", "/Library", "/Applications", "/private", "/Volumes",
    ];
    ROOTS.iter().any(|r| resolved == Path::new(r))
}

fn rm_floor(args: &[String], home: Option<&Path>) -> Option<String> {
    if args.iter().any(|a| a == "--no-preserve-root") {
        return Some("hard floor: rm --no-preserve-root".to_string());
    }
    let mut recursive = false;
    let mut flags_done = false;
    let mut targets = Vec::new();
    for a in args {
        if !flags_done && a == "--" {
            flags_done = true;
            continue;
        }
        if !flags_done && a.starts_with('-') && a.len() > 1 {
            recursive |= is_recursive_flag(a);
            continue;
        }
        targets.push(a);
    }
    if !recursive {
        return None;
    }
    targets
        .into_iter()
        .find(|t| is_catastrophic_root(t, home))
        .map(|t| format!("hard floor: recursive rm of {t}"))
}

fn find_delete_floor(args: &[String], home: Option<&Path>) -> Option<String> {
    if !args.iter().any(|a| a == "-delete") {
        return None;
    }
    args.iter()
        .take_while(|a| !a.starts_with('-'))
        .find(|a| is_catastrophic_root(a, home))
        .map(|a| format!("hard floor: find {a} -delete"))
}

fn git_push_floor(args: &[String]) -> Option<String> {
    // Skip global options (`-C dir`, `-c k=v`, `--git-dir=...`).
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-C" || a == "-c" || a == "--git-dir" || a == "--work-tree" || a == "--namespace" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        break;
    }
    if args.get(i).map(String::as_str) != Some("push") {
        return None;
    }
    let rest = &args[i + 1..];
    let force = rest.iter().any(|a| {
        a == "--force"
            || a.starts_with("--force-with-lease")
            || a == "--force-if-includes"
            || a == "--mirror"
            || (a.starts_with('-') && !a.starts_with("--") && a.contains('f'))
    });
    let delete = rest
        .iter()
        .any(|a| a == "--delete" || (a.starts_with('-') && !a.starts_with("--") && a.contains('d')));
    let refspecs: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    let protected_ref = |r: &str| {
        let r = r.trim_start_matches('+');
        let dst = r.rsplit(':').next().unwrap_or(r);
        let dst = dst.trim_start_matches("refs/heads/");
        dst == "main" || dst == "master"
    };
    let plus_protected = refspecs.iter().any(|r| r.starts_with('+') && protected_ref(r));
    let delete_colon = refspecs
        .iter()
        .any(|r| r.starts_with(':') && protected_ref(r));
    let names_protected = refspecs.iter().any(|r| protected_ref(r));
    if plus_protected || delete_colon || ((force || delete) && names_protected) {
        return Some("hard floor: force-push/delete of main/master".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Option<&'static Path> {
        Some(Path::new("/Users/u"))
    }

    fn denied(cmd: &str) -> bool {
        check(cmd, home()).is_some()
    }

    #[test]
    fn denies_catastrophic_rm() {
        for cmd in [
            "rm -rf /",
            "rm -rf ~",
            "rm -rf ~/",
            "rm -fr $HOME",
            "rm -rf \"$HOME\"",
            "rm -rf ${HOME}/",
            "rm -rf /*",
            "sudo rm -rf --no-preserve-root /",
            "rm -r -f /Users/u",
            "rm -R /usr",
            "cd /tmp && rm -rf ~",
            "echo $(rm -rf ~)",
            "bash -c 'rm -rf /'",
            "find / -delete",
        ] {
            assert!(denied(cmd), "expected deny: {cmd}");
        }
    }

    #[test]
    fn allows_ordinary_rm() {
        for cmd in ["rm -rf build", "rm -rf ./target", "rm ~/.cache/x", "rm -rf ~/scratch/tmp", "rm /", "rm -rf ./*", "rm -rf ."] {
            assert!(!denied(cmd), "expected allow: {cmd}");
        }
    }

    #[test]
    fn denies_disk_power_forkbomb_keychain() {
        for cmd in [
            "mkfs.ext4 /dev/sda1",
            "mkfs -t ext4 /dev/sdb",
            "diskutil eraseDisk JHFS+ X disk2",
            "dd if=/dev/zero of=/dev/disk2 bs=1m",
            "cat img > /dev/rdisk3",
            ":(){ :|:& };:",
            "bomb(){ bomb|bomb& }; bomb",
            "shutdown -h now",
            "sudo reboot",
            "systemctl poweroff",
            "security find-generic-password -s foo -w",
            "security dump-keychain -d login.keychain",
        ] {
            assert!(denied(cmd), "expected deny: {cmd}");
        }
    }

    #[test]
    fn git_push_force_to_main() {
        for cmd in [
            "git push --force origin main",
            "git push -f origin master",
            "git push origin +main",
            "git push --force-with-lease origin HEAD:main",
            "git -C repo push -f origin refs/heads/main",
            "git push origin :main",
            "git push --delete origin master",
        ] {
            assert!(denied(cmd), "expected deny: {cmd}");
        }
        for cmd in [
            "git push origin main",
            "git push -f origin feature/x",
            "git push --force-with-lease origin session/abc",
            "git push -u origin spawn-gate-hooks-no-yolo",
        ] {
            assert!(!denied(cmd), "expected allow: {cmd}");
        }
    }

    #[test]
    fn allows_everyday_commands() {
        for cmd in ["ls -la", "cargo test -p foo", "git status", "echo 'rm -rf /' > notes.txt", "dd if=a of=b"] {
            assert!(!denied(cmd), "expected allow: {cmd}");
        }
    }
}
