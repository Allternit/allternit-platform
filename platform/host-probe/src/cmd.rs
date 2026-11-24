//! Shared command runner: no shell, a timeout, the child killed when it passes.
//!
//! Every module that shells out (services, ports, updates) goes through
//! [`run`], so a missing or slow tool is reported the same way everywhere.

use std::io::Read;

use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};


/// How long any one command may run unless a module says otherwise.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether one data source (a tool, a file) could be read, and why not when
/// it could not. A missing or slow tool is a status, never an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub source: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl SourceStatus {
    pub fn ok(source: &str) -> Self {
        SourceStatus { source: source.into(), available: true, reason: None }
    }
    pub fn unavailable(source: &str, reason: impl Into<String>) -> Self {
        SourceStatus { source: source.into(), available: false, reason: Some(reason.into()) }
    }
}

/// First non-empty line of `s`, trimmed; `no output` when there is none.
pub fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string()
}

#[derive(Debug)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum RunError {
    NotFound,
    Timeout,
    Io(String),
}

impl RunError {
    pub fn reason(&self, program: &str) -> String {
        match self {
            RunError::NotFound => format!("{program} not found"),
            RunError::Timeout => format!("{program} timed out"),
            RunError::Io(e) => format!("{program}: {e}"),
        }
    }
}

/// Run `program args` with no shell and a timeout; the child is killed when
/// the timeout passes.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> std::result::Result<CmdOutput, RunError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunError::NotFound
            } else {
                RunError::Io(e.to_string())
            }
        })?;

    fn drain<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    }
    let out_h = drain(child.stdout.take());
    let err_h = drain(child.stderr.take());

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Reader threads end when the pipes close; do not block on
                    // them if a grandchild still holds a pipe open.
                    return Err(RunError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(RunError::Io(e.to_string())),
        }
    };
    let stdout = out_h.join().unwrap_or_default();
    let stderr = err_h.join().unwrap_or_default();
    Ok(CmdOutput {
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_not_found() {
        let r = run("allternit-no-such-program-xyz", &[], Duration::from_secs(1));
        assert!(matches!(r, Err(RunError::NotFound)));
    }

    #[cfg(unix)]
    #[test]
    fn slow_command_times_out() {
        let t = Instant::now();
        let r = run("sleep", &["5"], Duration::from_millis(150));
        assert!(matches!(r, Err(RunError::Timeout)));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn runner_captures_output_without_a_shell() {
        let o = run("echo", &["a;b $HOME"], Duration::from_secs(2)).unwrap();
        assert_eq!(o.code, Some(0));
        assert_eq!(o.stdout.trim(), "a;b $HOME", "arguments are not interpreted");
    }
}
