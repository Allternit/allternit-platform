//! Minimal POSIX-ish shell splitter for the spawn gate.
//!
//! This is NOT a shell. It exists so the PreToolUse hook can answer two
//! questions about a `Bash` tool call without executing it:
//!   1. does any simple command in it hit the catastrophic hard floor?
//!   2. which paths does it write to (for WIH lease coverage)?
//!
//! Anything it cannot understand is surfaced to the caller (unresolved
//! targets) so the caller can fail closed.

use std::path::{Component, Path, PathBuf};

/// One simple command: its words plus any output-redirection targets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Segment {
    pub words: Vec<String>,
    /// Targets of `>`, `>>`, `&>`, `>|`, `N>` redirections.
    pub redirects: Vec<String>,
    /// Sources of `<` input redirections (read, never written).
    pub inputs: Vec<String>,
}

/// Split `command` into simple commands. Command substitutions (`$(...)`,
/// backticks) and `sh -c '...'` bodies are parsed recursively and appended as
/// their own segments, so nothing hides inside a subshell.
pub fn parse(command: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    parse_into(command, &mut out, 0);
    out
}

// The end_word!/end_seg! macros reset state that the final expansion at EOF
// never reads again.
#[allow(unused_assignments)]
fn parse_into(command: &str, out: &mut Vec<Segment>, depth: usize) {
    if depth > 8 {
        return;
    }
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    let mut seg = Segment::default();
    let mut word = String::new();
    let mut word_started = false;
    let mut pending_redirect = false; // next word is an output-redirect target
    let mut pending_input = false; // next word is an input-redirect source
    let mut input_is_data = false; // ... a here-string (data, not a path)
    let mut pending_heredoc = false; // next word is a heredoc delimiter
    let mut heredocs: Vec<(String, bool)> = Vec::new(); // (delim, strip_tabs)
    let mut nested: Vec<String> = Vec::new();

    macro_rules! end_word {
        () => {
            if word_started {
                let w = std::mem::take(&mut word);
                if pending_redirect {
                    seg.redirects.push(w);
                    pending_redirect = false;
                } else if pending_input {
                    if !input_is_data {
                        seg.inputs.push(w);
                    }
                    pending_input = false;
                    input_is_data = false;
                } else if pending_heredoc {
                    let strip = w.starts_with('-');
                    let delim = w.trim_start_matches('-').to_string();
                    heredocs.push((delim, strip));
                    pending_heredoc = false;
                } else {
                    seg.words.push(w);
                }
                word_started = false;
            }
        };
    }
    macro_rules! end_seg {
        () => {
            end_word!();
            if !seg.words.is_empty() || !seg.redirects.is_empty() || !seg.inputs.is_empty() {
                out.push(std::mem::take(&mut seg));
            }
            pending_redirect = false;
            pending_input = false;
            input_is_data = false;
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                if i + 1 < chars.len() {
                    if chars[i + 1] != '\n' {
                        word.push(chars[i + 1]);
                        word_started = true;
                    }
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            '\'' => {
                word_started = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
                continue;
            }
            '"' => {
                word_started = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        word.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
                        let (inner, next) = take_balanced(&chars, i + 2);
                        nested.push(inner.clone());
                        word.push_str("$(");
                        word.push_str(&inner);
                        word.push(')');
                        i = next;
                        continue;
                    }
                    if chars[i] == '`' {
                        let (inner, next) = take_backtick(&chars, i + 1);
                        nested.push(inner);
                        i = next;
                        continue;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
                continue;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                let (inner, next) = take_balanced(&chars, i + 2);
                nested.push(inner.clone());
                word.push_str("$(");
                word.push_str(&inner);
                word.push(')');
                word_started = true;
                i = next;
                continue;
            }
            '`' => {
                let (inner, next) = take_backtick(&chars, i + 1);
                nested.push(inner);
                word_started = true;
                i = next;
                continue;
            }
            '\n' => {
                end_seg!();
                i += 1;
                // Skip heredoc bodies registered on the line just ended.
                for (delim, strip) in std::mem::take(&mut heredocs) {
                    loop {
                        if i >= chars.len() {
                            break;
                        }
                        let start = i;
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                        let line: String = chars[start..i].iter().collect();
                        i += 1;
                        let candidate = if strip { line.trim_start_matches('\t') } else { line.as_str() };
                        if candidate == delim {
                            break;
                        }
                    }
                }
                continue;
            }
            ';' | '|' | '&' | '(' | ')' => {
                // `&>` / `&>>` are redirects, `>&N` handled under '>'.
                if c == '&' && i + 1 < chars.len() && chars[i + 1] == '>' {
                    end_word!();
                    i += 2;
                    if i < chars.len() && chars[i] == '>' {
                        i += 1;
                    }
                    pending_redirect = true;
                    continue;
                }
                end_seg!();
                i += 1;
                continue;
            }
            '>' => {
                // `N>` — a bare fd number just before `>` is not a word.
                if word_started && word.chars().all(|ch| ch.is_ascii_digit()) {
                    word.clear();
                    word_started = false;
                } else {
                    end_word!();
                }
                i += 1;
                if i < chars.len() && (chars[i] == '>' || chars[i] == '|') {
                    i += 1;
                }
                // `>&2` / `>&-` duplicate a descriptor: not a file.
                if i < chars.len() && chars[i] == '&' {
                    i += 1;
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '-') {
                        i += 1;
                    }
                    continue;
                }
                pending_redirect = true;
                continue;
            }
            '<' => {
                end_word!();
                i += 1;
                if i + 1 < chars.len() && chars[i] == '<' && chars[i + 1] == '<' {
                    // here-string `<<<`: next word is data.
                    i += 2;
                    pending_input = true;
                    input_is_data = true;
                } else if i < chars.len() && chars[i] == '<' {
                    i += 1;
                    pending_heredoc = true;
                } else {
                    pending_input = true;
                }
                continue;
            }
            ' ' | '\t' | '\r' => {
                end_word!();
                i += 1;
                continue;
            }
            _ => {
                word.push(c);
                word_started = true;
                i += 1;
            }
        }
    }
    end_seg!();

    for inner in nested {
        parse_into(&inner, out, depth + 1);
    }
}

fn take_balanced(chars: &[char], mut i: usize) -> (String, usize) {
    let mut depth = 1;
    let mut inner = String::new();
    let mut in_single = false;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            }
        } else if c == '\'' {
            in_single = true;
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
            if depth == 0 {
                return (inner, i + 1);
            }
        }
        inner.push(c);
        i += 1;
    }
    (inner, i)
}

fn take_backtick(chars: &[char], mut i: usize) -> (String, usize) {
    let mut inner = String::new();
    while i < chars.len() && chars[i] != '`' {
        inner.push(chars[i]);
        i += 1;
    }
    (inner, i + 1)
}

/// Strip leading `VAR=value` assignments and transparent wrappers
/// (`sudo`, `env`, `command`, `nohup`, `time`, `nice`, `exec`, `xargs`, ...)
/// so the real program is `words[0]`.
pub fn effective_words(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        let Some(first) = rest.first() else { return rest };
        if is_assignment(first) || first == "{" || first == "!" {
            rest = &rest[1..];
            continue;
        }
        let name = basename(first);
        match name {
            "sudo" | "doas" => {
                rest = &rest[1..];
                // sudo flags; -u/-g/-C/-h/-p take a value.
                while let Some(w) = rest.first() {
                    if w == "--" {
                        rest = &rest[1..];
                        break;
                    }
                    if !w.starts_with('-') {
                        break;
                    }
                    let takes_value = matches!(w.as_str(), "-u" | "-g" | "-C" | "-h" | "-p" | "-U" | "-r" | "-t");
                    rest = &rest[1..];
                    if takes_value && !rest.is_empty() {
                        rest = &rest[1..];
                    }
                }
            }
            "env" => {
                rest = &rest[1..];
                while let Some(w) = rest.first() {
                    if w.starts_with('-') || is_assignment(w) {
                        let takes_value = matches!(w.as_str(), "-u" | "-C" | "-S");
                        rest = &rest[1..];
                        if takes_value && !rest.is_empty() {
                            rest = &rest[1..];
                        }
                    } else {
                        break;
                    }
                }
            }
            "command" | "builtin" | "exec" | "nohup" | "time" | "caffeinate" => {
                rest = &rest[1..];
                while rest.first().map(|w| w.starts_with('-')).unwrap_or(false) {
                    rest = &rest[1..];
                }
            }
            "nice" | "ionice" => {
                rest = &rest[1..];
                while let Some(w) = rest.first() {
                    if w == "-n" {
                        rest = &rest[rest.len().min(2)..];
                    } else if w.starts_with('-') {
                        rest = &rest[1..];
                    } else {
                        break;
                    }
                }
            }
            "timeout" | "gtimeout" => {
                rest = &rest[1..];
                while rest.first().map(|w| w.starts_with('-')).unwrap_or(false) {
                    rest = &rest[1..];
                }
                if !rest.is_empty() {
                    rest = &rest[1..]; // duration
                }
            }
            "xargs" => {
                rest = &rest[1..];
                while let Some(w) = rest.first() {
                    if !w.starts_with('-') {
                        break;
                    }
                    let takes_value = matches!(w.as_str(), "-I" | "-n" | "-P" | "-L" | "-s" | "-d" | "-E");
                    rest = &rest[1..];
                    if takes_value && !rest.is_empty() {
                        rest = &rest[1..];
                    }
                }
            }
            _ => return rest,
        }
    }
}

fn is_assignment(word: &str) -> bool {
    match word.find('=') {
        Some(eq) if eq > 0 => word[..eq]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !word[..eq].chars().next().unwrap().is_ascii_digit(),
        _ => false,
    }
}

pub fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// For `sh -c '<body>'` / `bash -c` / `zsh -c` / `eval`, return the inner
/// script so callers can recurse into it.
pub fn inner_script(words: &[String]) -> Option<String> {
    let first = words.first()?;
    match basename(first) {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" => {
            let mut iter = words.iter().skip(1);
            while let Some(w) = iter.next() {
                if w == "-c" || (w.starts_with('-') && !w.starts_with("--") && w.contains('c')) {
                    return iter.next().cloned();
                }
            }
            None
        }
        "eval" => Some(words[1..].join(" ")),
        _ => None,
    }
}

/// A write target: resolved to an absolute, lexically normalized path, or
/// left unresolved when it depends on something the gate cannot evaluate
/// (a variable other than `$HOME`, a command substitution).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Path(PathBuf),
    Unresolved(String),
    /// A program whose write effect the gate cannot scan (inline interpreter
    /// code, a script, a build tool, a custom executable). `literals` are the
    /// absolute / `../` / `~/` path literals found statically in its argv and
    /// inline code. Q25: allowed and recorded by default, denied when a
    /// literal lands outside the workspace or lease, or under a strict fence.
    UnknownEffect { what: String, literals: Vec<Target> },
}

/// Resolve a shell word used as a path.
pub fn resolve(word: &str, cwd: &Path, home: Option<&Path>) -> Target {
    let expanded = if let Some(rest) = word.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest).to_string_lossy().to_string(),
            None => return Target::Unresolved(word.to_string()),
        }
    } else if word == "~" {
        match home {
            Some(h) => h.to_string_lossy().to_string(),
            None => return Target::Unresolved(word.to_string()),
        }
    } else if let Some(rest) = word
        .strip_prefix("${HOME}")
        .or_else(|| word.strip_prefix("$HOME"))
    {
        match home {
            Some(h) => format!("{}{}", h.to_string_lossy(), rest),
            None => return Target::Unresolved(word.to_string()),
        }
    } else {
        word.to_string()
    };
    if expanded.contains('$') || expanded.contains('`') || expanded.starts_with('~') {
        return Target::Unresolved(word.to_string());
    }
    let path = Path::new(&expanded);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    Target::Path(normalize(&abs))
}

/// Lexical normalization (`.` and `..`), no filesystem access.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        out
    }
}

/// Paths a `Bash` command writes to. `cwd` is the tool call's working
/// directory; `cd X` segments move it for the segments that follow.
pub fn write_targets(command: &str, cwd: &Path, home: Option<&Path>) -> Vec<Target> {
    let mut targets = Vec::new();
    collect_write_targets(command, cwd, home, &mut targets, 0);
    targets
}

fn collect_write_targets(command: &str, cwd: &Path, home: Option<&Path>, out: &mut Vec<Target>, depth: usize) {
    let mut cwd = cwd.to_path_buf();
    for seg in parse(command) {
        for r in &seg.redirects {
            if is_benign_device(r) {
                continue;
            }
            out.push(resolve(r, &cwd, home));
        }
        let words = strip_keywords(effective_words(&seg.words));
        let Some(first) = words.first() else { continue };
        if let Some(inner) = inner_script(words) {
            if depth < 4 {
                collect_write_targets(&inner, &cwd, home, out, depth + 1);
            } else {
                out.push(unknown_effect(first, "nested too deep to scan", &words[1..], &cwd, home));
            }
            continue;
        }
        // A path-invoked program outside the system bin dirs is custom code:
        // `./rm` or `bin/ls` can do anything, whatever its name says.
        if first.contains('/') && !SYSTEM_BIN_DIRS.iter().any(|d| first.starts_with(d)) {
            out.push(unknown_effect(first, "custom executable", &words[1..], &cwd, home));
            continue;
        }
        let name = basename(first);
        let args = &words[1..];
        let operands = non_flag_args(args);
        match name {
            "cd" | "pushd" => {
                match operands.first() {
                    Some(dir) => match resolve(dir, &cwd, home) {
                        Target::Path(p) => cwd = p,
                        // An unresolvable cd makes every later relative path
                        // unresolvable; mark it so the caller fails closed.
                        Target::Unresolved(raw) => {
                            out.push(Target::Unresolved(format!("cd {raw}")));
                        }
                        // `resolve` never yields an effect; kept exhaustive.
                        Target::UnknownEffect { what, .. } => out.push(Target::Unresolved(format!("cd {what}"))),
                    },
                    None => {
                        if let Some(h) = home {
                            cwd = h.to_path_buf();
                        }
                    }
                }
            }
            "touch" | "mkdir" | "rm" | "rmdir" | "unlink" | "mv" | "tee" | "shred" | "mkfifo" => {
                for op in operands {
                    out.push(resolve(op, &cwd, home));
                }
            }
            "cp" | "ln" | "install" | "rsync" | "scp" | "ditto" => {
                if let Some(dir) = flag_value(args, &["-t", "--target-directory"]) {
                    out.push(resolve(&dir, &cwd, home));
                } else if let Some(last) = operands.last() {
                    if operands.len() >= 2 || name == "install" {
                        out.push(resolve(last, &cwd, home));
                    }
                }
            }
            "truncate" => {
                let mut skip_next = false;
                for a in args {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if a == "-s" || a == "-r" {
                        skip_next = true;
                        continue;
                    }
                    if !a.starts_with('-') {
                        out.push(resolve(a, &cwd, home));
                    }
                }
            }
            "chmod" | "chown" | "chgrp" | "chflags" | "xattr" | "setfacl" => {
                for op in operands.iter().skip(1) {
                    out.push(resolve(op, &cwd, home));
                }
            }
            "dd" => {
                for a in args {
                    if let Some(of) = a.strip_prefix("of=") {
                        out.push(resolve(of, &cwd, home));
                    }
                }
            }
            "sed" | "gsed" => {
                let in_place = args.iter().any(|a| {
                    a == "-i" || a.starts_with("-i") || a == "--in-place" || a.starts_with("--in-place=") || (a.starts_with('-') && !a.starts_with("--") && a.contains('i'))
                });
                if in_place {
                    let has_script_flag = args.iter().any(|a| a == "-e" || a == "-f" || a == "--expression");
                    let skip = if has_script_flag { 0 } else { 1 };
                    for op in operands.iter().skip(skip) {
                        out.push(resolve(op, &cwd, home));
                    }
                }
                // `w file` / `s///w file` write and `e` executes: not scannable.
                if sed_script_writes_or_executes(args) {
                    out.push(unknown_effect(name, "sed script writes (w) or executes (e)", args, &cwd, home));
                }
            }
            "find" => {
                if args.iter().any(|a| {
                    matches!(
                        a.as_str(),
                        "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fprint" | "-fprint0" | "-fprintf" | "-fls"
                    )
                }) {
                    out.push(unknown_effect(name, "find runs a command, deletes, or writes a file", args, &cwd, home));
                }
            }
            "sort" => {
                if let Some(o) = flag_value(args, &["-o", "--output"]) {
                    out.push(resolve(&o, &cwd, home));
                }
            }
            "uniq" => {
                if operands.len() >= 2 {
                    out.push(resolve(operands[1], &cwd, home));
                }
            }
            "curl" => {
                if let Some(o) = flag_value(args, &["-o", "--output"]) {
                    out.push(resolve(&o, &cwd, home));
                }
                if args.iter().any(|a| a == "-O" || a == "--remote-name") {
                    out.push(Target::Path(cwd.clone()));
                }
            }
            "wget" => {
                if let Some(o) = flag_value(args, &["-O", "--output-document", "-P", "--directory-prefix"]) {
                    out.push(resolve(&o, &cwd, home));
                } else {
                    out.push(Target::Path(cwd.clone()));
                }
            }
            "tar" | "bsdtar" | "gtar" => {
                // Mode letters ride either the first word (`tar xzf`) or a
                // short-flag cluster (`-xzf`).
                let is_mode_cluster = |a: &String| {
                    !a.starts_with("--")
                        && a.len() > 1
                        && a.trim_start_matches('-').chars().all(|ch| ch.is_ascii_alphabetic())
                        && a.contains('x')
                };
                let extracting = args.iter().any(|a| a == "--extract")
                    || args.first().map(is_mode_cluster).unwrap_or(false)
                    || args.iter().any(|a| a.starts_with('-') && is_mode_cluster(a));
                let creating_file = flag_value(args, &["-f", "--file"]);
                if extracting {
                    let dir = flag_value(args, &["-C", "--directory"]).unwrap_or_else(|| cwd.to_string_lossy().to_string());
                    out.push(resolve(&dir, &cwd, home));
                } else if let Some(f) = creating_file {
                    if args.iter().any(|a| a == "-c" || a == "--create" || (a.starts_with('-') && !a.starts_with("--") && a.contains('c'))) {
                        out.push(resolve(&f, &cwd, home));
                    }
                }
            }
            "unzip" => {
                let dir = flag_value(args, &["-d"]).unwrap_or_else(|| cwd.to_string_lossy().to_string());
                out.push(resolve(&dir, &cwd, home));
            }
            "git" => {
                // git writes into its work tree; `-C dir` moves it.
                let dir = flag_value(args, &["-C"]).unwrap_or_else(|| cwd.to_string_lossy().to_string());
                let sub = git_subcommand(args);
                if matches!(
                    sub.as_deref(),
                    Some("clone") | Some("init") | Some("worktree")
                ) {
                    // Destination is the last operand when given.
                    let ops = non_flag_args(args);
                    if ops.len() >= 3 {
                        out.push(resolve(ops.last().unwrap(), &cwd, home));
                    } else {
                        out.push(resolve(&dir, &cwd, home));
                    }
                } else if !matches!(
                    sub.as_deref(),
                    Some("status") | Some("log") | Some("diff") | Some("show") | Some("branch") | Some("rev-parse") | Some("ls-files") | Some("grep") | Some("blame") | Some("remote") | Some("config") | Some("describe") | Some("fetch") | Some("push") | Some("help") | Some("version") | None
                ) {
                    out.push(resolve(&dir, &cwd, home));
                }
            }
            n if READ_ONLY_COMMANDS.contains(&n) => {}
            // Interpreters with inline code or a script, build tools, and
            // any executable the gate has no scanner for: the write effect
            // is unknown, never "no writes".
            _ => out.push(unknown_effect(name, "no write-target scanner for this executable", args, &cwd, home)),
        }
    }
}

/// Programs the gate knows write nothing except through the redirects
/// already captured above. Everything else is an unresolved effect.
const READ_ONLY_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "less", "more", "grep", "egrep", "fgrep", "rg", "ag", "ack", "fd", "tree",
    "echo", "printf", "pwd", "wc", "cut", "tr", "diff", "cmp", "comm", "stat", "file", "which", "whereis",
    "type", "whoami", "id", "groups", "date", "cal", "true", "false", "test", "[", "[[", "]]", "basename",
    "dirname", "realpath", "readlink", "du", "df", "jq", "uname", "hostname", "ps", "pgrep", "printenv",
    "column", "nl", "od", "hexdump", "strings", "md5", "md5sum", "sha1sum", "sha256sum", "shasum", "cksum",
    "sleep", "seq", "expr", "lsof", "uptime", "sw_vers", "arch", "nproc", "getconf", "locale", "tty", "man",
    "export", "unset", "set", "alias", "unalias", "local", "declare", "readonly", "shift", "read", "wait",
    "exit", "return", "break", "continue", ":", "fi", "done", "esac", "for", "}", "in",
];

/// Absolute prefixes of system binaries; a program invoked by any other
/// path is treated as custom code.
const SYSTEM_BIN_DIRS: &[&str] = &[
    "/bin/", "/usr/bin/", "/sbin/", "/usr/sbin/", "/usr/local/bin/", "/opt/homebrew/bin/",
];

/// Shell control keywords that prefix a real command (`if cmd`, `then cmd`).
fn strip_keywords(mut words: &[String]) -> &[String] {
    while let Some(first) = words.first() {
        if matches!(first.as_str(), "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "time") {
            words = effective_words(&words[1..]);
        } else {
            break;
        }
    }
    words
}

/// A command whose write effect the gate cannot determine, with the path
/// literals statically visible in its arguments and inline code.
fn unknown_effect(program: &str, why: &str, args: &[String], cwd: &Path, home: Option<&Path>) -> Target {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(?:^|[\s"'`=(,:\[{<>])((?:/|\.\./|~/)[^\s"'`(),;\]}<>]+)"#).unwrap()
    });
    let mut literals = Vec::new();
    for arg in args {
        for cap in re.captures_iter(arg) {
            let lit = &cap[1];
            // `//host` is a URL tail, not a path; devices are benign.
            if lit.starts_with("//") || is_benign_device(lit) {
                continue;
            }
            let t = resolve(lit, cwd, home);
            if !literals.contains(&t) {
                literals.push(t);
            }
        }
    }
    Target::UnknownEffect {
        what: format!("{program} ({why}; write effect unresolved)"),
        literals,
    }
}

/// Whether a non-trivial sed script has a `w`/`W` write or an `e` execute
/// command or `s///w` / `s///e` flag.
fn sed_script_writes_or_executes(args: &[String]) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(^|[;{}\n]|[0-9$/,!]\s*)\s*[wWe](\s|$)").unwrap());
    let mut scripts: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-e" || a == "--expression" {
            if let Some(v) = args.get(i + 1) {
                scripts.push(v);
            }
            i += 2;
            continue;
        }
        if a == "-f" || a == "--file" {
            return true;
        }
        i += 1;
    }
    if scripts.is_empty() {
        if let Some(first) = non_flag_args(args).first() {
            scripts.push(first);
        }
    }
    scripts.iter().any(|s| re.is_match(s) || sed_substitute_flags(s).iter().any(|f| f.contains(['w', 'e'])))
}

/// Flag strings of every `s<d>re<d>repl<d>flags` command in a sed script,
/// for any delimiter `<d>` (backslash escapes respected).
fn sed_substitute_flags(script: &str) -> Vec<String> {
    let chars: Vec<char> = script.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let at_cmd = i == 0 || matches!(chars[i - 1], ';' | '\n' | '{' | '}' | ' ' | '/' | '!' | ',') || chars[i - 1].is_ascii_digit() || chars[i - 1] == '$';
        if chars[i] == 's' && at_cmd {
            if let Some(&d) = chars.get(i + 1) {
                if !d.is_alphanumeric() && !d.is_whitespace() && d != '\\' {
                    let mut j = i + 2;
                    let mut seen = 0;
                    while j < chars.len() && seen < 2 {
                        if chars[j] == '\\' {
                            j += 2;
                            continue;
                        }
                        if chars[j] == d {
                            seen += 1;
                        }
                        j += 1;
                    }
                    if seen == 2 {
                        let start = j;
                        while j < chars.len() && chars[j].is_ascii_alphanumeric() {
                            j += 1;
                        }
                        out.push(chars[start..j.min(chars.len())].iter().collect());
                        i = j;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

fn git_subcommand(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-C" || a == "-c" || a == "--git-dir" || a == "--work-tree" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(a.clone());
    }
    None
}

fn is_benign_device(target: &str) -> bool {
    matches!(
        target,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/fd/1" | "/dev/fd/2"
    )
}

/// Operands of a command (words that are not flags). `--` ends flags.
pub fn non_flag_args(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut flags_done = false;
    for a in args {
        if !flags_done && a == "--" {
            flags_done = true;
            continue;
        }
        if !flags_done && a.starts_with('-') && a.len() > 1 {
            continue;
        }
        out.push(a);
    }
    out
}

/// Value of the first matching flag (`-o x`, `-ox`, `--output=x`, `--output x`).
pub fn flag_value(args: &[String], names: &[&str]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        for name in names {
            if a == name {
                return args.get(i + 1).cloned();
            }
            if name.starts_with("--") {
                if let Some(v) = a.strip_prefix(&format!("{name}=")) {
                    return Some(v.to_string());
                }
            } else if name.len() == 2 && a.starts_with(name) && a.len() > 2 && !a.starts_with("--") {
                return Some(a[2..].to_string());
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(cmd: &str) -> Vec<Vec<String>> {
        parse(cmd).into_iter().map(|s| s.words).collect()
    }

    #[test]
    fn splits_on_operators_and_quotes() {
        assert_eq!(
            words("ls -la && echo 'a b' | wc -l; true"),
            vec![
                vec!["ls".to_string(), "-la".into()],
                vec!["echo".into(), "a b".into()],
                vec!["wc".into(), "-l".into()],
                vec!["true".into()],
            ]
        );
    }

    #[test]
    fn captures_redirect_targets() {
        let segs = parse("echo hi > out.txt 2>/dev/null; cat a >> b 2>&1");
        assert_eq!(segs[0].redirects, vec!["out.txt".to_string(), "/dev/null".into()]);
        assert_eq!(segs[1].redirects, vec!["b".to_string()]);
    }

    #[test]
    fn recurses_into_substitutions_and_sh_c() {
        let segs = parse("echo $(rm -rf /tmp/x) `touch y`");
        assert!(segs.iter().any(|s| s.words.first().map(String::as_str) == Some("rm")));
        assert!(segs.iter().any(|s| s.words.first().map(String::as_str) == Some("touch")));
        let t = write_targets("bash -c 'touch /etc/passwd'", Path::new("/w"), None);
        assert_eq!(t, vec![Target::Path(PathBuf::from("/etc/passwd"))]);
    }

    #[test]
    fn skips_heredoc_bodies() {
        let segs = parse("cat > notes.md <<'EOF'\nrm -rf /\nEOF\necho done");
        assert!(!segs.iter().any(|s| s.words.first().map(String::as_str) == Some("rm")));
        assert!(segs.iter().any(|s| s.words.first().map(String::as_str) == Some("echo")));
    }

    #[test]
    fn write_targets_resolve_cwd_home_and_cd() {
        let home = Path::new("/Users/u");
        let t = write_targets("touch a ~/b $HOME/c; cd /tmp && mkdir d", Path::new("/w/repo"), Some(home));
        assert_eq!(
            t,
            vec![
                Target::Path("/w/repo/a".into()),
                Target::Path("/Users/u/b".into()),
                Target::Path("/Users/u/c".into()),
                Target::Path("/tmp/d".into()),
            ]
        );
        let t = write_targets("cp /etc/hosts ./x && echo hi > $OUT", Path::new("/w"), None);
        assert_eq!(t[0], Target::Path("/w/x".into()));
        assert!(matches!(t[1], Target::Unresolved(_)));
    }

    #[test]
    fn read_only_commands_have_no_targets() {
        assert!(write_targets("ls -la /; cat /etc/hosts | grep x; git status", Path::new("/w"), None).is_empty());
    }

    #[test]
    fn effective_words_strips_wrappers() {
        let w: Vec<String> = ["sudo", "-u", "root", "env", "A=1", "nohup", "rm", "-rf", "/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(effective_words(&w)[0], "rm");
    }
}
