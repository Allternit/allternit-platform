//! The admin helper's allow-list (contract D5, "privileged helper model").
//!
//! The probe never runs as root. After the one-time setup in
//! [`crate::elevation`], actions that need root go through one small helper
//! binary (`allternit-host-helper`) that runs as root. This module is the
//! only thing that helper will do:
//!
//! | argv | does |
//! |---|---|
//! | `version` | prints [`HELPER_VERSION`] |
//! | `service start\|stop\|restart <provider> <unit>` | the fixed service command for that provider |
//! | `updates <source> <id>...` | installs exactly those waiting updates |
//! | `clean <item>` | one admin clean-up item from [`crate::actions::clean_items`] |
//! | `port-free <port>` | stops the processes listening on that port |
//!
//! There is no generic command, no shell and no snippet path: a saved
//! snippet is a user script, so it never runs through the helper.
//!
//! The helper re-validates every request on its own side, whoever sent it
//! (the sudoers drop-in, the launchd socket or the Windows task only decide
//! who may ask): the argv shape is parsed strictly ([`HelperRequest::parse`]),
//! and the target is looked up again on the host ([`handle`]) before any
//! command is built. Names the host did not report never reach a command.

use std::io::{BufRead, Read, Write};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::actions::{
    service_steps, update_step, Executor, ServiceOp, CLEAN_TIMEOUT, KILL_TIMEOUT, SERVICE_TIMEOUT,
    UPDATE_TIMEOUT,
};
use crate::cmd::{CmdOutput, RunError};
use crate::logs::{valid_name, valid_provider};
use crate::services::ServiceProvider;
use crate::updates::UpdateSource;
use crate::OsFamily;

/// Bumped whenever the allow-list or the helper's behaviour changes; an
/// installed helper that answers another version is "outdated".
pub const HELPER_VERSION: &str = "1";

/// File name of the helper binary (built from `src/bin/allternit-host-helper.rs`).
pub const HELPER_BIN: &str = if cfg!(windows) {
    "allternit-host-helper.exe"
} else {
    "allternit-host-helper"
};

/// Exit code for a request the allow-list refused (sysexits `EX_USAGE`).
pub const EXIT_REFUSED: i32 = 64;
/// Exit code when the target is not on this host (sysexits `EX_NOINPUT`).
pub const EXIT_NOT_FOUND: i32 = 66;
/// Exit code when the helper is not running as root/admin (`EX_NOPERM`).
pub const EXIT_NOT_ROOT: i32 = 77;

const MAX_UPDATE_IDS: usize = 500;
const MAX_ID_LEN: usize = 200;

/// One request the helper accepts. Anything else is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperRequest {
    Version,
    Service {
        op: ServiceOp,
        provider: ServiceProvider,
        unit: String,
    },
    UpdatesInstall {
        source: UpdateSource,
        ids: Vec<String>,
    },
    Clean {
        item: String,
    },
    PortFree {
        port: u16,
    },
}

/// Why the helper refused or failed a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HelperError {
    /// Off the allow-list or a bad argument shape.
    #[error("refused by the admin helper: {0}")]
    Refused(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("the admin helper is not running as root/admin")]
    NotRoot,
}

impl HelperError {
    pub fn exit_code(&self) -> i32 {
        match self {
            HelperError::Refused(_) => EXIT_REFUSED,
            HelperError::NotFound(_) => EXIT_NOT_FOUND,
            HelperError::NotRoot => EXIT_NOT_ROOT,
        }
    }
}

fn refused(msg: impl Into<String>) -> HelperError {
    HelperError::Refused(msg.into())
}

pub fn provider_arg(p: ServiceProvider) -> &'static str {
    match p {
        ServiceProvider::Systemd => "systemd",
        ServiceProvider::Supervisord => "supervisord",
        ServiceProvider::Docker => "docker",
        ServiceProvider::Launchd => "launchd",
        ServiceProvider::Scm => "scm",
    }
}

fn parse_provider(s: &str) -> Option<ServiceProvider> {
    [
        ServiceProvider::Systemd,
        ServiceProvider::Supervisord,
        ServiceProvider::Docker,
        ServiceProvider::Launchd,
        ServiceProvider::Scm,
    ]
    .into_iter()
    .find(|p| provider_arg(*p) == s)
}

pub fn source_arg(s: UpdateSource) -> &'static str {
    match s {
        UpdateSource::Apt => "apt",
        UpdateSource::Dnf => "dnf",
        UpdateSource::Softwareupdate => "softwareupdate",
        UpdateSource::Windows => "windows",
    }
}

fn parse_source(s: &str) -> Option<UpdateSource> {
    [
        UpdateSource::Apt,
        UpdateSource::Dnf,
        UpdateSource::Softwareupdate,
        UpdateSource::Windows,
    ]
    .into_iter()
    .find(|x| source_arg(*x) == s)
}

fn parse_op(s: &str) -> Option<ServiceOp> {
    [ServiceOp::Start, ServiceOp::Stop, ServiceOp::Restart]
        .into_iter()
        .find(|o| o.verb() == s)
}

/// Package / product ids: letters, digits and `._+:~-`, plus inner spaces
/// for macOS product labels; never a leading `-`.
fn valid_update_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s == s.trim()
        && !s.starts_with('-')
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '+' | ':' | '~' | '-' | ' ' | ',' | '(' | ')')
        })
}

impl HelperRequest {
    /// The argv sent to the helper (after the binary path).
    pub fn to_argv(&self) -> Vec<String> {
        match self {
            HelperRequest::Version => vec!["version".into()],
            HelperRequest::Service { op, provider, unit } => vec![
                "service".into(),
                op.verb().into(),
                provider_arg(*provider).into(),
                unit.clone(),
            ],
            HelperRequest::UpdatesInstall { source, ids } => {
                let mut v = vec!["updates".to_string(), source_arg(*source).to_string()];
                v.extend(ids.iter().cloned());
                v
            }
            HelperRequest::Clean { item } => vec!["clean".into(), item.clone()],
            HelperRequest::PortFree { port } => vec!["port-free".into(), port.to_string()],
        }
    }

    /// How the request reads in a change record.
    pub fn display(&self) -> String {
        let mut s = HELPER_BIN.trim_end_matches(".exe").to_string();
        for a in self.to_argv() {
            s.push(' ');
            s.push_str(&a);
        }
        s
    }

    /// How long the helper may take for this request.
    pub fn timeout(&self) -> Duration {
        match self {
            HelperRequest::Version => Duration::from_secs(5),
            HelperRequest::Service { .. } => SERVICE_TIMEOUT,
            HelperRequest::UpdatesInstall { .. } => UPDATE_TIMEOUT,
            HelperRequest::Clean { .. } => CLEAN_TIMEOUT,
            HelperRequest::PortFree { .. } => KILL_TIMEOUT,
        }
    }

    /// Strict parse of the helper's argv. Every shape is exact: a missing,
    /// extra or malformed argument is refused.
    pub fn parse<S: AsRef<str>>(argv: &[S]) -> Result<HelperRequest, HelperError> {
        let a: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
        if a.iter().any(|s| s.chars().any(char::is_control)) {
            return Err(refused("arguments have control characters"));
        }
        match a.as_slice() {
            ["version"] => Ok(HelperRequest::Version),
            ["service", op, provider, unit] => {
                let op = parse_op(op)
                    .ok_or_else(|| refused(format!("unknown service operation {op:?}")))?;
                let provider = parse_provider(provider)
                    .ok_or_else(|| refused(format!("unknown service provider {provider:?}")))?;
                let ok = match provider {
                    ServiceProvider::Scm => valid_provider(unit),
                    _ => valid_name(unit),
                };
                if !ok {
                    return Err(refused(format!(
                        "service unit {unit:?} cannot be passed safely"
                    )));
                }
                Ok(HelperRequest::Service {
                    op,
                    provider,
                    unit: unit.to_string(),
                })
            }
            ["updates", source, ids @ ..] => {
                let source = parse_source(source)
                    .ok_or_else(|| refused(format!("unknown update source {source:?}")))?;
                if ids.is_empty() {
                    return Err(refused("updates needs at least one id"));
                }
                if ids.len() > MAX_UPDATE_IDS {
                    return Err(refused("too many update ids"));
                }
                if let Some(bad) = ids.iter().find(|id| !valid_update_id(id)) {
                    return Err(refused(format!(
                        "update id {bad:?} cannot be passed safely"
                    )));
                }
                Ok(HelperRequest::UpdatesInstall {
                    source,
                    ids: ids.iter().map(|s| s.to_string()).collect(),
                })
            }
            ["clean", item] => {
                let known = crate::actions::clean_items()
                    .iter()
                    .any(|i| i.id == *item && i.needs_elevation);
                if !known {
                    return Err(refused(format!("{item:?} is not an admin clean-up item")));
                }
                Ok(HelperRequest::Clean {
                    item: item.to_string(),
                })
            }
            ["port-free", port] => {
                let port: u16 = port
                    .parse()
                    .ok()
                    .filter(|p| *p > 0 && port.chars().all(|c| c.is_ascii_digit()))
                    .ok_or_else(|| refused(format!("{port:?} is not a port number")))?;
                Ok(HelperRequest::PortFree { port })
            }
            [] => Err(refused("no request")),
            [first, ..] => Err(refused(format!(
                "{first:?} is not on the admin helper's list"
            ))),
        }
    }
}

/// Run one request on this host. `exec` must be running as root/admin (the
/// helper process); the target is re-checked against what the host reports.
pub fn handle<E: Executor>(exec: &E, req: &HelperRequest) -> Result<CmdOutput, HelperError> {
    if matches!(req, HelperRequest::Version) {
        return Ok(CmdOutput {
            code: Some(0),
            stdout: format!("{HELPER_VERSION}\n"),
            stderr: String::new(),
        });
    }
    if !exec.elevated() {
        return Err(HelperError::NotRoot);
    }
    let os = exec
        .os()
        .ok_or_else(|| refused(format!("actions on {}", std::env::consts::OS)))?;
    let steps: Vec<(String, Vec<String>, Duration)> = match req {
        HelperRequest::Version => unreachable!(),
        HelperRequest::Service { op, provider, unit } => {
            let list = exec.services().map_err(|e| refused(e.to_string()))?;
            let svc = list
                .iter()
                .find(|s| &s.unit == unit && s.provider == *provider)
                .ok_or_else(|| {
                    HelperError::NotFound(format!(
                        "no {} service {unit:?}",
                        provider_arg(*provider)
                    ))
                })?;
            let (steps, _) = service_steps(svc, *op).map_err(|e| refused(e.to_string()))?;
            steps
                .into_iter()
                .map(|s| (s.program, s.args, s.timeout))
                .collect()
        }
        HelperRequest::UpdatesInstall { source, ids } => {
            let waiting = exec.updates().map_err(|e| refused(e.to_string()))?;
            if let Some(missing) = ids
                .iter()
                .find(|id| !waiting.iter().any(|u| &u.id == *id && u.source == *source))
            {
                return Err(HelperError::NotFound(format!(
                    "{missing:?} is not a waiting {} update",
                    source_arg(*source)
                )));
            }
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let step = update_step(*source, &refs).map_err(|e| refused(e.to_string()))?;
            vec![(step.program, step.args, step.timeout)]
        }
        HelperRequest::Clean { item } => {
            let it = crate::actions::clean_items()
                .iter()
                .find(|i| &i.id == item && i.needs_elevation)
                .ok_or_else(|| refused(format!("{item:?} is not an admin clean-up item")))?;
            if it.os.is_some_and(|o| o != os) {
                return Err(refused(format!("clean-up item {item} is not for this OS")));
            }
            vec![(
                it.program.to_string(),
                it.args.iter().map(|a| a.to_string()).collect(),
                CLEAN_TIMEOUT,
            )]
        }
        HelperRequest::PortFree { port } => {
            let mut pids: Vec<u32> = exec
                .ports()
                .map_err(|e| refused(e.to_string()))?
                .into_iter()
                .filter(|p| p.port == *port)
                .filter_map(|p| p.pid)
                .collect();
            pids.sort_unstable();
            pids.dedup();
            if pids.is_empty() {
                return Err(HelperError::NotFound(format!(
                    "no process listens on port {port}"
                )));
            }
            if let Some(p) = pids.iter().find(|p| **p <= 1 || **p == exec.own_pid()) {
                return Err(refused(format!(
                    "port {port} is held by protected process {p}"
                )));
            }
            pids.iter()
                .map(|pid| {
                    let pid = pid.to_string();
                    match os {
                        OsFamily::Windows => (
                            "taskkill".to_string(),
                            vec!["/PID".into(), pid, "/T".into(), "/F".into()],
                            KILL_TIMEOUT,
                        ),
                        OsFamily::Linux | OsFamily::Macos => {
                            ("kill".to_string(), vec!["-TERM".into(), pid], KILL_TIMEOUT)
                        }
                    }
                })
                .collect()
        }
    };

    let mut out = CmdOutput {
        code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
    };
    for (program, args, timeout) in steps {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match exec.run(&program, &refs, timeout) {
            Ok(o) => {
                out.stdout.push_str(&o.stdout);
                out.stderr.push_str(&o.stderr);
                out.code = o.code;
                if o.code != Some(0) {
                    break;
                }
            }
            Err(e) => {
                out.stderr.push_str(&e.reason(&program));
                out.stderr.push('\n');
                out.code = Some(if matches!(e, RunError::Timeout) {
                    124
                } else {
                    127
                });
                break;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Socket protocol (macOS launchd helper, Windows task): one JSON line each way
// ---------------------------------------------------------------------------

/// One request line on the helper socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireRequest {
    pub argv: Vec<String>,
}

/// One answer line on the helper socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireReply {
    pub code: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

impl WireReply {
    pub fn into_output(self) -> CmdOutput {
        CmdOutput {
            code: Some(self.code),
            stdout: self.stdout,
            stderr: self.stderr,
        }
    }
}

const MAX_LINE: usize = 256 * 1024;

/// Serve one connection: read one request line, re-validate, run, answer.
/// Who may connect is decided by the channel itself: the macOS socket's
/// owner and mode, the Windows pipe's security descriptor.
pub fn serve_one<E: Executor, R: BufRead, W: Write>(
    exec: &E,
    mut reader: R,
    mut writer: W,
) -> std::io::Result<()> {
    let mut line = String::new();
    let n = reader.by_ref().take(MAX_LINE as u64).read_line(&mut line)?;
    let reply = if n == 0 {
        return Ok(());
    } else {
        answer(exec, &line)
    };
    let mut s = serde_json::to_string(&reply).unwrap_or_else(|_| "{\"code\":70}".into());
    s.push('\n');
    writer.write_all(s.as_bytes())?;
    writer.flush()
}

fn answer<E: Executor>(exec: &E, line: &str) -> WireReply {
    let fail = |e: HelperError| WireReply {
        code: e.exit_code(),
        stdout: String::new(),
        stderr: format!("{e}\n"),
    };
    let wire: WireRequest = match serde_json::from_str(line.trim()) {
        Ok(w) => w,
        Err(_) => return fail(refused("request is not valid JSON")),
    };
    match HelperRequest::parse(wire.argv.as_slice()).and_then(|req| handle(exec, &req)) {
        Ok(out) => WireReply {
            code: out.code.unwrap_or(1),
            stdout: out.stdout,
            stderr: out.stderr,
        },
        Err(e) => fail(e),
    }
}

/// Entry point of the `allternit-host-helper` binary. Returns the exit code.
///
/// `install` / `uninstall` / `serve` are the setup and daemon modes from
/// [`crate::elevation`]; anything else must parse as a [`HelperRequest`].
pub fn main(args: Vec<String>) -> i32 {
    // The helper runs as root: never trust the caller's PATH.
    #[cfg(unix)]
    std::env::set_var(
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    );

    match args.first().map(String::as_str) {
        Some("install") => return crate::elevation::helper_install_main(&args[1..]),
        Some("uninstall") => return crate::elevation::helper_uninstall_main(&args[1..]),
        Some("serve") => return crate::elevation::helper_serve_main(&args[1..]),
        _ => {}
    }
    let exec = crate::actions::SystemExecutor;
    match HelperRequest::parse(args.as_slice()).and_then(|req| handle(&exec, &req)) {
        Ok(out) => {
            print!("{}", out.stdout);
            eprint!("{}", out.stderr);
            out.code.unwrap_or(1)
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(argv: &[&str]) -> Result<HelperRequest, HelperError> {
        HelperRequest::parse(argv)
    }

    #[test]
    fn every_request_round_trips_through_argv() {
        let reqs = [
            HelperRequest::Version,
            HelperRequest::Service {
                op: ServiceOp::Restart,
                provider: ServiceProvider::Systemd,
                unit: "nginx.service".into(),
            },
            HelperRequest::Service {
                op: ServiceOp::Stop,
                provider: ServiceProvider::Scm,
                unit: "Print Spooler".into(),
            },
            HelperRequest::UpdatesInstall {
                source: UpdateSource::Apt,
                ids: vec!["openssl".into(), "libssl3".into()],
            },
            HelperRequest::Clean {
                item: "apt-cache".into(),
            },
            HelperRequest::PortFree { port: 8080 },
        ];
        for r in reqs {
            assert_eq!(HelperRequest::parse(r.to_argv().as_slice()).unwrap(), r);
        }
    }

    #[test]
    fn off_list_requests_are_refused() {
        for argv in [
            &[][..],
            &["sh", "-c", "id"][..],
            &["exec", "/bin/sh"][..],
            &["install", "--user", "root"][..],
            &["snippet", "abc"][..],
            &["service", "enable", "systemd", "nginx.service"][..],
            &["service", "stop", "initd", "nginx"][..],
            &["service", "stop", "systemd"][..],
            &["service", "stop", "systemd", "nginx.service", "extra"][..],
            &["service", "stop", "systemd", "--now"][..],
            &["service", "stop", "systemd", "a b"][..],
            &["service", "stop", "systemd", "x;rm -rf /"][..],
            &["updates", "apt"][..],
            &["updates", "brew", "wget"][..],
            &["updates", "apt", "-o"][..],
            &["updates", "apt", "pkg$(id)"][..],
            &["clean", "docker-prune"][..], // not an admin item
            &["clean", "/"][..],
            &["clean"][..],
            &["port-free", "0"][..],
            &["port-free", "+80"][..],
            &["port-free", "70000"][..],
            &["port-free", "80", "81"][..],
            &["version", "now"][..],
            &["service", "stop", "systemd", "nginx\n.service"][..],
        ] {
            let r = p(argv);
            assert!(
                matches!(r, Err(HelperError::Refused(_))),
                "{argv:?} gave {r:?}"
            );
        }
    }

    #[test]
    fn exit_codes_are_distinct() {
        assert_eq!(refused("x").exit_code(), EXIT_REFUSED);
        assert_eq!(
            HelperError::NotFound("x".into()).exit_code(),
            EXIT_NOT_FOUND
        );
        assert_eq!(HelperError::NotRoot.exit_code(), EXIT_NOT_ROOT);
    }
}
