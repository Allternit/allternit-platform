//! One-time admin setup for the probe (contract D5, "privileged helper model").
//!
//! The probe stays unprivileged. Actions that need root go through the one
//! helper binary in [`crate::helper`], after a setup the user approves once:
//!
//! | OS | What is installed | How the probe reaches it |
//! |---|---|---|
//! | Linux | helper at `/usr/local/libexec/allternit/allternit-host-helper` + sudoers drop-in `/etc/sudoers.d/allternit-host-probe` (checked with `visudo -cf` before it goes live) | `sudo -n <helper> <request>` |
//! | macOS | helper at `/Library/PrivilegedHelperTools/com.allternit.host-helper` + LaunchDaemon `/Library/LaunchDaemons/com.allternit.host-helper.plist` | Unix socket `/var/run/com.allternit.host-helper.sock`, owned by the user, mode 0600 |
//! | Windows | helper under `%ProgramFiles%\Allternit\HostHelper` + a SYSTEM task `AllternitHostHelper` started at boot | named pipe `\\.\pipe\allternit-host-helper`, first instance owned by SYSTEM, DACL = SYSTEM + the installing user's SID only, remote clients rejected; the probe checks the pipe's owner and the server's image path before it sends |
//! | Root / Orgo | nothing: the runtime already runs as root (Orgo cloud computers do) | the engine runs commands itself |
//!
//! The flow for a client: [`plan`] (shown to the user before approval: every
//! file with its contents, every command, what the helper allows), then
//! [`install`] (one OS admin prompt: pkexec, the macOS admin dialog, or UAC;
//! a headless Linux server gets the exact `sudo` command for its terminal),
//! [`status`] (installed / outdated / missing / not needed) and [`uninstall`].
//!
//! The privileged part of install and uninstall is done by the helper binary
//! itself (`allternit-host-helper install|uninstall`, run elevated), from the
//! same [`install_ops`] / [`uninstall_ops`] lists the plan shows, so the plan
//! is exactly what gets installed. Nothing here stores a password.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cmd::{run, CmdOutput, RunError};
use crate::helper::{HelperRequest, HELPER_BIN, HELPER_VERSION};
use crate::OsFamily;

pub const LINUX_HELPER: &str = "/usr/local/libexec/allternit/allternit-host-helper";
pub const LINUX_HELPER_DIR: &str = "/usr/local/libexec/allternit";
pub const SUDOERS_PATH: &str = "/etc/sudoers.d/allternit-host-probe";
/// sudo skips files whose name contains a `.`, so the staging copy is never live.
pub const SUDOERS_STAGING: &str = "/etc/sudoers.d/.allternit-host-probe.new";

pub const MAC_LABEL: &str = "com.allternit.host-helper";
pub const MAC_HELPER: &str = "/Library/PrivilegedHelperTools/com.allternit.host-helper";
pub const MAC_HELPER_DIR: &str = "/Library/PrivilegedHelperTools";
pub const MAC_PLIST: &str = "/Library/LaunchDaemons/com.allternit.host-helper.plist";
pub const MAC_SOCKET: &str = "/var/run/com.allternit.host-helper.sock";

pub const WINDOWS_TASK: &str = "AllternitHostHelper";
/// The helper's named pipe. Created by the SYSTEM task with
/// `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a process that grabs the name first
/// makes the helper fail loudly instead of sharing the name.
pub const WINDOWS_PIPE: &str = r"\\.\pipe\allternit-host-helper";

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ElevationState {
    /// The runtime already runs as root/admin (Orgo cloud computers).
    NotNeeded,
    Installed,
    /// Installed, but it answers an older helper version: run setup again.
    Outdated,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelperKind {
    Root,
    Sudoers,
    LaunchDaemon,
    WindowsTask,
}

impl HelperKind {
    pub fn for_os(os: OsFamily) -> HelperKind {
        match os {
            OsFamily::Linux => HelperKind::Sudoers,
            OsFamily::Macos => HelperKind::LaunchDaemon,
            OsFamily::Windows => HelperKind::WindowsTask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevationStatus {
    pub state: ElevationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<HelperKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    pub expected_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// How the user approves the setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalMethod {
    /// Nothing to approve (already root).
    None,
    /// Linux desktop: the polkit password dialog.
    Pkexec,
    /// Headless Linux: the user runs `terminalCommand` (sudo asks for the password).
    Terminal,
    /// macOS: the system admin dialog (`osascript ... with administrator privileges`).
    MacAdminDialog,
    /// Windows: the UAC prompt.
    Uac,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub path: String,
    pub mode: String,
    pub contents: String,
}

/// What setup will do, shown to the user before they approve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    pub kind: HelperKind,
    /// `false` when the runtime is already root: no setup.
    pub needed: bool,
    pub summary: String,
    /// What the helper may do, in plain words (and what it never does).
    pub allows: Vec<String>,
    /// Every privileged step, in order.
    pub steps: Vec<String>,
    /// Every file written, with its contents.
    pub files: Vec<PlannedFile>,
    pub approval: ApprovalMethod,
    /// For [`ApprovalMethod::Terminal`]: the exact command to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_command: Option<String>,
    pub helper_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ElevationError {
    #[error("not supported on this host: {0}")]
    Unsupported(String),
    #[error("invalid setup input: {0}")]
    Invalid(String),
    /// Headless Linux: no password dialog. Run this in the computer's terminal.
    #[error("run this in the computer's terminal: {command}")]
    NeedsTerminal { command: String },
    #[error("the admin prompt was cancelled")]
    Cancelled,
    #[error("setup failed: {0}")]
    Failed(String),
}

// ---------------------------------------------------------------------------
// The host the setup looks at (fake in tests)
// ---------------------------------------------------------------------------

pub trait Host {
    fn os(&self) -> Option<OsFamily>;
    /// Running as root (Unix) or elevated admin (Windows).
    fn is_root(&self) -> bool;
    /// Login name and (Unix) uid of the user the runtime runs as.
    fn user(&self) -> Option<(String, Option<u32>)>;
    /// Windows: the SID (`S-1-5-21-...`) of the user the runtime runs as.
    /// Only that SID (and SYSTEM) may open the helper's pipe.
    fn user_sid(&self) -> Option<String>;
    fn exists(&self, path: &str) -> bool;
    /// A program on PATH (`pkexec`).
    fn has_program(&self, name: &str) -> bool;
    /// A graphical session (Linux `DISPLAY` / `WAYLAND_DISPLAY`).
    fn has_display(&self) -> bool;
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, RunError>;
    /// Ask the installed helper for its version.
    fn helper_version(&self) -> Result<String, String>;
}

/// The real host.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemHost;

impl Host for SystemHost {
    fn os(&self) -> Option<OsFamily> {
        OsFamily::current()
    }

    fn is_root(&self) -> bool {
        crate::actions::Executor::elevated(&crate::actions::SystemExecutor)
    }

    fn user(&self) -> Option<(String, Option<u32>)> {
        if cfg!(windows) {
            let name = std::env::var("USERNAME").ok().filter(|u| !u.is_empty())?;
            return Some((name, None));
        }
        let name = run("id", &["-un"], STATUS_TIMEOUT)
            .ok()?
            .stdout
            .trim()
            .to_string();
        let uid = run("id", &["-u"], STATUS_TIMEOUT)
            .ok()
            .and_then(|o| o.stdout.trim().parse().ok());
        (!name.is_empty()).then_some((name, uid))
    }

    fn user_sid(&self) -> Option<String> {
        #[cfg(windows)]
        {
            win::current_user_sid()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn has_program(&self, name: &str) -> bool {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
            .unwrap_or(false)
    }

    fn has_display(&self) -> bool {
        ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
    }

    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, RunError> {
        run(program, args, timeout)
    }

    fn helper_version(&self) -> Result<String, String> {
        match send_to_helper(&HelperRequest::Version, STATUS_TIMEOUT) {
            Ok(o) if o.code == Some(0) => Ok(o.stdout.trim().to_string()),
            Ok(o) => Err(crate::cmd::first_line(&format!("{}{}", o.stderr, o.stdout))),
            Err(e) => Err(e.reason("admin helper")),
        }
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn windows_dir(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.into())
}

fn windows_helper_dir() -> String {
    format!(
        "{}\\Allternit\\HostHelper",
        windows_dir("ProgramFiles", "C:\\Program Files")
    )
}

pub fn windows_helper() -> String {
    format!("{}\\{HELPER_BIN}", windows_helper_dir())
}

/// A string SID as written by `ConvertSidToStringSidW` (`S-1-5-21-...`).
/// It goes into the task command line and the pipe's SDDL, so nothing else
/// is allowed through.
pub fn valid_sid(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("S-1-") else {
        return false;
    };
    s.len() <= 184
        && !rest.is_empty()
        && rest.split('-').all(|part| {
            !part.is_empty() && part.len() <= 20 && part.bytes().all(|b| b.is_ascii_digit())
        })
}

/// The pipe's security descriptor: owner SYSTEM, protected DACL with SYSTEM
/// (full) and the installing user. The user's mask (0x12019b) is read/write
/// data without FILE_CREATE_PIPE_INSTANCE, so even the user cannot add a
/// server instance of the pipe.
pub fn windows_pipe_sddl(sid: &str) -> Option<String> {
    valid_sid(sid).then(|| format!("O:SYG:SYD:P(A;;GA;;;SY)(A;;0x12019b;;;{sid})"))
}

/// Where the helper binary lives once installed.
pub fn helper_path(os: OsFamily) -> String {
    match os {
        OsFamily::Linux => LINUX_HELPER.into(),
        OsFamily::Macos => MAC_HELPER.into(),
        OsFamily::Windows => windows_helper(),
    }
}

// ---------------------------------------------------------------------------
// Generated files
// ---------------------------------------------------------------------------

/// Login names that can go into sudoers, a plist and an icacls argument.
pub fn valid_user(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 64
        && u.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The sudoers drop-in: `user` may run the helper as root with these
/// subcommands only. sudo's `*` would match any arguments, so the helper
/// re-checks the exact shape itself ([`HelperRequest::parse`]).
pub fn sudoers_text(user: &str) -> String {
    let h = LINUX_HELPER;
    let mut cmds = vec![
        format!("{h} version"),
        format!("{h} service start *"),
        format!("{h} service stop *"),
        format!("{h} service restart *"),
        format!("{h} updates apt *"),
        format!("{h} updates dnf *"),
    ];
    for item in crate::actions::clean_items() {
        if item.needs_elevation && item.os.map_or(true, |o| o == OsFamily::Linux) {
            cmds.push(format!("{h} clean {}", item.id));
        }
    }
    cmds.push(format!("{h} port-free *"));
    format!(
        "# Allternit host-probe admin helper, version {HELPER_VERSION}. Written by the Computers console setup.\n\
# Lets {user} run one fixed helper as root without a password. The helper re-checks every\n\
# request against its own allow-list and never runs a shell or a script.\n\
# Remove it from the console (Settings > Admin access) or: sudo {h} uninstall\n\
Cmnd_Alias ALLTERNIT_HOST_HELPER = \\\n    {}\n\
Defaults!ALLTERNIT_HOST_HELPER env_reset, !requiretty\n\
{user} ALL=(root) NOPASSWD: ALLTERNIT_HOST_HELPER\n",
        cmds.join(", \\\n    ")
    )
}

/// The LaunchDaemon that keeps the macOS helper's socket open.
pub fn launchd_plist(uid: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{MAC_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{MAC_HELPER}</string>
        <string>serve</string>
        <string>--uid</string>
        <string>{uid}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/var/log/{MAC_LABEL}.log</string>
</dict>
</plist>
"#
    )
}

// ---------------------------------------------------------------------------
// Privileged steps (shown in the plan, run by `allternit-host-helper install`)
// ---------------------------------------------------------------------------

/// One privileged step. The plan shows these; the elevated helper runs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOp {
    MakeDir {
        path: String,
        mode: u32,
    },
    /// Copy the helper binary to a root-owned path (never user-writable).
    CopyHelper {
        from: String,
        to: String,
        mode: u32,
    },
    WriteFile {
        path: String,
        mode: u32,
        contents: String,
        secret: bool,
    },
    /// Write to `staging`, run `check` (with `{}` = staging path), and only
    /// move it to `path` when the check passes; otherwise remove it.
    WriteChecked {
        path: String,
        staging: String,
        mode: u32,
        contents: String,
        check: Vec<String>,
    },
    Run {
        program: String,
        args: Vec<String>,
        allow_fail: bool,
    },
    RemoveFile {
        path: String,
    },
}

impl InstallOp {
    pub fn display(&self) -> String {
        match self {
            InstallOp::MakeDir { path, mode } => {
                format!("create folder {path} (mode {mode:o}, owner root)")
            }
            InstallOp::CopyHelper { from, to, mode } => {
                format!("copy helper {from} to {to} (mode {mode:o}, owner root)")
            }
            InstallOp::WriteFile { path, mode, .. } => {
                format!("write {path} (mode {mode:o}, owner root)")
            }
            InstallOp::WriteChecked {
                path, mode, check, ..
            } => format!(
                "write {path} (mode {mode:o}, owner root) after `{}` passes",
                check.join(" ")
            ),
            InstallOp::Run { program, args, .. } => {
                let mut s = format!("run {program}");
                for a in args {
                    s.push(' ');
                    s.push_str(a);
                }
                s
            }
            InstallOp::RemoveFile { path } => format!("remove {path}"),
        }
    }

    fn planned_file(&self) -> Option<PlannedFile> {
        match self {
            InstallOp::WriteFile {
                path,
                mode,
                contents,
                secret,
            } => Some(PlannedFile {
                path: path.clone(),
                mode: format!("{mode:o}"),
                contents: if *secret {
                    "<secret made at install>".into()
                } else {
                    contents.clone()
                },
            }),
            InstallOp::WriteChecked {
                path,
                mode,
                contents,
                ..
            } => Some(PlannedFile {
                path: path.clone(),
                mode: format!("{mode:o}"),
                contents: contents.clone(),
            }),
            _ => None,
        }
    }
}

/// Who the helper serves and where the binary to install comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupInput {
    pub user: String,
    pub uid: Option<u32>,
    pub helper_from: String,
    /// Windows: the installing user's SID, the only non-SYSTEM account the
    /// helper's pipe lets in. Read from the unelevated runtime, because the
    /// UAC prompt may run the install as a different admin account.
    pub sid: Option<String>,
}

fn run_op(program: &str, args: &[&str], allow_fail: bool) -> InstallOp {
    InstallOp::Run {
        program: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        allow_fail,
    }
}

pub fn install_ops(os: OsFamily, input: &SetupInput) -> Result<Vec<InstallOp>, ElevationError> {
    if !valid_user(&input.user) {
        return Err(ElevationError::Invalid(format!(
            "user name {:?}",
            input.user
        )));
    }
    Ok(match os {
        OsFamily::Linux => vec![
            InstallOp::MakeDir {
                path: LINUX_HELPER_DIR.into(),
                mode: 0o755,
            },
            InstallOp::CopyHelper {
                from: input.helper_from.clone(),
                to: LINUX_HELPER.into(),
                mode: 0o755,
            },
            InstallOp::WriteChecked {
                path: SUDOERS_PATH.into(),
                staging: SUDOERS_STAGING.into(),
                mode: 0o440,
                contents: sudoers_text(&input.user),
                check: vec!["visudo".into(), "-cf".into(), "{}".into()],
            },
        ],
        OsFamily::Macos => {
            let uid = input.uid.ok_or_else(|| {
                ElevationError::Invalid("the user's uid is needed on macOS".into())
            })?;
            let target = format!("system/{MAC_LABEL}");
            vec![
                InstallOp::MakeDir {
                    path: MAC_HELPER_DIR.into(),
                    mode: 0o755,
                },
                InstallOp::CopyHelper {
                    from: input.helper_from.clone(),
                    to: MAC_HELPER.into(),
                    mode: 0o755,
                },
                run_op("launchctl", &["bootout", target.as_str()], true),
                InstallOp::WriteFile {
                    path: MAC_PLIST.into(),
                    mode: 0o644,
                    contents: launchd_plist(uid),
                    secret: false,
                },
                run_op("launchctl", &["bootstrap", "system", MAC_PLIST], false),
            ]
        }
        OsFamily::Windows => {
            let sid = input
                .sid
                .as_deref()
                .filter(|s| valid_sid(s))
                .ok_or_else(|| {
                    ElevationError::Invalid(format!("Windows user SID {:?}", input.sid))
                })?;
            let helper = windows_helper();
            let tr = format!("\"{helper}\" serve --sid {sid}");
            vec![
                InstallOp::MakeDir {
                    path: windows_helper_dir(),
                    mode: 0o755,
                },
                InstallOp::CopyHelper {
                    from: input.helper_from.clone(),
                    to: helper.clone(),
                    mode: 0o755,
                },
                run_op("schtasks", &["/End", "/TN", WINDOWS_TASK], true),
                run_op(
                    "schtasks",
                    &[
                        "/Create",
                        "/F",
                        "/TN",
                        WINDOWS_TASK,
                        "/RU",
                        "SYSTEM",
                        "/RL",
                        "HIGHEST",
                        "/SC",
                        "ONSTART",
                        "/TR",
                        tr.as_str(),
                    ],
                    false,
                ),
                run_op("schtasks", &["/Run", "/TN", WINDOWS_TASK], false),
            ]
        }
    })
}

pub fn uninstall_ops(os: OsFamily) -> Vec<InstallOp> {
    match os {
        OsFamily::Linux => vec![
            InstallOp::RemoveFile {
                path: SUDOERS_STAGING.into(),
            },
            InstallOp::RemoveFile {
                path: SUDOERS_PATH.into(),
            },
            InstallOp::RemoveFile {
                path: LINUX_HELPER.into(),
            },
        ],
        OsFamily::Macos => vec![
            run_op(
                "launchctl",
                &["bootout", format!("system/{MAC_LABEL}").as_str()],
                true,
            ),
            InstallOp::RemoveFile {
                path: MAC_PLIST.into(),
            },
            InstallOp::RemoveFile {
                path: MAC_SOCKET.into(),
            },
            InstallOp::RemoveFile {
                path: MAC_HELPER.into(),
            },
        ],
        OsFamily::Windows => vec![
            run_op("schtasks", &["/End", "/TN", WINDOWS_TASK], true),
            run_op("schtasks", &["/Delete", "/F", "/TN", WINDOWS_TASK], true),
            InstallOp::RemoveFile {
                path: windows_helper(),
            },
        ],
    }
}

/// File system and commands as root. [`RealRootFs`] is the real one.
pub trait RootFs {
    fn make_dir(&self, path: &str, mode: u32) -> std::io::Result<()>;
    fn copy(&self, from: &str, to: &str, mode: u32) -> std::io::Result<()>;
    fn write(&self, path: &str, mode: u32, contents: &str) -> std::io::Result<()>;
    fn rename(&self, from: &str, to: &str) -> std::io::Result<()>;
    fn remove(&self, path: &str) -> std::io::Result<()>;
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput, RunError>;
}

/// Run the steps in order; stop at the first one that fails.
pub fn apply_ops(
    ops: &[InstallOp],
    fs: &dyn RootFs,
    log: &mut dyn FnMut(&str),
) -> Result<(), String> {
    for op in ops {
        log(&op.display());
        let r: Result<(), String> = match op {
            InstallOp::MakeDir { path, mode } => {
                fs.make_dir(path, *mode).map_err(|e| format!("{path}: {e}"))
            }
            InstallOp::CopyHelper { from, to, mode } => {
                fs.copy(from, to, *mode).map_err(|e| format!("{to}: {e}"))
            }
            InstallOp::WriteFile {
                path,
                mode,
                contents,
                ..
            } => fs
                .write(path, *mode, contents)
                .map_err(|e| format!("{path}: {e}")),
            InstallOp::WriteChecked {
                path,
                staging,
                mode,
                contents,
                check,
            } => {
                fs.write(staging, *mode, contents)
                    .map_err(|e| format!("{staging}: {e}"))?;
                let args: Vec<&str> = check[1..]
                    .iter()
                    .map(|a| {
                        if a == "{}" {
                            staging.as_str()
                        } else {
                            a.as_str()
                        }
                    })
                    .collect();
                match fs.run(&check[0], &args) {
                    Ok(o) if o.code == Some(0) => {
                        fs.rename(staging, path).map_err(|e| format!("{path}: {e}"))
                    }
                    Ok(o) => {
                        let _ = fs.remove(staging);
                        Err(format!(
                            "{} rejected {path}; nothing was installed there: {}",
                            check[0],
                            crate::cmd::first_line(&format!("{}{}", o.stderr, o.stdout))
                        ))
                    }
                    Err(e) => {
                        let _ = fs.remove(staging);
                        Err(e.reason(&check[0]))
                    }
                }
            }
            InstallOp::Run {
                program,
                args,
                allow_fail,
            } => {
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                match fs.run(program, &refs) {
                    Ok(o) if o.code == Some(0) || *allow_fail => Ok(()),
                    Ok(o) => Err(format!(
                        "{program} exited with {}: {}",
                        o.code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "a signal".into()),
                        crate::cmd::first_line(&format!("{}{}", o.stderr, o.stdout))
                    )),
                    Err(_) if *allow_fail => Ok(()),
                    Err(e) => Err(e.reason(program)),
                }
            }
            InstallOp::RemoveFile { path } => match fs.remove(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{path}: {e}")),
                _ => Ok(()),
            },
        };
        r?;
    }
    Ok(())
}

/// Real root file system: files owned by root (uid 0, gid 0) on Unix.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealRootFs;

#[cfg(unix)]
fn own_root(path: &str, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::os::unix::fs::chown(path, Some(0), Some(0))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn own_root(_path: &str, _mode: u32) -> std::io::Result<()> {
    // Windows: the helper sits under Program Files (admin-only write); the
    // pipe's own security descriptor decides who may talk to it.
    Ok(())
}

impl RootFs for RealRootFs {
    fn make_dir(&self, path: &str, mode: u32) -> std::io::Result<()> {
        std::fs::create_dir_all(path)?;
        own_root(path, mode)
    }

    fn copy(&self, from: &str, to: &str, mode: u32) -> std::io::Result<()> {
        if Path::new(from) == Path::new(to) {
            return own_root(to, mode);
        }
        let staging = format!("{to}.new");
        std::fs::copy(from, &staging)?;
        own_root(&staging, mode)?;
        std::fs::rename(&staging, to)
    }

    fn write(&self, path: &str, mode: u32, contents: &str) -> std::io::Result<()> {
        let staging = format!("{path}.tmp");
        std::fs::write(&staging, contents)?;
        own_root(&staging, mode)?;
        std::fs::rename(&staging, path)
    }

    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    fn remove(&self, path: &str) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput, RunError> {
        run(program, args, Duration::from_secs(60))
    }
}

// ---------------------------------------------------------------------------
// Client side: status, plan, install, uninstall
// ---------------------------------------------------------------------------

fn status_of(
    state: ElevationState,
    kind: Option<HelperKind>,
    path: Option<String>,
) -> ElevationStatus {
    ElevationStatus {
        state,
        kind,
        helper_path: path,
        installed_version: None,
        expected_version: HELPER_VERSION.into(),
        reason: None,
    }
}

/// Installed / outdated / missing / not needed, from the real helper's answer.
pub fn status(host: &dyn Host) -> ElevationStatus {
    let Some(os) = host.os() else {
        let mut s = status_of(ElevationState::Unsupported, None, None);
        s.reason = Some(format!("admin setup on {}", std::env::consts::OS));
        return s;
    };
    if host.is_root() {
        let mut s = status_of(ElevationState::NotNeeded, Some(HelperKind::Root), None);
        s.reason = Some("The runtime already runs as root/admin (Allternit Cloud computers do), so no setup is needed.".into());
        return s;
    }
    let kind = HelperKind::for_os(os);
    let path = helper_path(os);
    let mut s = status_of(ElevationState::Missing, Some(kind), Some(path.clone()));
    if !host.exists(&path) {
        s.reason = Some("Admin access is not set up on this computer.".into());
        return s;
    }
    match host.helper_version() {
        Ok(v) => {
            s.state = if v == HELPER_VERSION {
                ElevationState::Installed
            } else {
                ElevationState::Outdated
            };
            if s.state == ElevationState::Outdated {
                s.reason = Some(format!("The admin helper is version {v}; this runtime needs {HELPER_VERSION}. Run setup again."));
            }
            s.installed_version = Some(v);
        }
        Err(e) => {
            s.reason = Some(match kind {
                HelperKind::Sudoers => format!(
                    "The helper is there but sudo does not allow it ({e}). Run setup again."
                ),
                _ => format!("The helper is there but does not answer ({e}). Run setup again."),
            });
        }
    }
    s
}

fn allows(os: OsFamily) -> Vec<String> {
    let services = match os {
        OsFamily::Linux => "systemd, supervisord and Docker",
        OsFamily::Macos => "launchd and Docker",
        OsFamily::Windows => "Windows services and Docker",
    };
    let updates = match os {
        OsFamily::Linux => "apt or dnf",
        OsFamily::Macos => "softwareupdate",
        OsFamily::Windows => "Windows Update",
    };
    let mut v = vec![
        format!("Start, stop and restart the services this computer lists ({services})."),
        format!("Install waiting system updates by name ({updates})."),
    ];
    let cleans: Vec<&str> = crate::actions::clean_items()
        .iter()
        .filter(|i| i.needs_elevation && i.os.map_or(true, |o| o == os))
        .map(|i| i.label)
        .collect();
    if !cleans.is_empty() {
        v.push(format!("Clean up: {}.", cleans.join("; ")));
    }
    v.push("Stop the process that listens on a port.".into());
    v.push("Each of these still needs your approval in the console, every time.".into());
    v.push("Never: run scripts, snippets, a shell or any other command.".into());
    v
}

/// The helper binary path must be safe to put inside the elevation prompt
/// command (single-quoted for sh / PowerShell).
fn valid_source(p: &str) -> bool {
    let banned: &[char] = if cfg!(windows) {
        &['\'', '"', '`', '$']
    } else {
        &['\'', '"', '\\', '`', '$']
    };
    let absolute = p.starts_with('/')
        || (p.len() > 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic());
    absolute && !p.chars().any(|c| c.is_control() || banned.contains(&c))
}

fn install_args(input: &SetupInput) -> Vec<String> {
    let mut a = vec!["install".to_string(), "--user".into(), input.user.clone()];
    if let Some(uid) = input.uid {
        a.push("--uid".into());
        a.push(uid.to_string());
    }
    if let Some(sid) = &input.sid {
        a.push("--sid".into());
        a.push(sid.clone());
    }
    a
}

fn approval_for(host: &dyn Host, os: OsFamily) -> ApprovalMethod {
    match os {
        OsFamily::Linux if host.has_program("pkexec") && host.has_display() => {
            ApprovalMethod::Pkexec
        }
        OsFamily::Linux => ApprovalMethod::Terminal,
        OsFamily::Macos => ApprovalMethod::MacAdminDialog,
        OsFamily::Windows => ApprovalMethod::Uac,
    }
}

fn shell_line(program: &str, args: &[String]) -> String {
    let mut s = format!("'{program}'");
    for a in args {
        s.push_str(&format!(" '{a}'"));
    }
    s
}

fn input_for(
    host: &dyn Host,
    helper_source: &Path,
) -> Result<(OsFamily, SetupInput), ElevationError> {
    let os = host.os().ok_or_else(|| {
        ElevationError::Unsupported(format!("admin setup on {}", std::env::consts::OS))
    })?;
    let from = helper_source.to_string_lossy().to_string();
    if !valid_source(&from) {
        return Err(ElevationError::Invalid(format!("helper path {from:?}")));
    }
    if !host.exists(&from) {
        return Err(ElevationError::Invalid(format!(
            "the helper binary is missing at {from}"
        )));
    }
    let (user, uid) = host
        .user()
        .ok_or_else(|| ElevationError::Failed("could not read the current user".into()))?;
    if !valid_user(&user) {
        return Err(ElevationError::Invalid(format!("user name {user:?}")));
    }
    let sid = match os {
        OsFamily::Windows => {
            let sid = host.user_sid().ok_or_else(|| {
                ElevationError::Failed("could not read the current user's SID".into())
            })?;
            if !valid_sid(&sid) {
                return Err(ElevationError::Invalid(format!("Windows user SID {sid:?}")));
            }
            Some(sid)
        }
        _ => None,
    };
    Ok((
        os,
        SetupInput {
            user,
            uid,
            helper_from: from,
            sid,
        },
    ))
}

/// What setup will install, for the user to read before approving.
/// `helper_source` is the `allternit-host-helper` binary shipped next to the runtime.
pub fn plan(host: &dyn Host, helper_source: &Path) -> Result<SetupPlan, ElevationError> {
    let os = host.os().ok_or_else(|| {
        ElevationError::Unsupported(format!("admin setup on {}", std::env::consts::OS))
    })?;
    if host.is_root() {
        return Ok(SetupPlan {
            kind: HelperKind::Root,
            needed: false,
            summary: "Nothing to set up: the runtime already runs as root/admin.".into(),
            allows: Vec::new(),
            steps: Vec::new(),
            files: Vec::new(),
            approval: ApprovalMethod::None,
            terminal_command: None,
            helper_version: HELPER_VERSION.into(),
        });
    }
    let (_, input) = input_for(host, helper_source)?;
    let ops = install_ops(os, &input)?;
    let approval = approval_for(host, os);
    let summary = match os {
        OsFamily::Linux => format!(
            "Installs one helper at {LINUX_HELPER} and a sudoers rule that lets {} run only that helper as root. The rule is checked with visudo before it goes live.",
            input.user
        ),
        OsFamily::Macos => format!(
            "Installs one helper at {MAC_HELPER} that runs as root under launchd and only answers {} through a private socket.",
            input.user
        ),
        OsFamily::Windows => format!(
            "Installs one helper under {} that runs as SYSTEM at startup and only answers {} through a private named pipe that no other account can open.",
            windows_helper_dir(),
            input.user
        ),
    };
    Ok(SetupPlan {
        kind: HelperKind::for_os(os),
        needed: true,
        summary,
        allows: allows(os),
        steps: ops.iter().map(InstallOp::display).collect(),
        files: ops.iter().filter_map(InstallOp::planned_file).collect(),
        approval,
        terminal_command: (approval == ApprovalMethod::Terminal).then(|| {
            format!(
                "sudo {}",
                shell_line(&input.helper_from, &install_args(&input))
            )
        }),
        helper_version: HELPER_VERSION.into(),
    })
}

/// The command that shows the OS admin prompt and runs `helper args` elevated.
fn elevated_command(
    method: ApprovalMethod,
    helper: &str,
    args: &[String],
) -> Option<(String, Vec<String>)> {
    match method {
        ApprovalMethod::Pkexec => {
            let mut a = vec![helper.to_string()];
            a.extend(args.iter().cloned());
            Some(("pkexec".into(), a))
        }
        ApprovalMethod::MacAdminDialog => Some((
            "osascript".into(),
            vec![
                "-e".into(),
                format!(
                    "do shell script \"{}\" with administrator privileges",
                    shell_line(helper, args)
                ),
            ],
        )),
        ApprovalMethod::Uac => {
            let list = args
                .iter()
                .map(|a| format!("'{a}'"))
                .collect::<Vec<_>>()
                .join(",");
            Some((
                "powershell".into(),
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    format!(
                        "$p = Start-Process -FilePath '{helper}' -ArgumentList {list} -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode"
                    ),
                ],
            ))
        }
        ApprovalMethod::None | ApprovalMethod::Terminal => None,
    }
}

fn run_elevated(
    host: &dyn Host,
    method: ApprovalMethod,
    helper: &str,
    args: &[String],
) -> Result<(), ElevationError> {
    let Some((program, argv)) = elevated_command(method, helper, args) else {
        return Err(ElevationError::NeedsTerminal {
            command: format!("sudo {}", shell_line(helper, args)),
        });
    };
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = host
        .run(&program, &refs, PROMPT_TIMEOUT)
        .map_err(|e| ElevationError::Failed(e.reason(&program)))?;
    if out.code == Some(0) {
        return Ok(());
    }
    let text = format!("{}{}", out.stderr, out.stdout);
    // pkexec: 126 = dialog dismissed; osascript: -128 = user cancelled;
    // PowerShell: "The operation was canceled by the user".
    if out.code == Some(126)
        || text.contains("-128")
        || text.to_lowercase().contains("canceled by the user")
    {
        return Err(ElevationError::Cancelled);
    }
    Err(ElevationError::Failed(crate::cmd::first_line(&text)))
}

/// Install the helper: one OS admin prompt, then the status read back from
/// the helper itself. Already-root hosts return `NotNeeded` without a prompt.
pub fn install(host: &dyn Host, helper_source: &Path) -> Result<ElevationStatus, ElevationError> {
    if host.is_root() {
        return Ok(status(host));
    }
    let (os, input) = input_for(host, helper_source)?;
    let method = approval_for(host, os);
    run_elevated(host, method, &input.helper_from, &install_args(&input))?;
    let s = status(host);
    match s.state {
        ElevationState::Installed => Ok(s),
        _ => Err(ElevationError::Failed(s.reason.unwrap_or_else(|| {
            "the helper does not answer after setup".into()
        }))),
    }
}

/// Remove the helper and its rule (one OS admin prompt).
pub fn uninstall(host: &dyn Host, helper_source: &Path) -> Result<ElevationStatus, ElevationError> {
    if host.is_root() {
        return Ok(status(host));
    }
    let (os, input) = input_for(host, helper_source)?;
    let method = approval_for(host, os);
    run_elevated(host, method, &input.helper_from, &["uninstall".to_string()])?;
    Ok(status(host))
}

/// Where the runtime ships the helper: next to its own executable.
pub fn bundled_helper() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join(HELPER_BIN);
    p.is_file().then_some(p)
}

// ---------------------------------------------------------------------------
// Transport: probe -> installed helper
// ---------------------------------------------------------------------------

/// Send one request to the installed helper and return its answer.
pub fn send_to_helper(req: &HelperRequest, timeout: Duration) -> Result<CmdOutput, RunError> {
    let argv = req.to_argv();
    #[cfg(target_os = "linux")]
    {
        let mut args: Vec<&str> = vec!["-n", LINUX_HELPER];
        args.extend(argv.iter().map(String::as_str));
        run("sudo", &args, timeout)
    }
    #[cfg(target_os = "macos")]
    {
        let stream =
            std::os::unix::net::UnixStream::connect(MAC_SOCKET).map_err(|e| io_to_run(&e))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| io_to_run(&e))?;
        exchange(stream, &crate::helper::WireRequest { argv }, timeout)
    }
    #[cfg(windows)]
    {
        let pipe = win::connect_helper_pipe(STATUS_TIMEOUT)?;
        exchange(pipe, &crate::helper::WireRequest { argv }, timeout)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (argv, timeout);
        Err(RunError::NotFound)
    }
}

#[allow(dead_code)]
fn io_to_run(e: &std::io::Error) -> RunError {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound | ConnectionRefused | PermissionDenied => RunError::NotFound,
        WouldBlock | TimedOut => RunError::Timeout,
        _ => RunError::Io(e.to_string()),
    }
}

/// One JSON line out, one JSON line back.
#[allow(dead_code)]
fn exchange<S: std::io::Read + std::io::Write>(
    mut stream: S,
    req: &crate::helper::WireRequest,
    timeout: Duration,
) -> Result<CmdOutput, RunError> {
    use std::io::{BufRead, BufReader, Write};
    let _ = timeout; // the caller set the stream's read timeout
    let mut line = serde_json::to_string(req).map_err(|e| RunError::Io(e.to_string()))?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| io_to_run(&e))?;
    stream.flush().map_err(|e| io_to_run(&e))?;
    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .map_err(|e| io_to_run(&e))?;
    if answer.trim().is_empty() {
        return Err(RunError::Io(
            "the admin helper closed the connection".into(),
        ));
    }
    let reply: crate::helper::WireReply = serde_json::from_str(answer.trim())
        .map_err(|e| RunError::Io(format!("bad helper answer: {e}")))?;
    Ok(reply.into_output())
}

// ---------------------------------------------------------------------------
// Helper binary modes: install, uninstall, serve (run as root)
// ---------------------------------------------------------------------------

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn require_root() -> Result<(), i32> {
    if crate::actions::Executor::elevated(&crate::actions::SystemExecutor) {
        Ok(())
    } else {
        eprintln!("this must run as root/admin (the setup prompt does that)");
        Err(crate::helper::EXIT_NOT_ROOT)
    }
}

/// `allternit-host-helper install --user <name> [--uid <n>] [--sid <S-1-...>]`, run elevated.
pub fn helper_install_main(args: &[String]) -> i32 {
    if let Err(code) = require_root() {
        return code;
    }
    let Some(os) = OsFamily::current() else {
        eprintln!("admin setup is not supported on this OS");
        return 69;
    };
    let user = flag(args, "--user").unwrap_or_default().to_string();
    let uid = match flag(args, "--uid").map(str::parse::<u32>) {
        None => None,
        Some(Ok(u)) => Some(u),
        Some(Err(_)) => {
            eprintln!("--uid must be a number");
            return crate::helper::EXIT_REFUSED;
        }
    };
    let from = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("cannot find this binary: {e}");
            return 1;
        }
    };
    let sid = flag(args, "--sid").map(str::to_string);
    let input = SetupInput {
        user,
        uid,
        helper_from: from,
        sid,
    };
    let ops = match install_ops(os, &input) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return crate::helper::EXIT_REFUSED;
        }
    };
    match apply_ops(&ops, &RealRootFs, &mut |s| println!("{s}")) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("setup failed: {e}");
            1
        }
    }
}

/// `allternit-host-helper uninstall`, run elevated.
pub fn helper_uninstall_main(_args: &[String]) -> i32 {
    if let Err(code) = require_root() {
        return code;
    }
    let Some(os) = OsFamily::current() else {
        return 69;
    };
    match apply_ops(&uninstall_ops(os), &RealRootFs, &mut |s| println!("{s}")) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("uninstall failed: {e}");
            1
        }
    }
}

/// `allternit-host-helper serve`: the macOS LaunchDaemon / Windows task loop.
pub fn helper_serve_main(args: &[String]) -> i32 {
    if let Err(code) = require_root() {
        return code;
    }
    #[cfg(target_os = "macos")]
    {
        let Some(uid) = flag(args, "--uid").and_then(|u| u.parse::<u32>().ok()) else {
            eprintln!("serve needs --uid <n>");
            return crate::helper::EXIT_REFUSED;
        };
        if let Err(e) = serve_unix(uid) {
            eprintln!("serve: {e}");
            return 1;
        }
        0
    }
    #[cfg(windows)]
    {
        let Some(sid) = flag(args, "--sid").filter(|s| valid_sid(s)) else {
            eprintln!("serve needs --sid <S-1-...>");
            return crate::helper::EXIT_REFUSED;
        };
        if let Err(e) = win::serve_pipe(sid) {
            eprintln!("serve: {e}");
            return 1;
        }
        0
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = args;
        eprintln!("serve is only used on macOS and Windows; Linux uses sudo");
        crate::helper::EXIT_REFUSED
    }
}

#[cfg(target_os = "macos")]
fn serve_unix(uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    let _ = std::fs::remove_file(MAC_SOCKET);
    let listener = UnixListener::bind(MAC_SOCKET)?;
    // Only the user (and root) may connect.
    std::os::unix::fs::chown(MAC_SOCKET, Some(uid), Some(0))?;
    std::fs::set_permissions(MAC_SOCKET, std::fs::Permissions::from_mode(0o600))?;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Ok(reader) = stream.try_clone() {
                let _ = crate::helper::serve_one(
                    &crate::actions::SystemExecutor,
                    std::io::BufReader::new(reader),
                    stream,
                );
            }
        });
    }
    Ok(())
}

#[cfg(windows)]
mod win;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    const TEST_SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";

    struct FakeHost {
        os: Option<OsFamily>,
        root: bool,
        files: Vec<String>,
        display: bool,
        pkexec: bool,
        version: Result<String, String>,
        replies: RefCell<VecDeque<CmdOutput>>,
        calls: RefCell<Vec<(String, Vec<String>)>>,
    }

    impl FakeHost {
        fn new(os: OsFamily) -> FakeHost {
            FakeHost {
                os: Some(os),
                root: false,
                files: vec!["/opt/allternit/bin/allternit-host-helper".into()],
                display: false,
                pkexec: false,
                version: Err("sudo: a password is required".into()),
                replies: RefCell::new(VecDeque::new()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Host for FakeHost {
        fn os(&self) -> Option<OsFamily> {
            self.os
        }
        fn is_root(&self) -> bool {
            self.root
        }
        fn user(&self) -> Option<(String, Option<u32>)> {
            Some(("joe".into(), Some(501)))
        }
        fn user_sid(&self) -> Option<String> {
            (self.os == Some(OsFamily::Windows)).then(|| TEST_SID.to_string())
        }
        fn exists(&self, path: &str) -> bool {
            self.files.iter().any(|f| f == path)
        }
        fn has_program(&self, name: &str) -> bool {
            name == "pkexec" && self.pkexec
        }
        fn has_display(&self) -> bool {
            self.display
        }
        fn run(&self, program: &str, args: &[&str], _t: Duration) -> Result<CmdOutput, RunError> {
            self.calls
                .borrow_mut()
                .push((program.into(), args.iter().map(|a| a.to_string()).collect()));
            Ok(self.replies.borrow_mut().pop_front().unwrap_or(CmdOutput {
                code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            }))
        }
        fn helper_version(&self) -> Result<String, String> {
            self.version.clone()
        }
    }

    const SRC: &str = "/opt/allternit/bin/allternit-host-helper";

    #[test]
    fn root_and_orgo_need_no_setup() {
        let mut h = FakeHost::new(OsFamily::Linux);
        h.root = true;
        assert_eq!(status(&h).state, ElevationState::NotNeeded);
        let p = plan(&h, Path::new(SRC)).unwrap();
        assert!(!p.needed);
        assert_eq!(p.approval, ApprovalMethod::None);
        assert!(p.files.is_empty());
        let s = install(&h, Path::new(SRC)).unwrap();
        assert_eq!(s.state, ElevationState::NotNeeded);
        assert!(h.calls.borrow().is_empty(), "no prompt on a root host");
    }

    #[test]
    fn status_reads_the_helper_answer() {
        let mut h = FakeHost::new(OsFamily::Linux);
        assert_eq!(status(&h).state, ElevationState::Missing, "no helper file");
        h.files.push(LINUX_HELPER.into());
        assert_eq!(status(&h).state, ElevationState::Missing, "sudo refuses");
        h.version = Ok("0".into());
        let s = status(&h);
        assert_eq!(s.state, ElevationState::Outdated);
        assert_eq!(s.installed_version.as_deref(), Some("0"));
        h.version = Ok(HELPER_VERSION.into());
        assert_eq!(status(&h).state, ElevationState::Installed);
        h.os = None;
        assert_eq!(status(&h).state, ElevationState::Unsupported);
    }

    #[test]
    fn linux_plan_shows_the_sudoers_rule_and_a_terminal_command_when_headless() {
        let h = FakeHost::new(OsFamily::Linux);
        let p = plan(&h, Path::new(SRC)).unwrap();
        assert!(p.needed);
        assert_eq!(p.kind, HelperKind::Sudoers);
        assert_eq!(p.approval, ApprovalMethod::Terminal);
        assert_eq!(
            p.terminal_command.as_deref(),
            Some("sudo '/opt/allternit/bin/allternit-host-helper' 'install' '--user' 'joe' '--uid' '501'")
        );
        let sudoers = p.files.iter().find(|f| f.path == SUDOERS_PATH).unwrap();
        assert_eq!(sudoers.mode, "440");
        let rule = &sudoers.contents;
        assert!(rule.contains("joe ALL=(root) NOPASSWD: ALLTERNIT_HOST_HELPER"));
        assert!(rule.contains(&format!("{LINUX_HELPER} clean apt-cache")));
        assert!(rule.contains(&format!("{LINUX_HELPER} service stop *")));
        assert!(!rule.contains("docker-prune"), "only admin clean-up items");
        assert!(
            !rule.contains(&format!("{LINUX_HELPER} install"))
                && !rule.contains(&format!("{LINUX_HELPER} uninstall")),
            "setup itself is never passwordless"
        );
        assert!(!rule.contains("ALL=(ALL)") && !rule.contains("NOPASSWD: ALL\n"));
        assert!(p.steps.iter().any(|s| s.contains("visudo -cf")));
        assert!(p.allows.iter().any(|a| a.starts_with("Never")));
        // Every command line in the rule names the fixed helper path.
        for l in rule.lines().filter(|l| l.trim_start().starts_with('/')) {
            assert!(l.trim_start().starts_with(LINUX_HELPER), "{l}");
        }
    }

    #[test]
    fn linux_desktop_uses_pkexec_and_reads_back_status() {
        let mut h = FakeHost::new(OsFamily::Linux);
        h.display = true;
        h.pkexec = true;
        h.files.push(LINUX_HELPER.into());
        h.version = Ok(HELPER_VERSION.into());
        let s = install(&h, Path::new(SRC)).unwrap();
        assert_eq!(s.state, ElevationState::Installed);
        let calls = h.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "pkexec");
        assert_eq!(
            calls[0].1,
            vec![SRC, "install", "--user", "joe", "--uid", "501"]
        );
    }

    #[test]
    fn headless_linux_install_asks_for_the_terminal() {
        let h = FakeHost::new(OsFamily::Linux);
        let err = install(&h, Path::new(SRC)).unwrap_err();
        assert!(
            matches!(err, ElevationError::NeedsTerminal { .. }),
            "{err:?}"
        );
        assert!(h.calls.borrow().is_empty());
    }

    #[test]
    fn mac_install_uses_the_admin_dialog_and_cancel_is_reported() {
        let h = FakeHost::new(OsFamily::Macos);
        h.replies.borrow_mut().push_back(CmdOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "execution error: User canceled. (-128)".into(),
        });
        let err = install(&h, Path::new(SRC)).unwrap_err();
        assert_eq!(err, ElevationError::Cancelled);
        let calls = h.calls.borrow();
        assert_eq!(calls[0].0, "osascript");
        assert_eq!(
            calls[0].1[1],
            "do shell script \"'/opt/allternit/bin/allternit-host-helper' 'install' '--user' 'joe' '--uid' '501'\" with administrator privileges"
        );
    }

    #[test]
    fn mac_plan_has_the_launch_daemon() {
        let h = FakeHost::new(OsFamily::Macos);
        let p = plan(&h, Path::new(SRC)).unwrap();
        assert_eq!(p.kind, HelperKind::LaunchDaemon);
        assert_eq!(p.approval, ApprovalMethod::MacAdminDialog);
        let plist = p.files.iter().find(|f| f.path == MAC_PLIST).unwrap();
        assert!(plist.contents.contains("<string>serve</string>"));
        assert!(plist.contents.contains("<string>501</string>"));
        assert!(p
            .steps
            .iter()
            .any(|s| s == &format!("run launchctl bootstrap system {MAC_PLIST}")));
    }

    #[test]
    fn windows_plan_uses_a_pipe_for_the_user_sid_and_no_token() {
        let h = FakeHost::new(OsFamily::Windows);
        // The Unix-style source path is fine for the plan on any build host.
        let p = plan(&h, Path::new(SRC)).unwrap();
        assert_eq!(p.approval, ApprovalMethod::Uac);
        assert!(p.files.is_empty(), "no token or other file: {:?}", p.files);
        assert!(p.steps.iter().any(|s| s.contains("/RU SYSTEM")
            && s.contains(WINDOWS_TASK)
            && s.contains(&format!("serve --sid {TEST_SID}"))));
        assert!(!p
            .steps
            .iter()
            .any(|s| s.contains("icacls") || s.contains("token")));
        assert!(p.summary.contains("named pipe"));
    }

    #[test]
    fn windows_install_needs_a_valid_sid() {
        let mut input = linux_input();
        input.sid = None;
        assert!(matches!(
            install_ops(OsFamily::Windows, &input),
            Err(ElevationError::Invalid(_))
        ));
        input.sid = Some("S-1-5-21-1 & calc".into());
        assert!(matches!(
            install_ops(OsFamily::Windows, &input),
            Err(ElevationError::Invalid(_))
        ));
        input.sid = Some(TEST_SID.into());
        assert!(install_ops(OsFamily::Windows, &input).is_ok());
        let args = install_args(&input);
        assert_eq!(&args[args.len() - 2..], ["--sid", TEST_SID]);
    }

    #[test]
    fn sid_check_and_pipe_sddl() {
        for good in [TEST_SID, "S-1-5-18", "S-1-12-1-1234-5678"] {
            assert!(valid_sid(good), "{good}");
        }
        for bad in [
            "",
            "S-1-",
            "S-1-5-",
            "S-1-5--18",
            "s-1-5-18",
            "S-1-5-18)(A;;GA;;;WD",
            "S-1-5-21-1 2",
            "S-1-5-123456789012345678901",
        ] {
            assert!(!valid_sid(bad), "{bad:?}");
            assert!(windows_pipe_sddl(bad).is_none());
        }
        let sddl = windows_pipe_sddl(TEST_SID).unwrap();
        assert!(
            sddl.starts_with("O:SYG:SYD:P("),
            "owner SYSTEM, protected DACL"
        );
        assert!(sddl.contains("(A;;GA;;;SY)"));
        assert!(sddl.contains(&format!("(A;;0x12019b;;;{TEST_SID})")));
        assert!(!sddl.contains(";WD)") && !sddl.contains(";BU)") && !sddl.contains(";AU)"));
    }

    #[test]
    fn bad_inputs_are_refused_before_any_prompt() {
        let h = FakeHost::new(OsFamily::Linux);
        for bad in ["relative/helper", "/tmp/a'b", "/tmp/a\"b", "/tmp/$(id)"] {
            assert!(
                matches!(plan(&h, Path::new(bad)), Err(ElevationError::Invalid(_))),
                "{bad}"
            );
        }
        assert!(matches!(
            plan(&h, Path::new("/opt/missing/helper")),
            Err(ElevationError::Invalid(_))
        ));
        for u in ["", "root ALL", "a,b", "-x", "joe\nALL"] {
            assert!(!valid_user(u), "{u:?}");
        }
        assert!(valid_user("joe") && valid_user("deploy_1") && valid_user("j.smith"));
    }

    #[derive(Default)]
    struct FakeFs {
        files: RefCell<HashMap<String, String>>,
        log: RefCell<Vec<String>>,
        check_code: i32,
    }

    impl RootFs for FakeFs {
        fn make_dir(&self, path: &str, _m: u32) -> std::io::Result<()> {
            self.log.borrow_mut().push(format!("mkdir {path}"));
            Ok(())
        }
        fn copy(&self, _f: &str, to: &str, _m: u32) -> std::io::Result<()> {
            self.files.borrow_mut().insert(to.into(), "<bin>".into());
            Ok(())
        }
        fn write(&self, path: &str, _m: u32, c: &str) -> std::io::Result<()> {
            self.files.borrow_mut().insert(path.into(), c.into());
            Ok(())
        }
        fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
            let c = self.files.borrow_mut().remove(from).unwrap();
            self.files.borrow_mut().insert(to.into(), c);
            Ok(())
        }
        fn remove(&self, path: &str) -> std::io::Result<()> {
            self.files
                .borrow_mut()
                .remove(path)
                .map(|_| ())
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput, RunError> {
            self.log
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            Ok(CmdOutput {
                code: Some(if program == "visudo" {
                    self.check_code
                } else {
                    0
                }),
                stdout: String::new(),
                stderr: if self.check_code != 0 {
                    ">>> syntax error near line 3 <<<".into()
                } else {
                    String::new()
                },
            })
        }
    }

    fn linux_input() -> SetupInput {
        SetupInput {
            user: "joe".into(),
            uid: Some(1000),
            helper_from: SRC.into(),
            sid: None,
        }
    }

    #[test]
    fn sudoers_goes_live_only_after_visudo_passes() {
        let ops = install_ops(OsFamily::Linux, &linux_input()).unwrap();
        let fs = FakeFs::default();
        apply_ops(&ops, &fs, &mut |_| {}).unwrap();
        let files = fs.files.borrow();
        assert!(files.contains_key(SUDOERS_PATH));
        assert!(!files.contains_key(SUDOERS_STAGING));
        assert!(files.contains_key(LINUX_HELPER));
        assert!(fs
            .log
            .borrow()
            .contains(&format!("visudo -cf {SUDOERS_STAGING}")));
    }

    #[test]
    fn visudo_failure_leaves_no_rule_behind() {
        let ops = install_ops(OsFamily::Linux, &linux_input()).unwrap();
        let fs = FakeFs {
            check_code: 1,
            ..FakeFs::default()
        };
        let err = apply_ops(&ops, &fs, &mut |_| {}).unwrap_err();
        assert!(err.contains("visudo rejected"), "{err}");
        let files = fs.files.borrow();
        assert!(!files.contains_key(SUDOERS_PATH));
        assert!(!files.contains_key(SUDOERS_STAGING));
    }

    #[test]
    fn uninstall_removes_what_install_wrote() {
        for os in [OsFamily::Linux, OsFamily::Macos, OsFamily::Windows] {
            let mut input = linux_input();
            input.sid = Some(TEST_SID.into());
            let fs = FakeFs::default();
            apply_ops(&install_ops(os, &input).unwrap(), &fs, &mut |_| {}).unwrap();
            assert!(!fs.files.borrow().is_empty());
            apply_ops(&uninstall_ops(os), &fs, &mut |_| {}).unwrap();
            assert!(
                fs.files.borrow().is_empty(),
                "{os:?}: {:?}",
                fs.files.borrow().keys()
            );
        }
    }

    #[test]
    fn status_json_is_camel_case() {
        let mut s = status_of(ElevationState::NotNeeded, Some(HelperKind::Root), None);
        s.installed_version = Some("1".into());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "notNeeded");
        assert_eq!(v["kind"], "root");
        assert_eq!(v["expectedVersion"], HELPER_VERSION);
        assert_eq!(v["installedVersion"], "1");
    }
}
