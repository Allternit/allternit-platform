//! `POST /v1/host/actions`: the one write route of the probe.
//!
//! Body `{ type, target, approval_id }` (`approvalId` is accepted too), answer
//! `{ changeId, undo? }` (contract: `surfaces/docs/dev/computers-console-contract.mdx`,
//! TypeScript `HostAction` / `HostActionResult` in allternit-ai
//! `src/lib/computers/console-types.ts`).
//!
//! The action list is fixed: `service.start|stop|restart`, `port.free`,
//! `updates.install`, `space.clean`, `snippet.run`. Nothing else can run.
//!
//! Order of work for every action ([`ActionEngine::run`]):
//!
//! 1. Unknown `type` is [`ActionError::UnknownType`] (400).
//! 2. Missing, malformed or refused `approval_id` is [`ActionError::NotApproved`]
//!    (403); nothing has run.
//! 3. The target is looked up through the read modules (services, ports,
//!    updates) and turned into fixed commands. No shell, and only names the
//!    host itself reported reach a command line. `snippet.run` is the one
//!    exception: it runs the saved snippet's script, which the user wrote and
//!    approved.
//! 4. An action that needs root/admin on a probe without it goes through the
//!    admin helper when the one-time setup is installed ([`crate::helper`],
//!    [`crate::elevation`]); otherwise it is [`ActionError::NeedsElevation`]
//!    before anything runs and the approval stays unused. The helper never
//!    runs snippets.
//! 5. The approval id is claimed (single use) and the change record is
//!    written with status `pending` **before** any command runs.
//! 6. The commands run; the record gets the result, masked output, the state
//!    read back after, and the undo (or a plain note saying why there is none).
//!
//! An undo is itself an action and needs its own approval.
//!
//! Every command goes through an [`Executor`], so tests use a fake one and
//! never run a real command. The engine is blocking; async callers use
//! `tokio::task::spawn_blocking`.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::changes::{safe_file_name, ChangeRecord, ChangeStatus, ChangeStore};
use crate::cmd::{run, CmdOutput, RunError};
use crate::helper::HelperRequest;
use crate::logs::{valid_name, valid_provider};
use crate::mask::{mask_line, MaskOptions};
use crate::ports::HostPort;
use crate::services::{HostService, ServiceProvider, ServiceState};
use crate::updates::{HostUpdate, UpdateSource};
use crate::OsFamily;

pub(crate) const SERVICE_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) const KILL_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const UPDATE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
pub(crate) const CLEAN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SNIPPET_DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const SNIPPET_MAX_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SNIPPET_MAX_LEN: usize = 64 * 1024;
const OUTPUT_TAIL: usize = 4000;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// The fixed action list. Serialized as `service.start` and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionType {
    #[serde(rename = "service.start")]
    ServiceStart,
    #[serde(rename = "service.stop")]
    ServiceStop,
    #[serde(rename = "service.restart")]
    ServiceRestart,
    #[serde(rename = "port.free")]
    PortFree,
    #[serde(rename = "updates.install")]
    UpdatesInstall,
    #[serde(rename = "space.clean")]
    SpaceClean,
    #[serde(rename = "snippet.run")]
    SnippetRun,
}

impl ActionType {
    pub const ALL: [ActionType; 7] = [
        ActionType::ServiceStart,
        ActionType::ServiceStop,
        ActionType::ServiceRestart,
        ActionType::PortFree,
        ActionType::UpdatesInstall,
        ActionType::SpaceClean,
        ActionType::SnippetRun,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ActionType::ServiceStart => "service.start",
            ActionType::ServiceStop => "service.stop",
            ActionType::ServiceRestart => "service.restart",
            ActionType::PortFree => "port.free",
            ActionType::UpdatesInstall => "updates.install",
            ActionType::SpaceClean => "space.clean",
            ActionType::SnippetRun => "snippet.run",
        }
    }

    /// `None` for anything off the fixed list.
    pub fn parse(s: &str) -> Option<ActionType> {
        ActionType::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// Request body. `type` stays a string so an unknown value is a clean 400
/// ([`ActionError::UnknownType`]) and not a JSON error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRequest {
    #[serde(rename = "type")]
    pub action_type: String,
    pub target: String,
    #[serde(
        default,
        rename = "approval_id",
        alias = "approvalId",
        skip_serializing_if = "Option::is_none"
    )]
    pub approval_id: Option<String>,
    /// Restore point the cloud recorded before approving (D6), stored with
    /// the change record. Optional, additive.
    #[serde(
        default,
        rename = "restorePoint",
        alias = "restore_point",
        skip_serializing_if = "Option::is_none"
    )]
    pub restore_point: Option<String>,
    /// `snippet.run` only: the snippet body the cloud sends with the approved
    /// action (its `id` must equal `target`), so the node needs no saved copy.
    /// Without it the engine reads `<store>/snippets/<target>.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Snippet>,
}

/// The way back from a change. It is an action and needs its own approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoAction {
    #[serde(rename = "type")]
    pub action_type: ActionType,
    pub target: String,
    pub label: String,
}

/// Response body: contract fields `changeId` and `undo`; `note` is additive
/// (why there is no undo, or that nothing needed doing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    #[serde(alias = "change_id")]
    pub change_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<UndoAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Errors, each with an HTTP status for the route that mounts the engine.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ActionError {
    #[error("unknown action type {0:?}")]
    UnknownType(String),
    #[error("not approved: {0}")]
    NotApproved(String),
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    #[error("not found: {0}")]
    TargetNotFound(String),
    /// The probe will not do this (for example stop the runtime serving it).
    #[error("refused: {0}")]
    Refused(String),
    /// Needs root/admin and the probe has none. Nothing changed. `change_id`
    /// is set when the OS refused at run time (the record says so).
    #[error("needs elevation: {reason}")]
    NeedsElevation {
        reason: String,
        change_id: Option<String>,
    },
    #[error("not supported on this host: {0}")]
    Unsupported(String),
    /// The change record could not be written; nothing ran.
    #[error("change store: {0}")]
    Store(String),
    /// The action ran and failed; the record has the output.
    #[error("action failed: {message}")]
    Failed { change_id: String, message: String },
}

impl ActionError {
    pub fn http_status(&self) -> u16 {
        match self {
            ActionError::UnknownType(_) | ActionError::InvalidTarget(_) => 400,
            ActionError::NotApproved(_) | ActionError::NeedsElevation { .. } => 403,
            ActionError::TargetNotFound(_) => 404,
            ActionError::Refused(_) => 409,
            ActionError::Unsupported(_) => 501,
            ActionError::Store(_) | ActionError::Failed { .. } => 500,
        }
    }

    /// Stable machine code for the `error` field.
    pub fn code(&self) -> &'static str {
        match self {
            ActionError::UnknownType(_) => "unknown_type",
            ActionError::NotApproved(_) => "not_approved",
            ActionError::InvalidTarget(_) => "invalid_target",
            ActionError::TargetNotFound(_) => "not_found",
            ActionError::Refused(_) => "refused",
            ActionError::NeedsElevation { .. } => "needs_elevation",
            ActionError::Unsupported(_) => "unsupported",
            ActionError::Store(_) => "store_error",
            ActionError::Failed { .. } => "action_failed",
        }
    }

    /// `{ error, message, changeId? }` for the response body.
    pub fn body(&self) -> Value {
        let mut v = json!({ "error": self.code(), "message": self.to_string() });
        match self {
            ActionError::Failed { change_id, .. }
            | ActionError::NeedsElevation {
                change_id: Some(change_id),
                ..
            } => {
                v["changeId"] = json!(change_id);
            }
            _ => {}
        }
        v
    }
}

// ---------------------------------------------------------------------------
// Executor, approvals, clean-up items, snippets
// ---------------------------------------------------------------------------

/// Everything the engine reads from or does to the host. [`SystemExecutor`]
/// is the real one; tests use a fake.
pub trait Executor: Send + Sync {
    /// OS family the actions are planned for; `None` = unsupported host.
    fn os(&self) -> Option<OsFamily>;
    /// Running as root (Unix) or elevated admin (Windows).
    fn elevated(&self) -> bool;
    /// Pid of the runtime serving the probe (never stopped by an action).
    fn own_pid(&self) -> u32 {
        std::process::id()
    }
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, RunError>;
    fn services(&self) -> crate::Result<Vec<HostService>>;
    fn ports(&self) -> crate::Result<Vec<HostPort>>;
    fn updates(&self) -> crate::Result<Vec<HostUpdate>>;
    /// The admin helper is installed and answers the current version
    /// ([`crate::elevation::status`]). Default: no helper.
    fn helper_ready(&self) -> bool {
        false
    }
    /// Send one allow-listed request to the installed admin helper.
    fn run_helper(&self, req: &HelperRequest, timeout: Duration) -> Result<CmdOutput, RunError> {
        let _ = (req, timeout);
        Err(RunError::NotFound)
    }
}

/// Runs real commands through [`crate::cmd::run`] and reads the host through
/// the services, ports and updates modules.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemExecutor;

impl Executor for SystemExecutor {
    fn os(&self) -> Option<OsFamily> {
        OsFamily::current()
    }

    fn elevated(&self) -> bool {
        static ELEVATED: OnceLock<bool> = OnceLock::new();
        *ELEVATED.get_or_init(detect_elevated)
    }

    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, RunError> {
        run(program, args, timeout)
    }

    fn services(&self) -> crate::Result<Vec<HostService>> {
        crate::services::services()
    }

    fn ports(&self) -> crate::Result<Vec<HostPort>> {
        crate::ports::ports()
    }

    fn updates(&self) -> crate::Result<Vec<HostUpdate>> {
        crate::updates::updates()
    }

    fn helper_ready(&self) -> bool {
        !self.elevated()
            && crate::elevation::status(&crate::elevation::SystemHost).state
                == crate::elevation::ElevationState::Installed
    }

    fn run_helper(&self, req: &HelperRequest, timeout: Duration) -> Result<CmdOutput, RunError> {
        crate::elevation::send_to_helper(req, timeout)
    }
}

#[cfg(unix)]
fn detect_elevated() -> bool {
    run("id", &["-u"], crate::cmd::COMMAND_TIMEOUT)
        .map(|o| o.code == Some(0) && o.stdout.trim() == "0")
        .unwrap_or(false)
}

/// `net session` only succeeds in an elevated process.
#[cfg(windows)]
fn detect_elevated() -> bool {
    run("net", &["session"], crate::cmd::COMMAND_TIMEOUT)
        .map(|o| o.code == Some(0))
        .unwrap_or(false)
}

#[cfg(not(any(unix, windows)))]
fn detect_elevated() -> bool {
    false
}

/// Checks that the cloud issued `approval_id` for this action. The engine's
/// default ([`crate::approval::DenyApprovals`]) refuses everything; the
/// mounting runtime plugs in [`crate::approval::RelayKeyApproval`], which
/// checks the cloud's signed `ap1.*` ids with this runtime's relay key.
pub trait ApprovalVerifier: Send + Sync {
    fn verify(&self, approval_id: &str, action: ActionType, target: &str) -> Result<(), String>;
}

/// Accepts any well-formed approval id. Tests only: no mounted route may use
/// it (the cloud's signature is the approval).
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WellFormedApproval;

#[cfg(test)]

impl ApprovalVerifier for WellFormedApproval {
    fn verify(&self, _approval_id: &str, _action: ActionType, _target: &str) -> Result<(), String> {
        Ok(())
    }
}

/// One fixed clean-up item. Findings (`suggestedAction`) point at these ids.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanItem {
    pub id: &'static str,
    pub label: &'static str,
    /// `None` = any OS.
    pub os: Option<OsFamily>,
    pub needs_elevation: bool,
    #[serde(skip)]
    pub program: &'static str,
    #[serde(skip)]
    pub args: &'static [&'static str],
}

const CLEAN_ITEMS: &[CleanItem] = &[
    CleanItem {
        id: "apt-cache",
        label: "Downloaded package files (apt)",
        os: Some(OsFamily::Linux),
        needs_elevation: true,
        program: "apt-get",
        args: &["clean"],
    },
    CleanItem {
        id: "dnf-cache",
        label: "Downloaded package files (dnf)",
        os: Some(OsFamily::Linux),
        needs_elevation: true,
        program: "dnf",
        args: &["clean", "packages"],
    },
    CleanItem {
        id: "journal-vacuum",
        label: "System logs older than 7 days",
        os: Some(OsFamily::Linux),
        needs_elevation: true,
        program: "journalctl",
        args: &["--vacuum-time=7d"],
    },
    CleanItem {
        id: "docker-prune",
        label: "Stopped containers, unused networks, dangling images and build cache",
        os: None,
        needs_elevation: false,
        program: "docker",
        args: &["system", "prune", "-f"],
    },
    CleanItem {
        id: "brew-cleanup",
        label: "Old Homebrew downloads and versions",
        os: Some(OsFamily::Macos),
        needs_elevation: false,
        program: "brew",
        args: &["cleanup", "--prune=all"],
    },
];

/// Every clean-up item `space.clean` knows.
pub fn clean_items() -> &'static [CleanItem] {
    CLEAN_ITEMS
}

/// A saved snippet, read from `<store root>/snippets/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// Run with `sh -c` (Linux, macOS) or PowerShell `-Command` (Windows).
    pub script: String,
    /// Needs root/admin to make sense.
    #[serde(default)]
    pub elevated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_sec: Option<u64>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Runs approved actions against one host.
pub struct ActionEngine<E: Executor> {
    exec: E,
    store: ChangeStore,
    approvals: Box<dyn ApprovalVerifier>,
}

impl ActionEngine<SystemExecutor> {
    /// Real host, store at the runtime home, approvals checked by `verifier`
    /// (in production [`crate::approval::RelayKeyApproval`]).
    pub fn system(verifier: Box<dyn ApprovalVerifier>) -> Self {
        ActionEngine::new(SystemExecutor, ChangeStore::open_default()).with_verifier(verifier)
    }
}

/// Run one action on this host with [`ActionEngine::system`]. Blocking.
pub fn run_action(
    req: &ActionRequest,
    verifier: Box<dyn ApprovalVerifier>,
) -> Result<ActionResult, ActionError> {
    ActionEngine::system(verifier).run(req)
}

/// How a step's success is judged.
#[derive(Debug, Clone, Copy, PartialEq)]
enum OkCheck {
    ExitZero,
    /// supervisorctl can print `ERROR (...)` and still exit 0 on old versions.
    ExitZeroNoErrorWord,
}

#[derive(Debug, Clone)]
pub(crate) struct Step {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) timeout: Duration,
    ok: OkCheck,
}

impl Step {
    fn new(program: &str, args: &[&str], timeout: Duration) -> Step {
        Step {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            timeout,
            ok: OkCheck::ExitZero,
        }
    }

    fn display(&self) -> String {
        let mut s = self.program.clone();
        for a in &self.args {
            s.push(' ');
            s.push_str(a);
        }
        let masked = mask_line(&s, &MaskOptions::default());
        truncate_chars(&masked, 500)
    }
}

/// What to read back after the commands ran.
#[derive(Debug, Clone)]
enum After {
    Nothing,
    Service(String),
    Port(u16),
}

#[derive(Debug, Clone)]
struct Plan {
    steps: Vec<Step>,
    needs_elevation: bool,
    elevation_reason: String,
    before: Option<Value>,
    undo: Option<UndoAction>,
    note: Option<String>,
    after: After,
    /// The same work as allow-listed admin helper requests. Empty = the
    /// helper cannot do it (snippets).
    helper: Vec<HelperRequest>,
}

impl Plan {
    fn new(steps: Vec<Step>) -> Plan {
        Plan {
            steps,
            needs_elevation: false,
            elevation_reason: String::new(),
            before: None,
            undo: None,
            note: None,
            after: After::Nothing,
            helper: Vec::new(),
        }
    }
}

impl<E: Executor> ActionEngine<E> {
    /// Approvals are refused until [`ActionEngine::with_verifier`] plugs in
    /// a real check.
    pub fn new(exec: E, store: ChangeStore) -> Self {
        ActionEngine {
            exec,
            store,
            approvals: Box::new(crate::approval::DenyApprovals),
        }
    }

    /// Replace the approval check (for example a signature check).
    pub fn with_verifier(mut self, v: Box<dyn ApprovalVerifier>) -> Self {
        self.approvals = v;
        self
    }

    pub fn store(&self) -> &ChangeStore {
        &self.store
    }

    /// Run one action. See the module docs for the order of work.
    pub fn run(&self, req: &ActionRequest) -> Result<ActionResult, ActionError> {
        let action = ActionType::parse(req.action_type.trim())
            .ok_or_else(|| ActionError::UnknownType(req.action_type.clone()))?;

        let approval_id = req
            .approval_id
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| ActionError::NotApproved("approval_id is required".into()))?;
        if !safe_file_name(approval_id) {
            return Err(ActionError::NotApproved("approval_id is malformed".into()));
        }
        let target = req.target.trim();
        self.approvals
            .verify(approval_id, action, target)
            .map_err(ActionError::NotApproved)?;

        let os = self.exec.os().ok_or_else(|| {
            ActionError::Unsupported(format!("actions on {}", std::env::consts::OS))
        })?;

        let plan = self.plan(action, target, os, req.snippet.as_ref())?;
        let mut via_helper = false;
        if plan.needs_elevation && !self.exec.elevated() {
            if plan.helper.is_empty() {
                return Err(ActionError::NeedsElevation {
                    reason: format!(
                        "{} (the admin helper does not run this; run the runtime as root/admin)",
                        plan.elevation_reason
                    ),
                    change_id: None,
                });
            }
            if !self.exec.helper_ready() {
                return Err(ActionError::NeedsElevation {
                    reason: format!(
                        "{}; set up admin access for this computer first",
                        plan.elevation_reason
                    ),
                    change_id: None,
                });
            }
            via_helper = true;
        }

        let change_id = new_change_id();
        match self.store.claim_approval(approval_id, &change_id) {
            Ok(true) => {}
            Ok(false) => {
                return Err(ActionError::NotApproved(
                    "approval_id was already used".into(),
                ))
            }
            Err(e) => return Err(ActionError::Store(e)),
        }

        let mut rec = ChangeRecord {
            id: change_id.clone(),
            approval_id: approval_id.to_string(),
            action_type: action,
            target: target.to_string(),
            status: ChangeStatus::Pending,
            created_at: now_iso(),
            finished_at: None,
            os: Some(os),
            restore_point: req.restore_point.clone().filter(|r| !r.trim().is_empty()),
            before: plan.before.clone(),
            after: None,
            commands: if via_helper {
                plan.helper.iter().map(HelperRequest::display).collect()
            } else {
                plan.steps.iter().map(Step::display).collect()
            },
            output: None,
            error: None,
            undo: None,
            note: None,
        };
        if let Err(e) = self.store.create(&rec) {
            self.store.release_approval(approval_id);
            return Err(ActionError::Store(e));
        }

        let mut outcome = if via_helper {
            self.execute_helper(&plan.helper)
        } else {
            self.execute(&plan.steps)
        };
        // Tried without root (supervisord, Docker, launchd, kill) and the OS
        // refused before changing anything: retry once through the helper.
        if !via_helper
            && matches!(outcome.failure, Some(Failure::Permission(_)))
            && !plan.helper.is_empty()
            && !self.exec.elevated()
            && self.exec.helper_ready()
        {
            rec.commands
                .extend(plan.helper.iter().map(HelperRequest::display));
            let first = outcome.output.take();
            outcome = self.execute_helper(&plan.helper);
            outcome.output = match (first, outcome.output) {
                (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
                (a, b) => a.or(b),
            };
        }
        rec.finished_at = Some(now_iso());
        rec.output = outcome.output;
        let result = match outcome.failure {
            None => {
                rec.status = ChangeStatus::Succeeded;
                rec.after = self.read_after(&plan.after);
                rec.undo = plan.undo.clone();
                rec.note = plan.note.clone();
                Ok(ActionResult {
                    change_id: change_id.clone(),
                    undo: plan.undo,
                    note: plan.note,
                })
            }
            Some(Failure::Permission(msg)) => {
                // The OS refused before changing anything: give the approval
                // back so it can run after the elevation setup.
                rec.status = ChangeStatus::NeedsElevation;
                rec.error = Some(msg.clone());
                rec.note = Some("Nothing was changed. Set up admin access for this computer, then run it again.".into());
                self.store.release_approval(approval_id);
                Err(ActionError::NeedsElevation {
                    reason: msg,
                    change_id: Some(change_id.clone()),
                })
            }
            Some(Failure::Other(msg)) => {
                rec.status = ChangeStatus::Failed;
                rec.error = Some(msg.clone());
                rec.after = self.read_after(&plan.after);
                Err(ActionError::Failed {
                    change_id: change_id.clone(),
                    message: msg,
                })
            }
        };
        if let Err(e) = self.store.update(&rec) {
            // The action already ran; report it but keep its result.
            if let Ok(ok) = &result {
                return Ok(ActionResult {
                    note: Some(format!(
                        "{} (the change record could not be updated: {e})",
                        ok.note.clone().unwrap_or_else(|| "Done".into())
                    )),
                    ..ok.clone()
                });
            }
        }
        result
    }

    // -- planning ----------------------------------------------------------

    fn plan(
        &self,
        action: ActionType,
        target: &str,
        os: OsFamily,
        inline_snippet: Option<&Snippet>,
    ) -> Result<Plan, ActionError> {
        if target.is_empty() {
            return Err(ActionError::InvalidTarget("target is required".into()));
        }
        if target.chars().any(char::is_control) {
            return Err(ActionError::InvalidTarget(
                "target has control characters".into(),
            ));
        }
        match action {
            ActionType::ServiceStart => self.plan_service(ServiceOp::Start, target),
            ActionType::ServiceStop => self.plan_service(ServiceOp::Stop, target),
            ActionType::ServiceRestart => self.plan_service(ServiceOp::Restart, target),
            ActionType::PortFree => self.plan_port(target, os),
            ActionType::UpdatesInstall => self.plan_updates(target),
            ActionType::SpaceClean => plan_clean(target, os),
            ActionType::SnippetRun => self.plan_snippet(target, os, inline_snippet),
        }
    }

    fn find_service(&self, target: &str) -> Result<HostService, ActionError> {
        let list = self
            .exec
            .services()
            .map_err(|e| ActionError::Unsupported(e.to_string()))?;
        if let Some(s) = list.iter().find(|s| s.unit == target) {
            return Ok(s.clone());
        }
        let by_name: Vec<&HostService> = list.iter().filter(|s| s.name == target).collect();
        match by_name.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(ActionError::TargetNotFound(format!(
                "no service {target:?} on this computer"
            ))),
            _ => Err(ActionError::InvalidTarget(format!(
                "{target:?} matches more than one service; use the unit"
            ))),
        }
    }

    fn plan_service(&self, op: ServiceOp, target: &str) -> Result<Plan, ActionError> {
        let svc = self.find_service(target)?;
        if op != ServiceOp::Start && svc.pid.is_some_and(|p| p == self.exec.own_pid()) {
            return Err(ActionError::Refused(format!(
                "{} runs this console's runtime; stopping it would cut the console off",
                svc.name
            )));
        }
        let mut plan = Plan::new(Vec::new());
        plan.before = Some(json!({ "state": svc.state, "provider": svc.provider, "pid": svc.pid }));
        plan.after = After::Service(svc.unit.clone());

        let already = match op {
            ServiceOp::Start => svc.state == ServiceState::Running,
            ServiceOp::Stop => svc.state == ServiceState::Stopped,
            ServiceOp::Restart => false,
        };
        if already {
            plan.note = Some(format!(
                "Nothing to do: {} was already {}.",
                svc.name,
                if op == ServiceOp::Start {
                    "running"
                } else {
                    "stopped"
                }
            ));
            return Ok(plan);
        }

        let (steps, needs_elevation) = service_steps(&svc, op)?;
        plan.steps = steps;
        plan.needs_elevation = needs_elevation;
        plan.helper = vec![HelperRequest::Service {
            op,
            provider: svc.provider,
            unit: svc.unit.clone(),
        }];
        plan.elevation_reason = format!(
            "{} services need admin rights to {}",
            provider_name(svc.provider),
            op.verb()
        );
        match op {
            ServiceOp::Start => {
                plan.undo = Some(UndoAction {
                    action_type: ActionType::ServiceStop,
                    target: svc.unit.clone(),
                    label: format!("Stop {} again", svc.name),
                });
            }
            ServiceOp::Stop if svc.state == ServiceState::Running => {
                plan.undo = Some(UndoAction {
                    action_type: ActionType::ServiceStart,
                    target: svc.unit.clone(),
                    label: format!("Start {} again", svc.name),
                });
            }
            ServiceOp::Stop => {
                plan.note = Some(format!(
                    "Not undoable: {} was not running before, so there is no running state to go back to.",
                    svc.name
                ));
            }
            ServiceOp::Restart => {
                plan.note =
                    Some("Not undoable: a restart has no earlier state to go back to.".into());
            }
        }
        Ok(plan)
    }

    fn plan_port(&self, target: &str, os: OsFamily) -> Result<Plan, ActionError> {
        let port: u16 = target.parse().ok().filter(|p| *p > 0).ok_or_else(|| {
            ActionError::InvalidTarget(format!("{target:?} is not a port number"))
        })?;
        let listeners: Vec<HostPort> = self
            .exec
            .ports()
            .map_err(|e| ActionError::Unsupported(e.to_string()))?
            .into_iter()
            .filter(|p| p.port == port)
            .collect();
        if listeners.is_empty() {
            return Err(ActionError::TargetNotFound(format!(
                "nothing listens on port {port}"
            )));
        }
        let mut pids: Vec<u32> = listeners.iter().filter_map(|p| p.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        if pids.is_empty() {
            if self.exec.elevated() {
                return Err(ActionError::Refused(format!(
                    "the process on port {port} could not be identified"
                )));
            }
            // Another user's process: only the admin helper can see and stop
            // it. The helper looks the listeners up again itself.
            let mut plan = Plan::new(Vec::new());
            plan.needs_elevation = true;
            plan.elevation_reason = format!("the process on port {port} belongs to another user");
            plan.helper = vec![HelperRequest::PortFree { port }];
            plan.before = Some(json!({ "port": port, "pids": [], "process": null }));
            plan.after = After::Port(port);
            plan.note = Some(format!(
                "Not undoable: the process that held port {port} was stopped. Start it again yourself if you need it."
            ));
            return Ok(plan);
        }
        if pids.iter().any(|p| *p == self.exec.own_pid()) {
            return Err(ActionError::Refused(format!(
                "port {port} is served by this console's runtime; freeing it would cut the console off"
            )));
        }
        if let Some(p) = pids.iter().find(|p| **p <= 1) {
            return Err(ActionError::Refused(format!(
                "port {port} is held by system process {p}"
            )));
        }
        let process = listeners.iter().find_map(|p| p.process.clone());
        let before = json!({ "port": port, "pids": pids, "process": process });

        // A port held by a known service is freed by stopping the service, so
        // its manager does not restart it and the undo can start it again.
        let services = self.exec.services().unwrap_or_default();
        if let Some(svc) = services
            .iter()
            .find(|s| s.state == ServiceState::Running && s.pid.is_some_and(|p| pids.contains(&p)))
        {
            let (steps, needs_elevation) = service_steps(svc, ServiceOp::Stop)?;
            let mut plan = Plan::new(steps);
            plan.needs_elevation = needs_elevation;
            plan.helper = vec![HelperRequest::Service {
                op: ServiceOp::Stop,
                provider: svc.provider,
                unit: svc.unit.clone(),
            }];
            plan.elevation_reason = format!(
                "{} services need admin rights to stop",
                provider_name(svc.provider)
            );
            plan.before = Some(
                json!({ "port": port, "pids": pids, "process": process, "service": svc.unit }),
            );
            plan.after = After::Port(port);
            plan.undo = Some(UndoAction {
                action_type: ActionType::ServiceStart,
                target: svc.unit.clone(),
                label: format!("Start {} again (it held port {port})", svc.name),
            });
            return Ok(plan);
        }

        let steps = pids
            .iter()
            .map(|pid| {
                let pid = pid.to_string();
                match os {
                    OsFamily::Windows => Step::new(
                        "taskkill",
                        &["/PID", pid.as_str(), "/T", "/F"],
                        KILL_TIMEOUT,
                    ),
                    OsFamily::Linux | OsFamily::Macos => {
                        Step::new("kill", &["-TERM", pid.as_str()], KILL_TIMEOUT)
                    }
                }
            })
            .collect();
        let mut plan = Plan::new(steps);
        plan.helper = vec![HelperRequest::PortFree { port }];
        plan.before = Some(before);
        plan.after = After::Port(port);
        plan.note = Some(format!(
            "Not undoable: the process{} that held port {port} {} stopped. Start it again yourself if you need it.",
            process.as_deref().map(|p| format!(" {p}")).unwrap_or_default(),
            if pids.len() > 1 { "were" } else { "was" }
        ));
        Ok(plan)
    }

    fn plan_updates(&self, target: &str) -> Result<Plan, ActionError> {
        let all = self
            .exec
            .updates()
            .map_err(|e| ActionError::Unsupported(e.to_string()))?;
        let chosen: Vec<HostUpdate> = if target == "all-security" {
            all.into_iter().filter(|u| u.security).collect()
        } else {
            all.into_iter().filter(|u| u.id == target).collect()
        };
        if chosen.is_empty() {
            return Err(ActionError::TargetNotFound(if target == "all-security" {
                "no security updates are waiting".into()
            } else {
                format!("no waiting update {target:?}")
            }));
        }
        if let Some(bad) = chosen
            .iter()
            .find(|u| u.id.starts_with('-') || u.id.chars().any(char::is_control))
        {
            return Err(ActionError::InvalidTarget(format!(
                "update id {:?} cannot be passed safely",
                bad.id
            )));
        }

        let mut by_source: BTreeMap<u8, (UpdateSource, Vec<&str>)> = BTreeMap::new();
        for u in &chosen {
            by_source
                .entry(source_rank(u.source))
                .or_insert_with(|| (u.source, Vec::new()))
                .1
                .push(u.id.as_str());
        }
        let mut steps = Vec::new();
        let mut helper = Vec::new();
        for (source, ids) in by_source.values() {
            steps.push(update_step(*source, ids)?);
            helper.push(HelperRequest::UpdatesInstall {
                source: *source,
                ids: ids.iter().map(|s| s.to_string()).collect(),
            });
        }
        let mut plan = Plan::new(steps);
        plan.helper = helper;
        plan.needs_elevation = true;
        plan.elevation_reason = "installing updates needs admin rights".into();
        plan.before = Some(json!({
            "updates": chosen
                .iter()
                .map(|u| json!({ "id": u.id, "current": u.current, "available": u.available, "source": u.source }))
                .collect::<Vec<_>>()
        }));
        let restart = chosen.iter().any(|u| u.requires_restart);
        plan.note = Some(format!(
            "Not undoable: installed updates are not rolled back; the restore point is the way back.{}",
            if restart { " A restart is needed to finish; it was not done for you." } else { "" }
        ));
        Ok(plan)
    }

    fn plan_snippet(
        &self,
        target: &str,
        os: OsFamily,
        inline: Option<&Snippet>,
    ) -> Result<Plan, ActionError> {
        if !safe_file_name(target) {
            return Err(ActionError::InvalidTarget(format!(
                "{target:?} is not a snippet id"
            )));
        }
        let snippet: Snippet = if let Some(inline) = inline {
            if inline.id != target {
                return Err(ActionError::InvalidTarget(format!(
                    "the snippet sent with the request is {:?}, not {target:?}",
                    inline.id
                )));
            }
            inline.clone()
        } else {
            let path = self.store.snippets_dir().join(format!("{target}.json"));
            match std::fs::read(&path) {
                Ok(b) => serde_json::from_slice(&b).map_err(|e| {
                    ActionError::InvalidTarget(format!("snippet {target:?} is unreadable: {e}"))
                })?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ActionError::TargetNotFound(format!(
                        "no saved snippet {target:?} on this computer"
                    )))
                }
                Err(e) => return Err(ActionError::Store(format!("read {}: {e}", path.display()))),
            }
        };
        if snippet.script.trim().is_empty() {
            return Err(ActionError::InvalidTarget(format!(
                "snippet {target:?} is empty"
            )));
        }
        if snippet.script.len() > SNIPPET_MAX_LEN {
            return Err(ActionError::InvalidTarget(format!(
                "snippet {target:?} is longer than 64 KB"
            )));
        }
        let timeout = snippet
            .timeout_sec
            .map(Duration::from_secs)
            .unwrap_or(SNIPPET_DEFAULT_TIMEOUT)
            .min(SNIPPET_MAX_TIMEOUT);
        let step = match os {
            OsFamily::Windows => Step::new(
                "powershell",
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    snippet.script.as_str(),
                ],
                timeout,
            ),
            OsFamily::Linux | OsFamily::Macos => {
                Step::new("sh", &["-c", snippet.script.as_str()], timeout)
            }
        };
        let mut plan = Plan::new(vec![step]);
        plan.needs_elevation = snippet.elevated;
        plan.elevation_reason =
            format!("snippet {} is marked as needing admin rights", snippet.name);
        plan.before = Some(json!({ "snippet": snippet.id, "name": snippet.name }));
        plan.note = Some("Not undoable: what a snippet changes is not tracked.".into());
        Ok(plan)
    }

    // -- running -----------------------------------------------------------

    fn execute(&self, steps: &[Step]) -> Outcome {
        let mut combined = String::new();
        for step in steps {
            let args: Vec<&str> = step.args.iter().map(String::as_str).collect();
            match self.exec.run(&step.program, &args, step.timeout) {
                Ok(out) => {
                    let text = format!("{}{}", out.stdout, out.stderr);
                    combined.push_str(&text);
                    let ok = out.code == Some(0)
                        && !(step.ok == OkCheck::ExitZeroNoErrorWord && text.contains("ERROR"));
                    if !ok {
                        let reason = cmd_first_line(&out);
                        let failure = if looks_like_permission_error(&text) {
                            Failure::Permission(format!("{}: {reason}", step.program))
                        } else {
                            Failure::Other(format!(
                                "{} exited with {}: {reason}",
                                step.program,
                                out.code
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| "a signal".into())
                            ))
                        };
                        return Outcome {
                            output: mask_output(&combined),
                            failure: Some(failure),
                        };
                    }
                }
                Err(e) => {
                    let msg = match e {
                        RunError::Timeout => format!(
                            "{} timed out after {}s; it may have partly run",
                            step.program,
                            step.timeout.as_secs()
                        ),
                        other => other.reason(&step.program),
                    };
                    return Outcome {
                        output: mask_output(&combined),
                        failure: Some(Failure::Other(msg)),
                    };
                }
            }
        }
        Outcome {
            output: mask_output(&combined),
            failure: None,
        }
    }

    /// Run the plan through the admin helper (it re-validates each request).
    fn execute_helper(&self, reqs: &[HelperRequest]) -> Outcome {
        let mut combined = String::new();
        for req in reqs {
            let name = crate::helper::HELPER_BIN;
            match self.exec.run_helper(req, req.timeout()) {
                Ok(out) => {
                    let text = format!("{}{}", out.stdout, out.stderr);
                    combined.push_str(&text);
                    if out.code == Some(0) {
                        continue;
                    }
                    let reason = cmd_first_line(&out);
                    let lower = text.to_lowercase();
                    let failure = if lower.contains("password is required")
                        || lower.contains("no tty present")
                        || out.code == Some(crate::helper::EXIT_NOT_ROOT)
                    {
                        // sudo asked for a password: the setup is gone or broken.
                        Failure::Permission(format!(
                            "the admin helper is not set up on this computer: {reason}"
                        ))
                    } else if out.code == Some(crate::helper::EXIT_REFUSED)
                        || out.code == Some(crate::helper::EXIT_NOT_FOUND)
                    {
                        Failure::Other(format!("{name} refused the request: {reason}"))
                    } else {
                        Failure::Other(format!(
                            "{} exited with {}: {reason}",
                            req.display(),
                            out.code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "a signal".into())
                        ))
                    };
                    return Outcome {
                        output: mask_output(&combined),
                        failure: Some(failure),
                    };
                }
                Err(RunError::NotFound) => {
                    return Outcome {
                        output: mask_output(&combined),
                        failure: Some(Failure::Permission(
                            "the admin helper is not reachable; set up admin access again".into(),
                        )),
                    }
                }
                Err(e) => {
                    let msg = match e {
                        RunError::Timeout => format!(
                            "{} timed out after {}s; it may have partly run",
                            req.display(),
                            req.timeout().as_secs()
                        ),
                        other => other.reason(name),
                    };
                    return Outcome {
                        output: mask_output(&combined),
                        failure: Some(Failure::Other(msg)),
                    };
                }
            }
        }
        Outcome {
            output: mask_output(&combined),
            failure: None,
        }
    }

    fn read_after(&self, after: &After) -> Option<Value> {
        match after {
            After::Nothing => None,
            After::Service(unit) => {
                let list = self.exec.services().ok()?;
                let s = list.iter().find(|s| &s.unit == unit)?;
                Some(json!({ "state": s.state, "pid": s.pid }))
            }
            After::Port(port) => {
                let list = self.exec.ports().ok()?;
                Some(json!({ "port": port, "listening": list.iter().any(|p| p.port == *port) }))
            }
        }
    }
}

/// A service operation (also the helper's `service <op>` argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    Start,
    Stop,
    Restart,
}

impl ServiceOp {
    pub fn verb(self) -> &'static str {
        match self {
            ServiceOp::Start => "start",
            ServiceOp::Stop => "stop",
            ServiceOp::Restart => "restart",
        }
    }
}

fn provider_name(p: ServiceProvider) -> &'static str {
    match p {
        ServiceProvider::Systemd => "systemd",
        ServiceProvider::Supervisord => "supervisord",
        ServiceProvider::Docker => "Docker",
        ServiceProvider::Launchd => "launchd",
        ServiceProvider::Scm => "Windows",
    }
}

/// Commands for one service operation, and whether they need elevation up
/// front. Supervisord, Docker and launchd user jobs are tried as is; a
/// permission error at run time becomes [`ActionError::NeedsElevation`].
pub(crate) fn service_steps(
    svc: &HostService,
    op: ServiceOp,
) -> Result<(Vec<Step>, bool), ActionError> {
    let unit = svc.unit.as_str();
    let ok = match svc.provider {
        ServiceProvider::Scm => valid_provider(unit),
        _ => valid_name(unit),
    };
    if !ok {
        return Err(ActionError::InvalidTarget(format!(
            "service unit {unit:?} cannot be passed safely"
        )));
    }
    let verb = op.verb();
    let t = SERVICE_TIMEOUT;
    Ok(match svc.provider {
        ServiceProvider::Systemd => (vec![Step::new("systemctl", &[verb, unit], t)], true),
        ServiceProvider::Supervisord => {
            let mut s = Step::new("supervisorctl", &[verb, unit], t);
            s.ok = OkCheck::ExitZeroNoErrorWord;
            (vec![s], false)
        }
        ServiceProvider::Docker => (vec![Step::new("docker", &[verb, unit], t)], false),
        ServiceProvider::Launchd => {
            let steps = match op {
                ServiceOp::Start => vec![Step::new("launchctl", &["start", unit], t)],
                ServiceOp::Stop => vec![Step::new("launchctl", &["stop", unit], t)],
                ServiceOp::Restart => vec![
                    Step::new("launchctl", &["stop", unit], t),
                    Step::new("launchctl", &["start", unit], t),
                ],
            };
            (steps, false)
        }
        // `net start|stop` wait for the service to reach the new state.
        ServiceProvider::Scm => {
            let steps = match op {
                ServiceOp::Start => vec![Step::new("net", &["start", unit], t)],
                ServiceOp::Stop => vec![Step::new("net", &["stop", unit], t)],
                ServiceOp::Restart => vec![
                    Step::new("net", &["stop", unit], t),
                    Step::new("net", &["start", unit], t),
                ],
            };
            (steps, true)
        }
    })
}

fn source_rank(s: UpdateSource) -> u8 {
    match s {
        UpdateSource::Apt => 0,
        UpdateSource::Dnf => 1,
        UpdateSource::Softwareupdate => 2,
        UpdateSource::Windows => 3,
    }
}

pub(crate) fn update_step(source: UpdateSource, ids: &[&str]) -> Result<Step, ActionError> {
    let t = UPDATE_TIMEOUT;
    Ok(match source {
        UpdateSource::Apt => {
            let mut args = vec![
                "DEBIAN_FRONTEND=noninteractive",
                "apt-get",
                "install",
                "-y",
                "--only-upgrade",
                "-o",
                "Dpkg::Options::=--force-confdef",
                "-o",
                "Dpkg::Options::=--force-confold",
            ];
            args.extend_from_slice(ids);
            Step::new("env", &args, t)
        }
        UpdateSource::Dnf => {
            let mut args = vec!["upgrade", "-y"];
            args.extend_from_slice(ids);
            Step::new("dnf", &args, t)
        }
        UpdateSource::Softwareupdate => {
            let mut args = vec!["--install"];
            args.extend_from_slice(ids);
            Step::new("softwareupdate", &args, t)
        }
        UpdateSource::Windows => {
            let kbs: Vec<&str> = ids.iter().copied().filter(|id| is_kb(id)).collect();
            if kbs.len() != ids.len() {
                return Err(ActionError::Unsupported(
                    "installing a Windows update that has no KB number".into(),
                ));
            }
            Step::new(
                "powershell",
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    windows_update_script(&kbs).as_str(),
                ],
                t,
            )
        }
    })
}

/// `KB` followed by 4 to 10 digits; nothing else is put into the script.
fn is_kb(s: &str) -> bool {
    s.len() >= 6
        && s.len() <= 12
        && s.starts_with("KB")
        && s[2..].chars().all(|c| c.is_ascii_digit())
}

/// Windows Update Agent COM script that installs exactly the given KBs.
fn windows_update_script(kbs: &[&str]) -> String {
    let list = kbs
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$ErrorActionPreference='Stop'; $ids=@({list}); \
$s=New-Object -ComObject Microsoft.Update.Session; \
$r=$s.CreateUpdateSearcher().Search('IsInstalled=0 and IsHidden=0'); \
$c=New-Object -ComObject Microsoft.Update.UpdateColl; \
foreach($u in $r.Updates){{foreach($k in $u.KBArticleIDs){{if($ids -contains ('KB'+$k)){{if(-not $u.EulaAccepted){{$u.AcceptEula()}}; [void]$c.Add($u)}}}}}}; \
if($c.Count -eq 0){{Write-Error 'no matching updates'; exit 2}}; \
$d=$s.CreateUpdateDownloader(); $d.Updates=$c; [void]$d.Download(); \
$i=$s.CreateUpdateInstaller(); $i.Updates=$c; $res=$i.Install(); \
\"ResultCode=$($res.ResultCode) RebootRequired=$($res.RebootRequired)\"; \
if($res.ResultCode -ne 2 -and $res.ResultCode -ne 3){{exit 1}}"
    )
}

fn plan_clean(target: &str, os: OsFamily) -> Result<Plan, ActionError> {
    let item = CLEAN_ITEMS
        .iter()
        .find(|i| i.id == target)
        .ok_or_else(|| ActionError::TargetNotFound(format!("no clean-up item {target:?}")))?;
    if item.os.is_some_and(|o| o != os) {
        return Err(ActionError::Unsupported(format!(
            "clean-up item {} on this OS",
            item.id
        )));
    }
    let mut plan = Plan::new(vec![Step::new(item.program, item.args, CLEAN_TIMEOUT)]);
    plan.needs_elevation = item.needs_elevation;
    if item.needs_elevation {
        plan.helper = vec![HelperRequest::Clean {
            item: item.id.to_string(),
        }];
    }
    plan.elevation_reason = format!("cleaning {} needs admin rights", item.label.to_lowercase());
    plan.before = Some(json!({ "item": item.id, "label": item.label }));
    plan.note = Some("Not undoable: the cleaned files are deleted.".into());
    Ok(plan)
}

enum Failure {
    Permission(String),
    Other(String),
}

struct Outcome {
    output: Option<String>,
    failure: Option<Failure>,
}

fn looks_like_permission_error(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "permission denied",
        "access is denied",
        "access denied",
        "operation not permitted",
        "interactive authentication required",
        "authentication is required",
        "must be root",
        "must be run as root",
        "are you root",
        "requires root",
        "requires elevation",
        "run as administrator",
        "superuser privileges",
        "not privileged",
    ]
    .iter()
    .any(|p| t.contains(p))
}

fn cmd_first_line(out: &CmdOutput) -> String {
    let from = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    let line = crate::cmd::first_line(from);
    truncate_chars(&mask_line(&line, &MaskOptions::default()), 300)
}

/// Masked tail of the output, at most [`OUTPUT_TAIL`] characters.
fn mask_output(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        return None;
    }
    let opts = MaskOptions::default();
    let masked: Vec<String> = s.lines().map(|l| mask_line(l, &opts)).collect();
    let joined = masked.join("\n");
    let n = joined.chars().count();
    Some(if n > OUTPUT_TAIL {
        joined.chars().skip(n - OUTPUT_TAIL).collect()
    } else {
        joined
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `chg_<UTC yyyymmddThhmmssZ>_<12 hex>`: sorts by time, safe as a file name.
fn new_change_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "chg_{}_{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        &hex[..12]
    )
}

// ---------------------------------------------------------------------------
// Tests (fake executor only; no real command runs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{Exposure, Protocol};
    use crate::updates::UpdateKind;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    struct Fake {
        os: OsFamily,
        elevated: bool,
        own_pid: u32,
        services: Mutex<Vec<HostService>>,
        ports: Vec<HostPort>,
        updates: Vec<HostUpdate>,
        replies: Mutex<VecDeque<Result<CmdOutput, RunError>>>,
        calls: Mutex<Vec<String>>,
        /// Store root, to check the record is `pending` when a command runs.
        root: PathBuf,
        pending_seen: Mutex<Vec<bool>>,
        helper_ready: bool,
        helper_replies: Mutex<VecDeque<Result<CmdOutput, RunError>>>,
        helper_calls: Mutex<Vec<Vec<String>>>,
    }

    impl Fake {
        fn new(os: OsFamily, root: &Path) -> Fake {
            Fake {
                os,
                elevated: true,
                own_pid: 4242,
                services: Mutex::new(Vec::new()),
                ports: Vec::new(),
                updates: Vec::new(),
                replies: Mutex::new(VecDeque::new()),
                calls: Mutex::new(Vec::new()),
                root: root.to_path_buf(),
                pending_seen: Mutex::new(Vec::new()),
                helper_ready: false,
                helper_replies: Mutex::new(VecDeque::new()),
                helper_calls: Mutex::new(Vec::new()),
            }
        }
        fn helper_reply(&self, code: i32, stdout: &str, stderr: &str) {
            self.helper_replies.lock().unwrap().push_back(Ok(CmdOutput {
                code: Some(code),
                stdout: stdout.into(),
                stderr: stderr.into(),
            }));
        }
        fn helper_calls(&self) -> Vec<Vec<String>> {
            self.helper_calls.lock().unwrap().clone()
        }
        fn reply(&self, code: i32, stdout: &str, stderr: &str) {
            self.replies.lock().unwrap().push_back(Ok(CmdOutput {
                code: Some(code),
                stdout: stdout.into(),
                stderr: stderr.into(),
            }));
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Executor for Fake {
        fn os(&self) -> Option<OsFamily> {
            Some(self.os)
        }
        fn elevated(&self) -> bool {
            self.elevated
        }
        fn own_pid(&self) -> u32 {
            self.own_pid
        }
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _timeout: Duration,
        ) -> Result<CmdOutput, RunError> {
            let mut line = program.to_string();
            for a in args {
                line.push(' ');
                line.push_str(a);
            }
            self.calls.lock().unwrap().push(line);
            let pending = ChangeStore::new(&self.root)
                .list(100)
                .unwrap()
                .iter()
                .any(|r| r.status == ChangeStatus::Pending);
            self.pending_seen.lock().unwrap().push(pending);
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(CmdOutput {
                    code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                }))
        }
        fn services(&self) -> crate::Result<Vec<HostService>> {
            Ok(self.services.lock().unwrap().clone())
        }
        fn ports(&self) -> crate::Result<Vec<HostPort>> {
            Ok(self.ports.clone())
        }
        fn updates(&self) -> crate::Result<Vec<HostUpdate>> {
            Ok(self.updates.clone())
        }
        fn helper_ready(&self) -> bool {
            self.helper_ready
        }
        fn run_helper(
            &self,
            req: &HelperRequest,
            _timeout: Duration,
        ) -> Result<CmdOutput, RunError> {
            // What the real helper would accept: the argv must parse.
            let argv = req.to_argv();
            assert_eq!(&HelperRequest::parse(argv.as_slice()).unwrap(), req);
            self.helper_calls.lock().unwrap().push(argv);
            self.helper_replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(CmdOutput {
                    code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                }))
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "host-probe-actions-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn svc(
        provider: ServiceProvider,
        name: &str,
        unit: &str,
        state: ServiceState,
        pid: Option<u32>,
    ) -> HostService {
        let mut s = HostService::new(provider, name, unit, state);
        s.pid = pid;
        s
    }

    fn port(n: u16, pid: Option<u32>, process: Option<&str>) -> HostPort {
        HostPort {
            port: n,
            protocol: Protocol::Tcp,
            address: "0.0.0.0".into(),
            process: process.map(str::to_string),
            pid,
            exposure: Exposure::Public,
            mesh: false,
        }
    }

    fn update(source: UpdateSource, id: &str, security: bool) -> HostUpdate {
        HostUpdate {
            id: id.into(),
            name: id.into(),
            current: Some("1.0".into()),
            available: "1.1".into(),
            security,
            kind: UpdateKind::Package,
            source,
            cves: Vec::new(),
            requires_restart: false,
            recommended: false,
        }
    }

    fn req(t: &str, target: &str, approval: Option<&str>) -> ActionRequest {
        ActionRequest {
            action_type: t.into(),
            target: target.into(),
            approval_id: approval.map(str::to_string),
            restore_point: None,
            snippet: None,
        }
    }

    fn engine(f: Fake) -> ActionEngine<Fake> {
        let store = ChangeStore::new(&f.root);
        ActionEngine::new(f, store).with_verifier(Box::new(WellFormedApproval))
    }

    fn nginx_linux(tag: &str, state: ServiceState) -> ActionEngine<Fake> {
        let root = temp_root(tag);
        let f = Fake::new(OsFamily::Linux, &root);
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Systemd,
            "nginx",
            "nginx.service",
            state,
            Some(900),
        ));
        engine(f)
    }

    fn cleanup<E: Executor>(e: &ActionEngine<E>) {
        let _ = std::fs::remove_dir_all(e.store().root());
    }

    #[test]
    fn unknown_type_is_400_and_runs_nothing() {
        let e = nginx_linux("unknown", ServiceState::Running);
        let err = e
            .run(&req("service.delete", "nginx.service", Some("ap1")))
            .unwrap_err();
        assert_eq!(err.http_status(), 400);
        assert_eq!(err.code(), "unknown_type");
        assert!(e.exec.calls().is_empty());
        assert!(e.store().list(10).unwrap().is_empty());
        cleanup(&e);
    }

    #[test]
    fn missing_or_malformed_approval_is_403_and_changes_nothing() {
        let e = nginx_linux("noapproval", ServiceState::Running);
        for a in [None, Some(""), Some("  "), Some("../x"), Some("a b")] {
            let err = e.run(&req("service.stop", "nginx.service", a)).unwrap_err();
            assert_eq!(err.http_status(), 403, "{a:?}");
        }
        assert!(e.exec.calls().is_empty());
        assert!(e.store().list(10).unwrap().is_empty());
        cleanup(&e);
    }

    #[test]
    fn verifier_can_refuse() {
        struct No;
        impl ApprovalVerifier for No {
            fn verify(&self, _: &str, _: ActionType, _: &str) -> Result<(), String> {
                Err("approval is for another computer".into())
            }
        }
        let e = nginx_linux("verifier", ServiceState::Running).with_verifier(Box::new(No));
        let err = e
            .run(&req("service.stop", "nginx.service", Some("ap1")))
            .unwrap_err();
        assert!(matches!(err, ActionError::NotApproved(ref m) if m.contains("another computer")));
        assert!(e.exec.calls().is_empty());
        cleanup(&e);
    }

    #[test]
    fn the_default_verifier_refuses_every_id() {
        let root = temp_root("deny-default");
        let f = Fake::new(OsFamily::Linux, &root);
        let store = ChangeStore::new(&f.root);
        let e = ActionEngine::new(f, store);
        let err = e
            .run(&req("service.stop", "nginx.service", Some("ap-any")))
            .unwrap_err();
        assert_eq!(err.http_status(), 403);
        assert!(e.exec.calls().is_empty());
        cleanup(&e);
    }

    #[test]
    fn cloud_approval_runs_once_and_a_replay_is_refused() {
        use crate::approval::{mint_for_test, relay_key_from_device_token, RelayKeyApproval};
        let key = relay_key_from_device_token("tok");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let id = mint_for_test(
            &key,
            "rt_box",
            "service.stop",
            "nginx.service",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            now + 120,
        );
        let e = nginx_linux("cloud-approval", ServiceState::Running)
            .with_verifier(Box::new(RelayKeyApproval::new(key.clone(), "rt_box")));
        // Bound to another target: refused, nothing ran, the id is not spent.
        let err = e
            .run(&req("service.stop", "sshd.service", Some(&id)))
            .unwrap_err();
        assert_eq!(err.code(), "not_approved");
        assert!(e.exec.calls().is_empty());
        // The real action passes once.
        e.run(&req("service.stop", "nginx.service", Some(&id)))
            .unwrap();
        assert_eq!(e.exec.calls(), vec!["systemctl stop nginx.service"]);
        // A second use of the same id is refused by the store claim.
        let err = e
            .run(&req("service.stop", "nginx.service", Some(&id)))
            .unwrap_err();
        assert_eq!(err.http_status(), 403);
        assert!(err.to_string().contains("already used"), "{err}");
        assert_eq!(e.exec.calls().len(), 1);
        cleanup(&e);
    }

    #[test]
    fn systemd_stop_writes_record_first_then_acts_with_undo() {
        let e = nginx_linux("stop", ServiceState::Running);
        let r = e
            .run(&ActionRequest {
                restore_point: Some("orgo-backup-2026-11-20".into()),
                ..req("service.stop", "nginx.service", Some("ap-stop"))
            })
            .unwrap();
        assert_eq!(e.exec.calls(), vec!["systemctl stop nginx.service"]);
        assert_eq!(
            *e.exec.pending_seen.lock().unwrap(),
            vec![true],
            "record was pending before the command ran"
        );
        let undo = r.undo.clone().unwrap();
        assert_eq!(undo.action_type, ActionType::ServiceStart);
        assert_eq!(undo.target, "nginx.service");

        let rec = e.store().get(&r.change_id).unwrap().unwrap();
        assert_eq!(rec.status, ChangeStatus::Succeeded);
        assert_eq!(rec.approval_id, "ap-stop");
        assert_eq!(rec.restore_point.as_deref(), Some("orgo-backup-2026-11-20"));
        assert_eq!(rec.before.as_ref().unwrap()["state"], "running");
        assert_eq!(rec.commands, vec!["systemctl stop nginx.service"]);
        assert_eq!(rec.undo, r.undo);
        assert!(rec.finished_at.is_some());
        cleanup(&e);
    }

    #[test]
    fn approval_is_single_use() {
        let e = nginx_linux("reuse", ServiceState::Running);
        e.run(&req("service.restart", "nginx.service", Some("ap-once")))
            .unwrap();
        let err = e
            .run(&req("service.restart", "nginx.service", Some("ap-once")))
            .unwrap_err();
        assert!(matches!(err, ActionError::NotApproved(ref m) if m.contains("already used")));
        assert_eq!(e.exec.calls().len(), 1);
        cleanup(&e);
    }

    #[test]
    fn systemd_without_root_needs_elevation_and_keeps_the_approval() {
        let root = temp_root("elev");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Systemd,
            "nginx",
            "nginx.service",
            ServiceState::Running,
            Some(900),
        ));
        let e = engine(f);
        let err = e
            .run(&req("service.stop", "nginx.service", Some("ap-elev")))
            .unwrap_err();
        assert_eq!(err.code(), "needs_elevation");
        assert_eq!(err.http_status(), 403);
        assert!(e.exec.calls().is_empty(), "nothing ran");
        assert!(
            e.store().list(10).unwrap().is_empty(),
            "no record for a change that never started"
        );
        assert!(
            e.store().claim_approval("ap-elev", "x").unwrap(),
            "approval still unused"
        );
        cleanup(&e);
    }

    #[test]
    fn systemd_without_root_goes_through_the_installed_helper() {
        let root = temp_root("helper-svc");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.helper_ready = true;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Systemd,
            "nginx",
            "nginx.service",
            ServiceState::Running,
            Some(900),
        ));
        let e = engine(f);
        let res = e
            .run(&req("service.stop", "nginx.service", Some("ap-h1")))
            .unwrap();
        assert!(
            e.exec.calls().is_empty(),
            "no direct systemctl without root"
        );
        assert_eq!(
            e.exec.helper_calls(),
            vec![vec!["service", "stop", "systemd", "nginx.service"]]
        );
        let undo = res.undo.expect("stop of a running service has an undo");
        assert_eq!(undo.action_type, ActionType::ServiceStart);
        let rec = e.store().get(&res.change_id).unwrap().unwrap();
        assert_eq!(rec.status, ChangeStatus::Succeeded);
        assert_eq!(
            rec.commands,
            vec!["allternit-host-helper service stop systemd nginx.service"]
        );
        assert!(
            !e.store().claim_approval("ap-h1", "x").unwrap(),
            "approval was used"
        );
        cleanup(&e);
    }

    #[test]
    fn helper_never_runs_snippets() {
        let root = temp_root("helper-snip");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.helper_ready = true;
        let e = engine(f);
        std::fs::create_dir_all(e.store().snippets_dir()).unwrap();
        std::fs::write(
            e.store().snippets_dir().join("s1.json"),
            r#"{"id":"s1","name":"wipe","script":"rm -rf /var/tmp/x","elevated":true}"#,
        )
        .unwrap();
        let err = e.run(&req("snippet.run", "s1", Some("ap-h2"))).unwrap_err();
        assert_eq!(err.code(), "needs_elevation");
        assert!(e.exec.calls().is_empty());
        assert!(e.exec.helper_calls().is_empty(), "helper got nothing");
        assert!(e.store().claim_approval("ap-h2", "x").unwrap());
        cleanup(&e);
    }

    #[test]
    fn run_time_permission_error_retries_through_the_helper() {
        let root = temp_root("helper-docker");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.helper_ready = true;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Docker,
            "web",
            "abc123",
            ServiceState::Running,
            None,
        ));
        f.reply(
            1,
            "",
            "permission denied while trying to connect to the Docker daemon socket",
        );
        let e = engine(f);
        let res = e
            .run(&req("service.stop", "abc123", Some("ap-h3")))
            .unwrap();
        assert_eq!(e.exec.calls(), vec!["docker stop abc123"]);
        assert_eq!(
            e.exec.helper_calls(),
            vec![vec!["service", "stop", "docker", "abc123"]]
        );
        let rec = e.store().get(&res.change_id).unwrap().unwrap();
        assert_eq!(rec.status, ChangeStatus::Succeeded);
        assert_eq!(rec.commands.len(), 2, "both attempts are on the record");
        cleanup(&e);
    }

    #[test]
    fn helper_asking_for_a_password_is_needs_elevation_and_frees_the_approval() {
        let root = temp_root("helper-pw");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.helper_ready = true;
        f.helper_reply(1, "", "sudo: a password is required");
        let e = engine(f);
        let err = e
            .run(&req("space.clean", "apt-cache", Some("ap-h4")))
            .unwrap_err();
        let ActionError::NeedsElevation {
            change_id: Some(id),
            ..
        } = &err
        else {
            panic!("{err:?}")
        };
        assert_eq!(e.exec.helper_calls(), vec![vec!["clean", "apt-cache"]]);
        assert_eq!(
            e.store().get(id).unwrap().unwrap().status,
            ChangeStatus::NeedsElevation
        );
        assert!(e.store().claim_approval("ap-h4", "x").unwrap());
        cleanup(&e);
    }

    #[test]
    fn helper_refusal_is_a_failed_change() {
        let root = temp_root("helper-refuse");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.helper_ready = true;
        f.helper_reply(
            crate::helper::EXIT_NOT_FOUND,
            "",
            "not found: \"openssl\" is not a waiting apt update",
        );
        f.updates = vec![update(UpdateSource::Apt, "openssl", true)];
        let e = engine(f);
        let err = e
            .run(&req("updates.install", "openssl", Some("ap-h5")))
            .unwrap_err();
        assert_eq!(err.code(), "action_failed", "{err:?}");
        assert!(err.to_string().contains("refused"), "{err}");
        assert_eq!(
            e.exec.helper_calls(),
            vec![vec!["updates", "apt", "openssl"]]
        );
        cleanup(&e);
    }

    #[test]
    fn port_of_another_user_is_freed_by_the_helper() {
        let root = temp_root("helper-port");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.ports = vec![port(5432, None, None)];
        let no_helper = engine(f);
        assert_eq!(
            no_helper
                .run(&req("port.free", "5432", Some("ap-h6")))
                .unwrap_err()
                .code(),
            "needs_elevation"
        );
        cleanup(&no_helper);

        let root = temp_root("helper-port2");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.helper_ready = true;
        f.ports = vec![port(5432, None, None)];
        let e = engine(f);
        let res = e.run(&req("port.free", "5432", Some("ap-h7"))).unwrap();
        assert!(res.undo.is_none());
        assert_eq!(e.exec.helper_calls(), vec![vec!["port-free", "5432"]]);
        cleanup(&e);
    }

    #[test]
    fn restart_is_not_undoable() {
        let e = nginx_linux("restart", ServiceState::Running);
        let r = e
            .run(&req("service.restart", "nginx", Some("ap-r")))
            .unwrap();
        assert_eq!(
            e.exec.calls(),
            vec!["systemctl restart nginx.service"],
            "name resolves to the unit"
        );
        assert!(r.undo.is_none());
        assert!(r.note.unwrap().starts_with("Not undoable"));
        cleanup(&e);
    }

    #[test]
    fn start_when_running_does_nothing_but_records() {
        let e = nginx_linux("noop", ServiceState::Running);
        let r = e
            .run(&req("service.start", "nginx.service", Some("ap-n")))
            .unwrap();
        assert!(e.exec.calls().is_empty());
        assert!(r.undo.is_none());
        assert!(r.note.unwrap().contains("already running"));
        assert_eq!(
            e.store().get(&r.change_id).unwrap().unwrap().status,
            ChangeStatus::Succeeded
        );
        cleanup(&e);
    }

    #[test]
    fn start_offers_stop_as_undo() {
        let e = nginx_linux("start", ServiceState::Failed);
        let r = e
            .run(&req("service.start", "nginx.service", Some("ap-s")))
            .unwrap();
        assert_eq!(r.undo.unwrap().action_type, ActionType::ServiceStop);
        cleanup(&e);
    }

    #[test]
    fn unknown_service_is_404() {
        let e = nginx_linux("nosvc", ServiceState::Running);
        let err = e
            .run(&req("service.stop", "redis.service", Some("ap")))
            .unwrap_err();
        assert_eq!(err.http_status(), 404);
        cleanup(&e);
    }

    #[test]
    fn never_stops_its_own_runtime() {
        let root = temp_root("self");
        let f = Fake::new(OsFamily::Linux, &root);
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Systemd,
            "allternit-api",
            "allternit-api.service",
            ServiceState::Running,
            Some(4242),
        ));
        let e = engine(f);
        let err = e
            .run(&req("service.stop", "allternit-api.service", Some("ap")))
            .unwrap_err();
        assert_eq!(err.code(), "refused");
        assert!(e.exec.calls().is_empty());
        cleanup(&e);
    }

    #[test]
    fn supervisor_error_word_is_a_failure_with_a_record() {
        let root = temp_root("supervisor");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Supervisord,
            "vnc",
            "vnc",
            ServiceState::Stopped,
            None,
        ));
        f.reply(0, "vnc: ERROR (spawn error)\n", "");
        let e = engine(f);
        let err = e
            .run(&req("service.start", "vnc", Some("ap-sup")))
            .unwrap_err();
        let ActionError::Failed { change_id, .. } = &err else {
            panic!("{err:?}")
        };
        assert_eq!(
            e.exec.calls(),
            vec!["supervisorctl start vnc"],
            "supervisord is tried without root"
        );
        let rec = e.store().get(change_id).unwrap().unwrap();
        assert_eq!(rec.status, ChangeStatus::Failed);
        assert!(rec.output.unwrap().contains("spawn error"));
        assert_eq!(err.body()["changeId"], json!(change_id));
        cleanup(&e);
    }

    #[test]
    fn permission_error_at_run_time_releases_the_approval() {
        let root = temp_root("docker-perm");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Docker,
            "web",
            "abc123",
            ServiceState::Running,
            None,
        ));
        f.reply(
            1,
            "",
            "permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock",
        );
        let e = engine(f);
        let err = e
            .run(&req("service.stop", "abc123", Some("ap-d")))
            .unwrap_err();
        let ActionError::NeedsElevation {
            change_id: Some(id),
            ..
        } = &err
        else {
            panic!("{err:?}")
        };
        assert_eq!(
            e.store().get(id).unwrap().unwrap().status,
            ChangeStatus::NeedsElevation
        );
        assert!(
            e.store().claim_approval("ap-d", "x").unwrap(),
            "approval can be used again"
        );
        cleanup(&e);
    }

    #[test]
    fn launchd_restart_is_stop_then_start() {
        let root = temp_root("launchd");
        let mut f = Fake::new(OsFamily::Macos, &root);
        f.elevated = false;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Launchd,
            "com.example.agent",
            "com.example.agent",
            ServiceState::Running,
            Some(77),
        ));
        let e = engine(f);
        e.run(&req("service.restart", "com.example.agent", Some("ap-l")))
            .unwrap();
        assert_eq!(
            e.exec.calls(),
            vec![
                "launchctl stop com.example.agent",
                "launchctl start com.example.agent"
            ]
        );
        cleanup(&e);
    }

    #[test]
    fn windows_service_uses_net_and_needs_admin() {
        let root = temp_root("scm");
        let f = Fake::new(OsFamily::Windows, &root);
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Scm,
            "Spooler",
            "Spooler",
            ServiceState::Running,
            Some(1500),
        ));
        let e = engine(f);
        e.run(&req("service.stop", "Spooler", Some("ap-w")))
            .unwrap();
        assert_eq!(e.exec.calls(), vec!["net stop Spooler"]);
        cleanup(&e);

        let root = temp_root("scm-noadmin");
        let mut f = Fake::new(OsFamily::Windows, &root);
        f.elevated = false;
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Scm,
            "Spooler",
            "Spooler",
            ServiceState::Running,
            Some(1500),
        ));
        let e = engine(f);
        assert_eq!(
            e.run(&req("service.stop", "Spooler", Some("ap-w")))
                .unwrap_err()
                .code(),
            "needs_elevation"
        );
        cleanup(&e);
    }

    #[test]
    fn port_held_by_a_service_stops_the_service_with_undo() {
        let root = temp_root("port-svc");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.services.lock().unwrap().push(svc(
            ServiceProvider::Systemd,
            "nginx",
            "nginx.service",
            ServiceState::Running,
            Some(900),
        ));
        f.ports = vec![
            port(80, Some(900), Some("nginx")),
            port(80, Some(900), Some("nginx")),
        ];
        let e = engine(f);
        let r = e.run(&req("port.free", "80", Some("ap-p"))).unwrap();
        assert_eq!(e.exec.calls(), vec!["systemctl stop nginx.service"]);
        let undo = r.undo.unwrap();
        assert_eq!(
            (undo.action_type, undo.target.as_str()),
            (ActionType::ServiceStart, "nginx.service")
        );
        cleanup(&e);
    }

    #[test]
    fn port_held_by_a_plain_process_is_killed_and_not_undoable() {
        let root = temp_root("port-kill");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.elevated = false;
        f.ports = vec![port(3000, Some(5150), Some("node"))];
        let e = engine(f);
        let r = e.run(&req("port.free", "3000", Some("ap-k"))).unwrap();
        assert_eq!(e.exec.calls(), vec!["kill -TERM 5150"]);
        assert!(r.undo.is_none());
        assert!(r.note.unwrap().contains("Not undoable"));
        let rec = e.store().get(&r.change_id).unwrap().unwrap();
        assert_eq!(
            rec.after.unwrap()["listening"],
            true,
            "after is read back (the fake does not change)"
        );
        cleanup(&e);

        let root = temp_root("port-kill-win");
        let mut f = Fake::new(OsFamily::Windows, &root);
        f.ports = vec![port(3000, Some(5150), Some("node.exe"))];
        let e = engine(f);
        e.run(&req("port.free", "3000", Some("ap-k"))).unwrap();
        assert_eq!(e.exec.calls(), vec!["taskkill /PID 5150 /T /F"]);
        cleanup(&e);
    }

    #[test]
    fn port_edge_cases() {
        let root = temp_root("port-edge");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.ports = vec![
            port(8013, Some(4242), Some("allternit-api")),
            port(22, None, None),
        ];
        f.elevated = false;
        let e = engine(f);
        assert_eq!(
            e.run(&req("port.free", "abc", Some("a1")))
                .unwrap_err()
                .http_status(),
            400
        );
        assert_eq!(
            e.run(&req("port.free", "0", Some("a2")))
                .unwrap_err()
                .http_status(),
            400
        );
        assert_eq!(
            e.run(&req("port.free", "9999", Some("a3")))
                .unwrap_err()
                .http_status(),
            404
        );
        assert_eq!(
            e.run(&req("port.free", "8013", Some("a4")))
                .unwrap_err()
                .code(),
            "refused"
        );
        assert_eq!(
            e.run(&req("port.free", "22", Some("a5")))
                .unwrap_err()
                .code(),
            "needs_elevation"
        );
        assert!(e.exec.calls().is_empty());
        cleanup(&e);
    }

    #[test]
    fn install_all_security_groups_by_source() {
        let root = temp_root("updates");
        let mut f = Fake::new(OsFamily::Linux, &root);
        f.updates = vec![
            update(UpdateSource::Apt, "openssl", true),
            update(UpdateSource::Apt, "vim", false),
            update(UpdateSource::Apt, "libssl3", true),
        ];
        let e = engine(f);
        let r = e
            .run(&req("updates.install", "all-security", Some("ap-u")))
            .unwrap();
        let calls = e.exec.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0]
            .starts_with("env DEBIAN_FRONTEND=noninteractive apt-get install -y --only-upgrade"));
        assert!(calls[0].ends_with(" openssl libssl3"), "{}", calls[0]);
        assert!(!calls[0].contains("vim"));
        assert!(r.undo.is_none());
        assert!(r.note.unwrap().starts_with("Not undoable"));
        cleanup(&e);
    }

    #[test]
    fn install_needs_root_and_a_known_id() {
        let root = temp_root("updates-elev");
        let mut f = Fake::new(OsFamily::Macos, &root);
        f.elevated = false;
        f.updates = vec![update(
            UpdateSource::Softwareupdate,
            "macOS Sequoia 15.2-24C101",
            true,
        )];
        let e = engine(f);
        assert_eq!(
            e.run(&req(
                "updates.install",
                "macOS Sequoia 15.2-24C101",
                Some("a1")
            ))
            .unwrap_err()
            .code(),
            "needs_elevation"
        );
        assert_eq!(
            e.run(&req("updates.install", "nope", Some("a2")))
                .unwrap_err()
                .http_status(),
            404
        );
        assert!(e.exec.calls().is_empty());
        cleanup(&e);
    }

    #[test]
    fn windows_updates_need_a_kb_number() {
        let root = temp_root("updates-win");
        let mut f = Fake::new(OsFamily::Windows, &root);
        f.updates = vec![
            update(UpdateSource::Windows, "KB5044284", true),
            update(UpdateSource::Windows, "Some driver", false),
        ];
        let e = engine(f);
        e.run(&req("updates.install", "KB5044284", Some("a1")))
            .unwrap();
        let calls = e.exec.calls();
        assert!(calls[0].starts_with("powershell -NoProfile -NonInteractive -Command"));
        assert!(calls[0].contains("$ids=@('KB5044284')"));
        assert_eq!(
            e.run(&req("updates.install", "Some driver", Some("a2")))
                .unwrap_err()
                .code(),
            "unsupported"
        );
        assert!(is_kb("KB123456") && !is_kb("KB12'3") && !is_kb("KB"));
        cleanup(&e);
    }

    #[test]
    fn clean_items_per_os() {
        let root = temp_root("clean");
        let f = Fake::new(OsFamily::Linux, &root);
        let e = engine(f);
        let r = e.run(&req("space.clean", "apt-cache", Some("a1"))).unwrap();
        assert_eq!(e.exec.calls(), vec!["apt-get clean"]);
        assert!(r.note.unwrap().contains("Not undoable"));
        assert_eq!(
            e.run(&req("space.clean", "brew-cleanup", Some("a2")))
                .unwrap_err()
                .code(),
            "unsupported"
        );
        assert_eq!(
            e.run(&req("space.clean", "rm-rf", Some("a3")))
                .unwrap_err()
                .http_status(),
            404
        );
        cleanup(&e);
        assert!(clean_items().iter().all(|i| safe_file_name(i.id)));
        let v = serde_json::to_value(&clean_items()[0]).unwrap();
        assert!(v.get("needsElevation").is_some() && v.get("program").is_none());
    }

    #[test]
    fn snippet_runs_from_the_request_with_no_saved_copy() {
        let root = temp_root("snippet-inline");
        let f = Fake::new(OsFamily::Linux, &root);
        f.reply(0, "ok\n", "");
        let e = engine(f);
        let sn = Snippet {
            id: "snp_1".into(),
            name: "Uptime".into(),
            script: "uptime".into(),
            elevated: false,
            timeout_sec: None,
        };
        let r = e
            .run(&ActionRequest {
                snippet: Some(sn.clone()),
                ..req("snippet.run", "snp_1", Some("a1"))
            })
            .unwrap();
        assert_eq!(e.exec.calls(), vec!["sh -c uptime"]);
        assert!(r.undo.is_none());
        // The body must be the snippet the approval named.
        let err = e
            .run(&ActionRequest {
                snippet: Some(sn),
                ..req("snippet.run", "snp_2", Some("a2"))
            })
            .unwrap_err();
        assert_eq!(err.http_status(), 400);
        cleanup(&e);
    }

    #[test]
    fn snippet_runs_from_the_store_and_output_is_masked() {
        let root = temp_root("snippet");
        let f = Fake::new(OsFamily::Linux, &root);
        f.reply(0, "connecting with password=hunter2\nok\n", "");
        let e = engine(f);
        let dir = e.store().snippets_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let sn = Snippet {
            id: "disk-report".into(),
            name: "Disk report".into(),
            script: "df -h; du -sh /var/log".into(),
            elevated: false,
            timeout_sec: Some(30),
        };
        std::fs::write(
            dir.join("disk-report.json"),
            serde_json::to_vec(&sn).unwrap(),
        )
        .unwrap();

        let r = e
            .run(&req("snippet.run", "disk-report", Some("a1")))
            .unwrap();
        assert_eq!(e.exec.calls(), vec!["sh -c df -h; du -sh /var/log"]);
        let rec = e.store().get(&r.change_id).unwrap().unwrap();
        let out = rec.output.unwrap();
        assert!(out.contains("ok") && !out.contains("hunter2"), "{out}");
        assert!(r.undo.is_none());

        assert_eq!(
            e.run(&req("snippet.run", "missing", Some("a2")))
                .unwrap_err()
                .http_status(),
            404
        );
        assert_eq!(
            e.run(&req("snippet.run", "../x", Some("a3")))
                .unwrap_err()
                .http_status(),
            400
        );
        cleanup(&e);
    }

    #[test]
    fn timeout_is_a_failure_that_says_it_may_have_partly_run() {
        let e = nginx_linux("timeout", ServiceState::Running);
        e.exec
            .replies
            .lock()
            .unwrap()
            .push_back(Err(RunError::Timeout));
        let err = e
            .run(&req("service.stop", "nginx.service", Some("a1")))
            .unwrap_err();
        assert!(
            matches!(err, ActionError::Failed { ref message, .. } if message.contains("partly run"))
        );
        cleanup(&e);
    }

    #[test]
    fn json_shapes_follow_the_contract() {
        let body = r#"{"type":"service.stop","target":"nginx.service","approval_id":"ap1"}"#;
        let r: ActionRequest = serde_json::from_str(body).unwrap();
        assert_eq!(r.approval_id.as_deref(), Some("ap1"));
        let r: ActionRequest =
            serde_json::from_str(r#"{"type":"port.free","target":"80","approvalId":"ap2"}"#)
                .unwrap();
        assert_eq!(r.approval_id.as_deref(), Some("ap2"));
        let r: ActionRequest = serde_json::from_str(r#"{"type":"x","target":"y"}"#).unwrap();
        assert!(
            r.approval_id.is_none(),
            "a missing approval is a 403 later, not a JSON error"
        );

        let res = ActionResult {
            change_id: "chg_1".into(),
            undo: Some(UndoAction {
                action_type: ActionType::ServiceStart,
                target: "nginx.service".into(),
                label: "Start nginx again".into(),
            }),
            note: None,
        };
        let v = serde_json::to_value(&res).unwrap();
        assert_eq!(
            v,
            json!({"changeId":"chg_1","undo":{"type":"service.start","target":"nginx.service","label":"Start nginx again"}})
        );
        for t in ActionType::ALL {
            assert_eq!(serde_json::to_value(t).unwrap(), json!(t.as_str()));
            assert_eq!(ActionType::parse(t.as_str()), Some(t));
        }
        assert!(new_change_id().starts_with("chg_") && safe_file_name(&new_change_id()));
    }
}
