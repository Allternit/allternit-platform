//! Running a verb: the exit-code table, the `--json` envelope, and the two
//! existing implementations every verb is wired to (the engine's maintenance
//! CLI and the pane engine).
//!
//! API.md §2: `0` ok · `1` refused by Gate · `2` not found · `3` transport
//! broken / engine missing · `4` timeout · `5` needs a person · `64` usage.
//! With `--json`, stdout is one JSON document; on error it is
//! `{"error":{"code","fact","action"}}`. `70` / `internal` is used for a failure
//! that fits none of those (requested as a contract addition in the F2 notes);
//! it is never used to hide a known class.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// API.md §2 exit codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Ok,
    Refused,
    NotFound,
    Transport,
    Timeout,
    NeedsPerson,
    Usage,
    Internal,
}

impl Code {
    pub fn exit(self) -> u8 {
        match self {
            Code::Ok => 0,
            Code::Refused => 1,
            Code::NotFound => 2,
            Code::Transport => 3,
            Code::Timeout => 4,
            Code::NeedsPerson => 5,
            Code::Usage => 64,
            Code::Internal => 70,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Code::Ok => "ok",
            Code::Refused => "refused",
            Code::NotFound => "not_found",
            Code::Transport => "transport",
            Code::Timeout => "timeout",
            Code::NeedsPerson => "needs_person",
            Code::Usage => "usage",
            Code::Internal => "internal",
        }
    }

    fn from_exit(code: i32) -> Code {
        match code {
            0 => Code::Ok,
            1 => Code::Refused,
            2 => Code::NotFound,
            3 => Code::Transport,
            4 => Code::Timeout,
            5 => Code::NeedsPerson,
            64 => Code::Usage,
            _ => Code::Internal,
        }
    }

    fn default_action(self) -> &'static str {
        match self {
            Code::Ok => "",
            Code::Refused => "Read the Gate's reason, then change the request or get it approved.",
            Code::NotFound => "Check the name or id; list what exists with the matching list/ps verb.",
            Code::Transport => "Start the engine (allternit-factory serve, or allternit-factory pane for agent panes) and retry.",
            Code::Timeout => "Retry, or allow more time.",
            Code::NeedsPerson => "A person has to answer or approve this (allternit-factory orchestration attention list).",
            Code::Usage => "Run the command with --help for its usage.",
            Code::Internal => "This is an engine failure; keep the stderr output and report it.",
        }
    }
}

/// Per-invocation settings shared by every verb.
pub struct Ctx {
    pub root: Option<PathBuf>,
    pub json: bool,
}

impl Ctx {
    pub fn new(root: Option<PathBuf>, json: bool) -> Self {
        Self { root, json }
    }

    /// The workspace root a native verb reads (`--root`, else the cwd).
    pub fn root_dir(&self) -> PathBuf {
        self.root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

/// Report a failure in the contract's shape and return its exit code.
pub fn fail(ctx: &Ctx, code: Code, fact: &str, action: Option<&str>) -> u8 {
    let action = action.unwrap_or_else(|| code.default_action());
    if ctx.json {
        println!(
            "{}",
            json!({ "error": { "code": code.name(), "fact": fact, "action": action } })
        );
    } else {
        eprintln!("error: {fact}");
        if !action.is_empty() {
            eprintln!("  {action}");
        }
    }
    code.exit()
}

/// Print a successful JSON result.
pub fn ok_json(value: Value) -> u8 {
    println!("{value}");
    0
}

/// A clap failure of this binary's own tree.
pub fn exit_usage(err: clap::Error, json: bool) -> u8 {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _ = err.print();
            0
        }
        _ => {
            if json {
                let text = err.to_string();
                let fact = text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("usage error")
                    .trim_start_matches("error: ")
                    .to_string();
                println!(
                    "{}",
                    json!({ "error": { "code": "usage", "fact": fact, "action": Code::Usage.default_action() } })
                );
            } else {
                let _ = err.print();
            }
            Code::Usage.exit()
        }
    }
}

/// `--dry-run` for a verb whose implementation has no dry run of its own:
/// nothing is executed; the exact command that would run is printed.
pub fn dry_run(ctx: &Ctx, target: &Target) -> u8 {
    let argv = target.display_argv(ctx);
    if ctx.json {
        ok_json(json!({ "dryRun": true, "wouldRun": argv, "changed": false }))
    } else {
        println!("dry run: would run: {}", shell_join(&argv));
        println!("nothing was changed");
        0
    }
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(|c| c.is_whitespace() || "'\"$`\\".contains(c)) {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Which existing implementation a verb runs, with that implementation's argv.
pub enum Target {
    /// The engine maintenance CLI (`allternit-factory internal core …`).
    Rails(Vec<String>),
    /// The pane engine's own argv (`allternit-factory pane …`).
    Pane(Vec<String>),
}

impl Target {
    fn display_argv(&self, ctx: &Ctx) -> Vec<String> {
        let mut argv = vec!["allternit-factory".to_string()];
        match self {
            Target::Rails(args) => {
                argv.extend(["internal".into(), "core".into()]);
                if let Some(root) = &ctx.root {
                    argv.extend(["--root".into(), root.display().to_string()]);
                }
                argv.extend(args.iter().cloned());
            }
            Target::Pane(args) => {
                argv.push("pane".into());
                argv.extend(args.iter().cloned());
            }
        }
        argv
    }
}

/// Run a verb's target and translate its outcome to the contract.
pub fn run(ctx: &Ctx, target: Target) -> u8 {
    run_shaped(ctx, target, None)
}

/// Like [`run`], but with `--json` the implementation's stdout is turned into
/// the contract's JSON document by `shape`.
pub fn run_shaped(ctx: &Ctx, target: Target, shape: Option<fn(&str) -> Value>) -> u8 {
    match shape {
        Some(f) => run_shaped_dyn(ctx, target, Some(&f)),
        None => run_shaped_dyn(ctx, target, None),
    }
}

/// [`run_shaped`] with a shaping closure (it may capture, e.g. the root).
pub fn run_shaped_dyn(ctx: &Ctx, target: Target, shape: Option<&dyn Fn(&str) -> Value>) -> u8 {
    match target {
        Target::Rails(args) if !ctx.json => run_rails_in_process(ctx.root.as_ref(), args, true),
        Target::Rails(args) => {
            let mut child = vec!["internal".to_string(), "verb-core".to_string()];
            if let Some(root) = &ctx.root {
                child.extend(["--root".into(), root.display().to_string()]);
            }
            child.extend(args);
            run_child(ctx, child, Classify::Table, shape)
        }
        Target::Pane(args) => {
            let mut child = vec!["pane".to_string()];
            child.extend(args);
            run_child(ctx, child, Classify::Pane, shape)
        }
    }
}

/// Hand the terminal to the pane engine in this process (TUI, attach, and the
/// raw `pane` group). Its own exit codes stand.
pub fn run_pane_interactive(args: Vec<String>) -> u8 {
    let mut argv: Vec<std::ffi::OsString> = vec!["allternit-factory pane".into()];
    argv.extend(args.into_iter().map(Into::into));
    match allternit_factory_pane::run(argv) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err}");
            if err.kind() == std::io::ErrorKind::ConnectionRefused
                || err.kind() == std::io::ErrorKind::NotFound
            {
                Code::Transport.exit()
            } else {
                Code::Internal.exit()
            }
        }
    }
}

/// Run the maintenance CLI in this process.
///
/// `table == false` is `internal core`: the old binary's exit codes
/// (Gate denial 2, other errors 1) so hooks and scripts that read them keep
/// working. `table == true` is a part verb: API.md §2 codes.
pub fn run_rails_in_process(root: Option<&PathBuf>, args: Vec<String>, table: bool) -> u8 {
    let mut argv = vec!["allternit-factory internal core".to_string()];
    if let Some(root) = root {
        argv.extend(["--root".to_string(), root.display().to_string()]);
    }
    argv.extend(args);
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("error: could not start the async runtime: {err}");
            return Code::Internal.exit();
        }
    };
    let result = runtime.block_on(allternit_factory_engine::cli::rails::run_args(argv));
    let Err(err) = result else { return 0 };
    if let Some(clap_err) = err.downcast_ref::<clap::Error>() {
        use clap::error::ErrorKind;
        let _ = clap_err.print();
        return match clap_err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
            _ if table => Code::Usage.exit(),
            _ => 2,
        };
    }
    if allternit_factory_engine::cli::rails::report_gate_error(&err) {
        return if table { Code::Refused.exit() } else { 2 };
    }
    if !table {
        // What `fn main() -> anyhow::Result<()>` printed before.
        eprintln!("Error: {err:?}");
        return 1;
    }
    eprintln!("error: {err:#}");
    classify_message(&format!("{err:#}")).exit()
}

#[derive(Clone, Copy)]
enum Classify {
    /// The child already speaks the table (`internal verb-core`).
    Table,
    /// The pane engine's legacy codes (0 ok, 1 failure, 2 usage, 3 pane dead,
    /// 4 timeout) plus its stderr wording.
    Pane,
}

fn run_child(
    ctx: &Ctx,
    args: Vec<String>,
    classify: Classify,
    shape: Option<&dyn Fn(&str) -> Value>,
) -> u8 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => return fail(ctx, Code::Internal, &format!("cannot locate allternit-factory: {err}"), None),
    };
    let mut command = Command::new(exe);
    command
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(if ctx.json { Stdio::piped() } else { Stdio::inherit() })
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return fail(ctx, Code::Internal, &format!("cannot run allternit-factory: {err}"), None),
    };

    // Stream stderr through live (long-running verbs report progress there)
    // while keeping the tail to classify a failure.
    let mut stderr = child.stderr.take().expect("piped stderr");
    let tee = std::thread::spawn(move || {
        let mut kept: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = std::io::stderr().write_all(&buf[..n]);
                    kept.extend_from_slice(&buf[..n]);
                    if kept.len() > 16 * 1024 {
                        kept.drain(..kept.len() - 16 * 1024);
                    }
                }
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    });
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => return fail(ctx, Code::Internal, &format!("waiting for allternit-factory: {err}"), None),
    };
    let err_text = tee.join().unwrap_or_default();

    let raw = status.code().unwrap_or(-1);
    let code = match classify {
        Classify::Table => Code::from_exit(raw),
        Classify::Pane => classify_pane(raw, &err_text),
    };

    if code == Code::Ok {
        if ctx.json {
            match shape {
                Some(shape) => println!("{}", shape(&out)),
                None => match serde_json::from_str::<Value>(out.trim()) {
                    Ok(value) => println!("{value}"),
                    Err(_) => println!("{}", json!({ "text": out })),
                },
            }
        }
        return 0;
    }
    if ctx.json {
        let fact = last_line(&err_text)
            .or_else(|| last_line(&out))
            .unwrap_or_else(|| format!("exited with status {raw}"));
        println!(
            "{}",
            json!({ "error": { "code": code.name(), "fact": fact, "action": code.default_action() } })
        );
    }
    code.exit()
}

fn last_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('{') && !l.starts_with('}') && !l.starts_with('"'))
        .map(|l| l.trim_start_matches("error: ").to_string())
}

fn classify_message(text: &str) -> Code {
    let lower = text.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("no such") || lower.contains("unknown dag")
        || lower.contains("does not exist")
    {
        Code::NotFound
    } else if lower.contains("timed out") || lower.contains("timeout") {
        Code::Timeout
    } else if lower.contains("connection refused") || lower.contains("not running") {
        Code::Transport
    } else {
        Code::Internal
    }
}

fn classify_pane(raw: i32, stderr: &str) -> Code {
    if raw == 0 {
        return Code::Ok;
    }
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not running") || lower.contains("connection refused")
        || lower.contains("server is not reachable")
    {
        return Code::Transport;
    }
    if lower.contains("spawn gate") || lower.contains("refus") {
        return Code::Refused;
    }
    if lower.contains("no session") || lower.contains("not found") || lower.contains("no such")
        || lower.contains("unknown session") || lower.contains("not readable")
    {
        return Code::NotFound;
    }
    match raw {
        2 => Code::Usage,
        3 => Code::NotFound, // `watch`: PANE-DEAD (agent or session gone)
        4 => Code::Timeout,
        _ => classify_message(stderr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_engine_down_is_transport() {
        assert_eq!(
            classify_pane(1, "error: ao engine is not running (socket /tmp/x.sock)\n"),
            Code::Transport
        );
    }

    #[test]
    fn pane_usage_and_missing_session() {
        assert_eq!(classify_pane(2, "usage: allternit-factory pane kill <slug>\n"), Code::Usage);
        assert_eq!(classify_pane(2, "error: no session ao-x in /h/state.json\n"), Code::NotFound);
        assert_eq!(classify_pane(4, "TIMEOUT after 5s\n"), Code::Timeout);
    }

    #[test]
    fn table_codes_round_trip() {
        for code in [
            Code::Ok,
            Code::Refused,
            Code::NotFound,
            Code::Transport,
            Code::Timeout,
            Code::NeedsPerson,
            Code::Usage,
            Code::Internal,
        ] {
            assert_eq!(Code::from_exit(code.exit() as i32), code);
        }
    }

    #[test]
    fn last_line_skips_json_noise() {
        assert_eq!(
            last_line("error: gate denied\n{\n  \"x\": 1\n}\n").as_deref(),
            Some("gate denied")
        );
    }
}
