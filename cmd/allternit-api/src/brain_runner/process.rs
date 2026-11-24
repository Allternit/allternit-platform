//! Running vendor CLIs: short commands (status, logout, `codex debug models`)
//! and one streamed chat turn. Every child runs in its own process group so a
//! cancel or timeout also stops whatever the CLI started.

use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::stream::{Outcome, Parser};
use super::vendors::{self, SignIn, Vendor};
use super::{protocol, Config};

const STDERR_TAIL: usize = 16 * 1024;

fn command(cfg: &Config, vendor: Vendor) -> Result<Command, String> {
    let program = vendors::binary(cfg, vendor).ok_or_else(|| format!("{} is not installed", vendor.binary_name()))?;
    let home = vendors::home(cfg, vendor);
    std::fs::create_dir_all(&home).map_err(|e| format!("could not make the {} home: {e}", vendor.id()))?;
    let mut cmd = Command::new(program);
    cmd.envs(vendors::env(cfg, vendor))
        .current_dir(&home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    Ok(cmd)
}

/// `pgid` is the child's pid taken at spawn: tokio forgets it once the child
/// is reaped, and the group can outlive the leader.
fn kill_group(pgid: Option<u32>, child: &mut Child) {
    #[cfg(unix)]
    if let Some(pgid) = pgid {
        // SAFETY: plain syscall on the group this child was made leader of.
        unsafe {
            libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pgid;
    let _ = child.start_kill();
}

/// Runs a short command; `(exit ok, stdout + stderr without ANSI)`.
pub(crate) async fn run_quick(cfg: &Config, vendor: Vendor, args: &[&str], limit: Duration) -> Result<(bool, String), String> {
    let mut cmd = command(cfg, vendor)?;
    cmd.args(args).stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", vendor.binary_name()))?;
    let pgid = child.id();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let read = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let read_out = async {
            if let Some(s) = stdout.as_mut() {
                let _ = s.read_to_end(&mut out).await;
            }
        };
        let read_err = async {
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_end(&mut err).await;
            }
        };
        tokio::join!(read_out, read_err);
        (out, err)
    };
    let result = tokio::time::timeout(limit, async {
        let (out, err) = read.await;
        (child.wait().await, out, err)
    })
    .await;
    kill_group(pgid, &mut child);
    match result {
        Ok((status, out, err)) => {
            let text = format!("{}\n{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
            Ok((status.map(|s| s.success()).unwrap_or(false), crate::cli_tools::strip_ansi(&text)))
        }
        Err(_) => Err(format!("{} did not answer in {} s", vendor.binary_name(), limit.as_secs())),
    }
}

pub(crate) async fn sign_in_state(cfg: &Config, vendor: Vendor) -> Result<SignIn, String> {
    let (ok, text) = run_quick(cfg, vendor, vendor.status_argv(), Duration::from_secs(20)).await?;
    Ok(vendors::parse_status(vendor, ok, &text))
}

/// Writes the tools-off codex catalog the first time a home needs it.
pub(crate) async fn ensure_codex_catalog(cfg: &Config) -> Result<(), String> {
    let path = vendors::codex_catalog_path(cfg);
    if path.is_file() {
        return Ok(());
    }
    let mut cmd = command(cfg, Vendor::Chatgpt)?;
    cmd.args(["debug", "models"]).stdin(Stdio::null()).stderr(Stdio::null());
    let output = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| "codex debug models timed out".to_string())?
        .map_err(|e| format!("codex debug models: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let start = text.find('{').ok_or("codex debug models printed no JSON")?;
    let catalog: Value = serde_json::from_str(text[start..].trim()).map_err(|e| format!("codex debug models: {e}"))?;
    let edited = vendors::codex_tools_off(catalog)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, edited.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// What a turn sends: frames (`turn_delta`, then `turn_done` or `error`).
pub(crate) struct TurnRequest {
    pub id: String,
    pub vendor: Vendor,
    pub model: String,
    pub system: String,
    pub history: Vec<vendors::Msg>,
}

async fn send(out: &mpsc::Sender<Value>, frame: Value) -> bool {
    out.send(frame).await.is_ok()
}

/// Runs one turn to the end, a cancel, or the timeout. Sends every frame
/// itself; on cancel (or a closed socket) it sends nothing and kills the CLI.
pub(crate) async fn run_turn(cfg: &Config, req: TurnRequest, out: mpsc::Sender<Value>, cancel: CancellationToken) {
    let id = req.id.as_str();
    let fail = |code: &str, message: &str| protocol::error(Some(id), code, message);

    if req.vendor == Vendor::Chatgpt {
        if !cfg.codex_catalog {
            send(&out, fail("bad_request", "ChatGPT turns need the tools-off catalog, which is turned off")).await;
            return;
        }
        // Signed-out codex retries ~10 times before failing: check first.
        match sign_in_state(cfg, Vendor::Chatgpt).await {
            Ok(SignIn::Out) => {
                send(&out, fail("login_expired", "Sign-in expired")).await;
                return;
            }
            Ok(SignIn::In { .. }) => {}
            Err(e) => {
                send(&out, fail("cli_failed", &e)).await;
                return;
            }
        }
        if let Err(e) = ensure_codex_catalog(cfg).await {
            send(&out, fail("cli_failed", &e)).await;
            return;
        }
    }

    let plan = match vendors::turn_plan(cfg, req.vendor, &req.model, &req.system, &req.history) {
        Ok(plan) => plan,
        Err(e) => {
            send(&out, fail("bad_request", &e)).await;
            return;
        }
    };
    let mut cmd = match command(cfg, req.vendor) {
        Ok(cmd) => cmd,
        Err(e) => {
            send(&out, fail("cli_failed", &e)).await;
            return;
        }
    };
    for (path, body) in &plan.files {
        if let Err(e) = std::fs::write(path, body) {
            send(&out, fail("cli_failed", &format!("could not write {}: {e}", path.display()))).await;
            return;
        }
    }
    cmd.args(&plan.args).stdin(if plan.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            send(&out, fail("cli_failed", &format!("could not start {}: {e}", req.vendor.binary_name()))).await;
            return;
        }
    };
    let pgid = child.id();

    if let (Some(text), Some(mut stdin)) = (plan.stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut tail: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_TAIL * 2 {
                    tail.drain(..tail.len() - STDERR_TAIL);
                }
            }
            crate::cli_tools::strip_ansi(&String::from_utf8_lossy(&tail))
        })
    });
    let Some(stdout) = child.stdout.take() else {
        kill_group(pgid, &mut child);
        send(&out, fail("cli_failed", "no output")).await;
        return;
    };

    let mut parser = Parser::new(req.vendor);
    let mut lines = BufReader::new(stdout).lines();
    let deadline = tokio::time::sleep(cfg.turn_timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                kill_group(pgid, &mut child);
                let _ = child.wait().await;
                return;
            }
            _ = &mut deadline => {
                kill_group(pgid, &mut child);
                let _ = child.wait().await;
                send(&out, fail("cli_failed", &format!("No reply within {} s", cfg.turn_timeout.as_secs()))).await;
                return;
            }
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    for text in parser.line(&line) {
                        if !send(&out, protocol::delta(id, &text)).await {
                            kill_group(pgid, &mut child);
                            let _ = child.wait().await;
                            return;
                        }
                    }
                }
                _ => break,
            }
        }
    }
    let status = tokio::select! {
        _ = cancel.cancelled() => { kill_group(pgid, &mut child); let _ = child.wait().await; return; }
        _ = &mut deadline => {
            kill_group(pgid, &mut child);
            let _ = child.wait().await;
            send(&out, fail("cli_failed", &format!("No reply within {} s", cfg.turn_timeout.as_secs()))).await;
            return;
        }
        status = child.wait() => status,
    };
    // Children the CLI left behind in its group go with it.
    kill_group(pgid, &mut child);
    let stderr = match stderr_task {
        Some(task) => tokio::time::timeout(Duration::from_secs(2), task).await.ok().and_then(Result::ok).unwrap_or_default(),
        None => String::new(),
    };
    let (tail, outcome) = parser.finish(status.map(|s| s.success()).unwrap_or(false), &stderr);
    if let Some(text) = tail {
        if !send(&out, protocol::delta(id, &text)).await {
            return;
        }
    }
    let frame = match outcome {
        Outcome::Done { input_tokens, output_tokens } => protocol::done(id, input_tokens, output_tokens),
        Outcome::Error { code, message } => fail(code, &message),
    };
    send(&out, frame).await;
}
