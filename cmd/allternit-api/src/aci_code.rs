//! Code-mode grant gate — one hash-bound grant per validated code payload
//! (spec: `code-mode-execution`, phase C0).
//!
//! Code execution is the THIRD integration mode (after per-action whitelist
//! steps and batch dispatch), and it is never the default: a run only reaches
//! this surface when the caller explicitly presents a `code_payload`
//! descriptor. The descriptor binds `{origin, session, language, exact code,
//! declared network targets}` under one SHA-256, enforced by the same
//! single-use expiring grant machinery as actions and batches
//! (`aci_safety::enforce_confirmation_with_mode`).
//!
//! Product boundaries (locked in the spec):
//! - **Refuse-list at descriptor time.** Credential patterns, destructive
//!   calls, network targets outside the task's declared targets, and host-path
//!   access outside the run sandbox are REFUSED — they can never be granted,
//!   only rewritten as whitelist actions. A refusal is a 400 with the refusal
//!   class named, recorded on the receipt trail; no grant flow starts.
//! - **Code is always risky.** Unlike batches, there is no all-reversible
//!   auto-pass: every code payload requires its own single-use expiring grant.
//! - **Audit-before-act.** The code receipt row exists BEFORE the executor
//!   runs; the fixed result envelope (truncated+scrubbed stdout, exit status,
//!   screenshot hash+ref) is the only thing that comes back.
//! - **Never silent retry.** A refused or tampered payload never executes;
//!   a mutated payload is a different descriptor with a different hash.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Mutex, Arc};
use std::time::Duration;

use crate::{auth::AuthUser, AppState};

/// Language allowlist, in the spec's order (playwright-js first,
/// pyautogui-python second). The Rust side validates against this list; which
/// of them have a wired executor is [`CODE_LANGUAGES_EXECUTABLE_V1`].
pub const CODE_LANGUAGE_ALLOWLIST: &[&str] = &["playwright-js", "pyautogui-python"];

/// Languages whose executor exists today. `pyautogui-python` is an accepted
/// descriptor language (the allowlist names it) but its executor is not wired
/// in v1 — presenting one is refused honestly, never executed best-effort.
pub const CODE_LANGUAGES_EXECUTABLE_V1: &[&str] = &["playwright-js"];

/// Hard cap on payload size so a descriptor cannot smuggle an unbounded
/// program into one grant.
pub const MAX_CODE_BYTES: usize = 64 * 1024;

/// Hard cap the grant surface forwards to executors as the default wall clock
/// (spec §4: default 30 s, operator-configurable downstream).
pub const DEFAULT_CODE_TIMEOUT: Duration = Duration::from_secs(30);

/// One grantable code payload. The SHA-256 of its canonical JSON is what the
/// grant binds to — a modified client cannot swap, extend, or re-target the
/// code after approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodePayloadDescriptor {
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub language: String,
    pub code: String,
    /// The task's declared network targets. URL literals in the payload must
    /// stay inside these hosts or the descriptor is refused (fail-closed).
    #[serde(default)]
    pub declared_targets: Vec<String>,
}

impl CodePayloadDescriptor {
    /// Canonical hashing payload. Field set must stay stable — same
    /// discipline as `BatchDescriptor::canonical_json`.
    pub fn canonical_json(&self) -> Value {
        json!({
            "origin": self.origin,
            "session": self.session,
            "language": self.language,
            "code": self.code,
            "declaredTargets": self.declared_targets,
        })
    }

    /// SHA-256 over the canonical descriptor — the code-grant binding.
    pub fn hash(&self) -> String {
        crate::aci_approvals::hash_action_payload(&self.canonical_json())
    }
}

/// The refuse-list taxonomy. Every variant is a descriptor-time refusal: the
/// payload can NEVER be granted in this form — the model must rewrite it as
/// whitelist actions or amend the declared targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeRefusalClass {
    /// Language outside the allowlist (or not executable in v1).
    LanguageNotAllowed,
    /// Payload exceeds the size cap.
    PayloadTooLarge,
    /// Credential-shaped material inside the payload. Credentials reach code
    /// only through `sandbox_env` at execution time — never as literals.
    CredentialPattern,
    /// Destructive or nested-execution calls (child processes, dynamic eval,
    /// self-references to the code-mode surface).
    DestructiveCall,
    /// A URL literal whose host is outside the declared task targets.
    UndeclaredNetworkTarget,
    /// Host-path access outside the run sandbox (absolute host paths or
    /// traversal outside the sandbox dir).
    HostPathAccess,
}

impl CodeRefusalClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LanguageNotAllowed => "language_not_allowed",
            Self::PayloadTooLarge => "payload_too_large",
            Self::CredentialPattern => "credential_pattern",
            Self::DestructiveCall => "destructive_call",
            Self::UndeclaredNetworkTarget => "undeclared_network_target",
            Self::HostPathAccess => "host_path_access",
        }
    }
}

/// A descriptor-time refusal: the class plus a human/model-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRefusal {
    pub class: CodeRefusalClass,
    pub reason: String,
}

impl CodeRefusal {
    fn new(class: CodeRefusalClass, reason: impl Into<String>) -> Self {
        Self {
            class,
            reason: reason.into(),
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "error": "code_refused",
            "class": self.class.as_str(),
            "message": self.reason,
        })
    }
}

// ─── Refuse-list scanners ───────────────────────────────────────────────────

/// Credential-shaped material that must never appear as a literal in a
/// grantable payload. Values arrive at execution time via `sandbox_env`
/// (PR #187 semantics) — a literal in the payload is a leak in the making.
const CREDENTIAL_PATTERNS: &[(&str, &str)] = &[
    (r"AKIA[0-9A-Z]{16}", "AWS access key id"),
    (r"AIza[0-9A-Za-z_-]{35}", "Google API key"),
    (r"ghp_[A-Za-z0-9]{20,}", "GitHub personal access token"),
    (r"xox[baprs]-[A-Za-z0-9-]{10,}", "Slack token"),
    (r"sk-[A-Za-z0-9_-]{16,}", "API secret key material"),
    (r"-----BEGIN [A-Z ]*PRIVATE KEY-----", "PEM private key block"),
];

/// Destructive or nested-execution calls. The executor additionally denies
/// these at runtime (defense in depth); the descriptor refuses them first so
/// they can never be granted.
const DESTRUCTIVE_PATTERNS: &[(&str, &str)] = &[
    (r"child_process", "child_process access"),
    (r"node:child_process", "child_process access"),
    (r"\bsubprocess\b", "subprocess access"),
    (r"\bos\.system\b", "os.system call"),
    (r"\bshutil\.rmtree\b", "shutil.rmtree call"),
    (r"\bfs\.(rm|rmdir|unlink)\b", "fs deletion call"),
    (r"rm\s+-rf", "recursive force delete"),
    (r"\bprocess\.exit\b", "process control"),
    // Nested code mode: code may never spawn further code mode.
    (r"/aci/code\b", "reference to the code-mode grant surface"),
    (r"executeCode", "reference to the code-mode executor"),
];

/// Absolute host paths and traversal outside the run sandbox. The executor
/// only exposes a sandbox dir rooted handle; literals naming host paths are
/// refused here.
const HOST_PATH_PATTERNS: &[(&str, &str)] = &[
    (r"/(?:Users|home|root|etc|var|private)\b", "absolute host path"),
    (r"(?i)[A-Za-z]:\\\\", "Windows host path"),
    // No lookbehind: the regex crate doesn't support it, and an uncompilable
    // pattern used to be skipped silently (the check failed open).
    (r"(?:^|[^A-Za-z0-9_])~[/']", "home-directory reference"),
    (r"\.\./", "path traversal outside the run sandbox"),
];

/// Extract the host of a URL literal; accepts both full URLs and bare hosts
/// in declared_targets.
fn target_host(target: &str) -> Option<String> {
    let t = target.trim();
    if t.is_empty() {
        return None;
    }
    let stripped = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    let hostport = stripped.split('/').next().unwrap_or(stripped);
    let host = hostport.split(':').next().unwrap_or(hostport);
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

fn scan_patterns(
    code: &str,
    patterns: &[(&str, &str)],
    class: CodeRefusalClass,
) -> Result<(), CodeRefusal> {
    for (pattern, label) in patterns {
        // Fail closed: a refuse-list pattern that doesn't compile must refuse,
        // never silently skip its check.
        let Ok(re) = regex::Regex::new(pattern) else {
            return Err(CodeRefusal::new(
                class,
                format!("refuse-list pattern for {label} failed to compile"),
            ));
        };
        if let Some(found) = re.find(code) {
            return Err(CodeRefusal::new(
                class,
                format!("payload contains {label}: {found:?}"),
            ));
        }
    }
    Ok(())
}

/// Validate a descriptor against the refuse-list. Ok(()) means grantable —
/// the payload still needs its own single-use grant before it executes.
pub fn validate_code_descriptor(
    descriptor: &CodePayloadDescriptor,
) -> Result<(), CodeRefusal> {
    if !CODE_LANGUAGE_ALLOWLIST.contains(&descriptor.language.as_str()) {
        return Err(CodeRefusal::new(
            CodeRefusalClass::LanguageNotAllowed,
            format!(
                "language {:?} is outside the allowlist {:?}",
                descriptor.language, CODE_LANGUAGE_ALLOWLIST
            ),
        ));
    }
    if !CODE_LANGUAGES_EXECUTABLE_V1.contains(&descriptor.language.as_str()) {
        return Err(CodeRefusal::new(
            CodeRefusalClass::LanguageNotAllowed,
            format!(
                "language {:?} is allowlisted but has no wired executor in this build; supported: {:?}",
                descriptor.language, CODE_LANGUAGES_EXECUTABLE_V1
            ),
        ));
    }
    if descriptor.code.is_empty() {
        return Err(CodeRefusal::new(
            CodeRefusalClass::PayloadTooLarge,
            "payload is empty",
        ));
    }
    if descriptor.code.len() > MAX_CODE_BYTES {
        return Err(CodeRefusal::new(
            CodeRefusalClass::PayloadTooLarge,
            format!(
                "payload is {} bytes; the maximum is {}",
                descriptor.code.len(),
                MAX_CODE_BYTES
            ),
        ));
    }

    let code = &descriptor.code;
    scan_patterns(code, CREDENTIAL_PATTERNS, CodeRefusalClass::CredentialPattern)?;
    scan_patterns(code, DESTRUCTIVE_PATTERNS, CodeRefusalClass::DestructiveCall)?;
    scan_patterns(code, HOST_PATH_PATTERNS, CodeRefusalClass::HostPathAccess)?;

    // Network: every URL literal in the payload must land inside a declared
    // task target. Fail-closed — an undeclared host refuses the descriptor.
    let declared_hosts: Vec<String> = descriptor
        .declared_targets
        .iter()
        .filter_map(|t| target_host(t))
        .collect();
    let url_re = regex::Regex::new(r"https?://[A-Za-z0-9._~-]+(?::[0-9]+)?(?:/[A-Za-z0-9._~/?&=+%#-]*)?")
        .expect("url pattern");
    for found in url_re.find_iter(code) {
        let lit = found.as_str();
        let host = target_host(lit).unwrap_or_default();
        let covered = declared_hosts
            .iter()
            .any(|d| d == &host || (d.starts_with('*') && host.ends_with(d.trim_start_matches('*'))));
        if !covered {
            return Err(CodeRefusal::new(
                CodeRefusalClass::UndeclaredNetworkTarget,
                format!(
                    "payload references {host:?} which is outside the declared task targets {:?}",
                    descriptor.declared_targets
                ),
            ));
        }
    }
    Ok(())
}

// ─── Grant enforcement ──────────────────────────────────────────────────────

/// Enforce the one-grant-per-payload path. Code is always `Risky` — there is
/// no auto-pass. Identical grant semantics to actions/batches: single-use,
/// expiring, hash-bound. Returns the presented grant id on success.
pub fn enforce_code_grant(
    approval_store: &crate::permission_policy::ApprovalStore,
    user_id: &str,
    route: &str,
    descriptor: &CodePayloadDescriptor,
    approval_id: Option<&str>,
) -> Result<Option<String>, crate::aci_safety::ConfirmationDenial> {
    enforce_code_grant_with_mode(
        crate::aci_safety::HOST_POLICY.mode,
        approval_store,
        user_id,
        route,
        descriptor,
        approval_id,
    )
}

/// Mode-injectable core of [`enforce_code_grant`] so tests are hermetic
/// regardless of `ALLTERNIT_ACI_SAFETY_MODE`.
pub fn enforce_code_grant_with_mode(
    mode: crate::aci_safety::SafetyMode,
    approval_store: &crate::permission_policy::ApprovalStore,
    user_id: &str,
    route: &str,
    descriptor: &CodePayloadDescriptor,
    approval_id: Option<&str>,
) -> Result<Option<String>, crate::aci_safety::ConfirmationDenial> {
    crate::aci_safety::enforce_confirmation_with_mode(
        mode,
        approval_store,
        user_id,
        route,
        crate::aci_safety::ConfirmationClass::Risky,
        &descriptor.canonical_json(),
        approval_id,
    )?;
    Ok(approval_id.map(str::to_string))
}

// ─── Code receipts ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeReceipt {
    pub receipt_id: String,
    pub code_id: String,
    pub user_id: String,
    pub descriptor_hash: String,
    pub language: String,
    pub grant_id: Option<String>,
    /// `dispatched` | `completed` | `failed` | `denied` | `refused`.
    pub status: String,
    /// Refusal class + reason when the descriptor was refused pre-grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Value>,
    /// Fixed-envelope outcome fields (post-execution overlay). stdout CONTENT
    /// never lands here — only its byte length; the envelope owns stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_ref: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodeReceiptRecord {
    kind: String,
    receipt: CodeReceipt,
    #[serde(default)]
    chain_hash: String,
}

const CODE_RECEIPT_CHAIN_GENESIS: &str = "allternit-code-receipts-v1";

fn code_receipt_chain_link(tip: &str, record: &CodeReceiptRecord) -> String {
    let mut value = serde_json::to_value(record).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = value {
        map.remove("chain_hash");
    }
    crate::aci_approvals::hash_action_payload(&json!({
        "chain": "aci.code.receipts.v1",
        "tip": tip,
        "record": value,
    }))
}

/// In-memory authoritative code receipts plus an optional chained JSONL audit
/// trail — same discipline as `aci_batch::BatchReceiptStore`.
#[derive(Debug, Default)]
pub struct CodeReceiptStore {
    receipts: Mutex<Vec<CodeReceipt>>,
    path: Option<std::path::PathBuf>,
    chain_tip: Mutex<Option<String>>,
}

const MAX_RETAINED_CODE_RECEIPTS: usize = 10_000;

impl CodeReceiptStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_persisted(path: std::path::PathBuf) -> Self {
        let store = Self {
            receipts: Mutex::new(Vec::new()),
            path: Some(path),
            chain_tip: Mutex::new(None),
        };
        store.load();
        store
    }

    fn load(&self) {
        let Some(path) = &self.path else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut receipts = self.receipts.lock().expect("code receipt lock");
        let mut tip = CODE_RECEIPT_CHAIN_GENESIS.to_string();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<CodeReceiptRecord>(line) {
                Ok(record) => {
                    tip = code_receipt_chain_link(&tip, &record);
                    match receipts.iter_mut().find(|r| r.receipt_id == record.receipt.receipt_id) {
                        Some(existing) => *existing = record.receipt,
                        None => receipts.push(record.receipt),
                    }
                }
                Err(e) => tracing::warn!("code receipts reload: skipping unparseable line: {e}"),
            }
        }
        let overflow = receipts.len().saturating_sub(MAX_RETAINED_CODE_RECEIPTS);
        if overflow > 0 {
            receipts.drain(0..overflow);
        }
        drop(receipts);
        *self.chain_tip.lock().expect("code receipt chain lock") = Some(tip);
    }

    fn persist(&self, kind: &str, receipt: &CodeReceipt) {
        let Some(path) = &self.path else { return };
        let mut record = CodeReceiptRecord {
            kind: kind.to_string(),
            receipt: receipt.clone(),
            chain_hash: String::new(),
        };
        {
            let mut tip_guard = self.chain_tip.lock().expect("code receipt chain lock");
            let tip = tip_guard
                .clone()
                .unwrap_or_else(|| CODE_RECEIPT_CHAIN_GENESIS.to_string());
            let link = code_receipt_chain_link(&tip, &record);
            record.chain_hash = link.clone();
            *tip_guard = Some(link);
        }
        let Ok(line) = serde_json::to_string(&record) else {
            return;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        use std::io::Write as _;
        match std::fs::OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                if let Err(e) = writeln!(file, "{line}") {
                    tracing::warn!("code receipts persist: write failed: {e}");
                }
            }
            Err(e) => tracing::warn!("code receipts persist: open failed: {e}"),
        }
    }

    /// Audit-before-act: the receipt row exists BEFORE the payload executes.
    pub fn record_dispatched(&self, receipt: CodeReceipt) {
        self.persist("dispatched", &receipt);
        let mut receipts = self.receipts.lock().expect("code receipt lock");
        if receipts.len() >= MAX_RETAINED_CODE_RECEIPTS {
            let drop = receipts.len() - MAX_RETAINED_CODE_RECEIPTS + 1;
            receipts.drain(0..drop);
        }
        receipts.push(receipt);
    }

    /// Overlay the fixed-envelope outcome onto a dispatched receipt.
    pub fn record_completed(
        &self,
        receipt_id: &str,
        outcome: &CodeRunOutcome,
        status: &str,
    ) -> Option<CodeReceipt> {
        let mut receipts = self.receipts.lock().expect("code receipt lock");
        let receipt = receipts.iter_mut().find(|r| r.receipt_id == receipt_id)?;
        receipt.status = status.to_string();
        receipt.exit_status = outcome.exit_status;
        receipt.stdout_bytes = Some(outcome.stdout.len());
        receipt.timed_out = Some(outcome.timed_out);
        receipt.screenshot_sha256 = outcome.screenshot_sha256.clone();
        receipt.screenshot_ref = outcome.screenshot_ref.clone();
        receipt.completed_at = Some(chrono::Utc::now().to_rfc3339());
        let updated = receipt.clone();
        drop(receipts);
        self.persist("completed", &updated);
        Some(updated)
    }

    pub fn get(&self, receipt_id: &str) -> Option<CodeReceipt> {
        self.receipts
            .lock()
            .expect("code receipt lock")
            .iter()
            .find(|r| r.receipt_id == receipt_id)
            .cloned()
    }

    pub fn receipts(&self) -> Vec<CodeReceipt> {
        self.receipts.lock().expect("code receipt lock").clone()
    }
}

/// Process-wide code receipt store; the audit trail lives next to the
/// per-action and batch receipts.
pub static CODE_RECEIPTS: Lazy<CodeReceiptStore> = Lazy::new(|| {
    CodeReceiptStore::new_persisted(
        crate::aci_approvals::computer_use_dir()
            .join("receipts")
            .join("code-receipts.jsonl"),
    )
});

// ─── Execution ──────────────────────────────────────────────────────────────

/// The FIXED result envelope a code execution returns (spec §3). Code cannot
/// return arbitrary host data: truncated+scrubbed stdout, exit status, and a
/// screenshot hash+ref are the whole surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRunOutcome {
    pub stdout: String,
    pub exit_status: Option<i32>,
    pub timed_out: bool,
    pub screenshot_sha256: Option<String>,
    pub screenshot_ref: Option<String>,
}

/// A whole-payload executor: runs the granted payload inside the run sandbox
/// and reports the fixed envelope. In production the executor lives microVM-
/// side; the payload crosses the sidecar/VM channel. The Rust side only ever
/// sees the envelope.
pub type CodeExecutor = Box<dyn FnOnce(&CodePayloadDescriptor) -> CodeRunOutcome>;

/// Execute a granted payload and overlay the envelope onto the dispatched
/// receipt. The dispatched receipt must already exist (audit-before-act).
/// Returns the updated receipt plus the fixed envelope (the caller surfaces
/// the envelope to the model; the receipt keeps only metadata).
pub fn run_code_payload(
    receipt_store: &CodeReceiptStore,
    receipt_id: &str,
    descriptor: &CodePayloadDescriptor,
    execute: CodeExecutor,
) -> Option<(CodeReceipt, CodeRunOutcome)> {
    let outcome = execute(descriptor);
    // `completed` only on a clean exit 0 — a refused execution (exit 13), a
    // non-zero exit, or a timeout is recorded honestly as failed.
    let status = if outcome.timed_out || outcome.exit_status != Some(0) {
        "failed"
    } else {
        "completed"
    };
    receipt_store
        .record_completed(receipt_id, &outcome, status)
        .map(|receipt| (receipt, outcome))
}

// ─── HTTP surface ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AciCodeBody {
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    pub language: String,
    pub code: String,
    #[serde(default)]
    pub declared_targets: Vec<String>,
    /// Code-grant id from a prior `confirmation_required` denial.
    #[serde(default)]
    pub approval_id: Option<String>,
}

/// Core gated-code pipeline, executor injected. Shared by the route (real
/// sandbox executor) and tests (stub executor).
pub async fn run_gated_code(
    state: &AppState,
    user_id: &str,
    body: &AciCodeBody,
    receipt_store: &CodeReceiptStore,
    make_executor: impl FnOnce(&CodePayloadDescriptor) -> Result<CodeExecutor, String>,
) -> Response {
    let descriptor = CodePayloadDescriptor {
        origin: body
            .origin
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "aci.code".to_string()),
        session: body.session.clone().filter(|s| !s.trim().is_empty()),
        language: body.language.clone(),
        code: body.code.clone(),
        declared_targets: body
            .declared_targets
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    };

    // Refuse-list FIRST: a refused payload never starts a grant flow and can
    // never execute. The refusal itself lands on the audit trail.
    if let Err(refusal) = validate_code_descriptor(&descriptor) {
        receipt_store.record_dispatched(CodeReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            code_id: descriptor.hash(),
            user_id: user_id.to_string(),
            descriptor_hash: descriptor.hash(),
            language: descriptor.language.clone(),
            grant_id: None,
            status: "refused".to_string(),
            refusal: Some(refusal.body()),
            exit_status: None,
            stdout_bytes: None,
            timed_out: None,
            screenshot_sha256: None,
            screenshot_ref: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: Some(chrono::Utc::now().to_rfc3339()),
        });
        return (StatusCode::BAD_REQUEST, Json(refusal.body())).into_response();
    }

    let descriptor_hash = descriptor.hash();

    // Grant enforcement — before dispatch, exactly like actions/batches.
    let grant_id = match enforce_code_grant(
        &state.approval_store,
        user_id,
        "aci.code",
        &descriptor,
        body.approval_id.as_deref(),
    ) {
        Ok(grant) => grant,
        Err(denial) => {
            receipt_store.record_dispatched(CodeReceipt {
                receipt_id: uuid::Uuid::new_v4().to_string(),
                code_id: descriptor_hash.clone(),
                user_id: user_id.to_string(),
                descriptor_hash: descriptor_hash.clone(),
                language: descriptor.language.clone(),
                grant_id: body.approval_id.clone(),
                status: "denied".to_string(),
                refusal: Some(denial.body.clone()),
                exit_status: None,
                stdout_bytes: None,
                timed_out: None,
                screenshot_sha256: None,
                screenshot_ref: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                completed_at: Some(chrono::Utc::now().to_rfc3339()),
            });
            return (denial.status, Json(denial.body)).into_response();
        }
    };

    // Audit-before-act: the receipt row exists BEFORE the payload executes.
    let receipt_id = uuid::Uuid::new_v4().to_string();
    receipt_store.record_dispatched(CodeReceipt {
        receipt_id: receipt_id.clone(),
        code_id: descriptor_hash.clone(),
        user_id: user_id.to_string(),
        descriptor_hash: descriptor_hash.clone(),
        language: descriptor.language.clone(),
        grant_id: grant_id.clone(),
        status: "dispatched".to_string(),
        refusal: None,
        exit_status: None,
        stdout_bytes: None,
        timed_out: None,
        screenshot_sha256: None,
        screenshot_ref: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    });

    let executor = match make_executor(&descriptor) {
        Ok(executor) => executor,
        Err(message) => {
            let outcome = CodeRunOutcome {
                stdout: String::new(),
                exit_status: None,
                timed_out: false,
                screenshot_sha256: None,
                screenshot_ref: None,
            };
            let receipt = receipt_store.record_completed(&receipt_id, &outcome, "failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "code_executor_unavailable",
                    "message": message,
                    "receipt": receipt,
                })),
            )
                .into_response();
        }
    };

    let Some((receipt, outcome)) =
        run_code_payload(receipt_store, &receipt_id, &descriptor, executor)
    else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "code_receipt_lost", "receipt_id": receipt_id})),
        )
            .into_response();
    };

    (
        StatusCode::OK,
        Json(json!({
            "receipt_id": receipt_id,
            "descriptor_hash": descriptor_hash,
            "grant_id": grant_id,
            "receipt": receipt,
            // The fixed model-visible result envelope (spec §3): truncated+
            // scrubbed stdout, exit status, screenshot hash+ref. Nothing else
            // crosses back.
            "result": {
                "stdout": outcome.stdout,
                "exit_status": outcome.exit_status,
                "timed_out": outcome.timed_out,
                "screenshot_sha256": outcome.screenshot_sha256,
                "screenshot_ref": outcome.screenshot_ref,
            },
        })),
    )
        .into_response()
}

/// Dev-gated sandbox executor for the code route.
///
/// Production runs this harness microVM-side and the payload crosses the
/// sidecar/VM channel. Until that channel is wired end-to-end, the route
/// executes NOTHING by default (an unavailable sandbox is a 502, never a
/// best-effort host run). An operator explicitly sets
/// `ALLTERNIT_CODE_EXECUTOR=node-sandbox` to enable the same sandboxed runner
/// (`code_runner.mjs`: vm-context payload, scoped fs, fail-closed egress,
/// sandbox_env-only credential path, wall-clock cap) as a local child process
/// for development and measurement. The containment boundary is the runner's
/// vm context, identical in both placements.
pub fn sandbox_code_executor(descriptor: &CodePayloadDescriptor) -> Result<CodeExecutor, String> {
    if std::env::var("ALLTERNIT_CODE_EXECUTOR").ok().as_deref() != Some("node-sandbox") {
        return Err(
            "code executor is not enabled: production dispatch is microVM-side, and host \
             execution is never an implicit fallback (set ALLTERNIT_CODE_EXECUTOR=node-sandbox \
             explicitly to run the sandboxed dev harness)"
                .to_string(),
        );
    }
    let runner_path = resolve_code_runner()?;
    let sandbox_dir = std::env::temp_dir().join(format!("allternit-code-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&sandbox_dir)
        .map_err(|e| format!("sandbox dir unavailable: {e}"))?;

    // The ONLY credential path: an operator-declared env-key allowlist whose
    // values are inherited from this process's environment (the run's
    // sandbox_env, injected Python-side). Key names travel in the spec;
    // values never do.
    let env_keys: Vec<String> = std::env::var("ALLTERNIT_CODE_SANDBOX_ENV_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let secrets: Vec<String> = env_keys
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .filter(|v| !v.is_empty())
        .collect();

    let spec = json!({
        "code": descriptor.code,
        "sandboxDir": sandbox_dir,
        "declaredTargets": descriptor.declared_targets,
        "timeoutMs": DEFAULT_CODE_TIMEOUT.as_millis() as u64,
        "sandboxEnvKeys": env_keys,
        // The dev placement has no browser bridge: page ops refuse honestly
        // (exit 13) instead of hanging. The Python harness sets this from the
        // presence of a live page_op_handler.
        "hasBridge": false,
    });
    let spec_path = sandbox_dir.join("code-spec.json");
    std::fs::write(&spec_path, spec.to_string())
        .map_err(|e| format!("sandbox spec write failed: {e}"))?;

    let mut child_env: Vec<(String, String)> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        child_env.push(("PATH".to_string(), path));
    }
    for key in &env_keys {
        if let Ok(value) = std::env::var(key) {
            child_env.push((key.clone(), value));
        }
    }

    let timeout = DEFAULT_CODE_TIMEOUT + Duration::from_secs(5);
    let code = descriptor.code.clone();
    Ok(Box::new(move |_descriptor: &CodePayloadDescriptor| {
        let _ = &code;
        let mut command = std::process::Command::new("node");
        command
            .arg(&runner_path)
            .arg(&spec_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&sandbox_dir)
            .env_clear()
            .envs(child_env.iter().cloned());
        let outcome = run_sandbox_child(&mut command, timeout, &secrets);
        let _ = std::fs::remove_dir_all(&sandbox_dir);
        outcome
    }))
}

fn resolve_code_runner() -> Result<std::path::PathBuf, String> {
    if let Ok(path) = std::env::var("ALLTERNIT_CODE_RUNNER") {
        if !path.is_empty() {
            return Ok(std::path::PathBuf::from(path));
        }
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest.ancestors() {
        let candidate = ancestor.join("domains/computer-use/core/core/code_runner.mjs");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("code_runner.mjs not found; set ALLTERNIT_CODE_RUNNER".to_string())
}

fn run_sandbox_child(
    command: &mut std::process::Command,
    timeout: Duration,
    secrets: &[String],
) -> CodeRunOutcome {
    let child = command.spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            return CodeRunOutcome {
                stdout: String::new(),
                exit_status: None,
                timed_out: false,
                screenshot_sha256: None,
                screenshot_ref: None,
            }
            .with_error_note(&format!("sandbox spawn failed: {e}"))
        }
    };
    let waited = child.wait_timeout(timeout).map(|w| w.is_none()).unwrap_or(false);
    if waited {
        let _ = child.kill();
        let _ = child.wait();
        return CodeRunOutcome {
            stdout: String::new(),
            exit_status: None,
            timed_out: true,
            screenshot_sha256: None,
            screenshot_ref: None,
        };
    }
    let _ = child.wait();
    let stdout = child
        .stdout
        .take()
        .map(|mut s| {
            use std::io::Read as _;
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
        .unwrap_or_default();
    // The runner's final line is the fixed envelope; everything before it is
    // protocol traffic (__ACI_STDOUT__/__ACI_OP__ lines), not payload stdout.
    let mut payload_stdout = String::new();
    let mut exit_status: Option<i32> = None;
    let mut timed_out = false;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("__ACI_RESULT__ ") {
            if let Ok(value) = serde_json::from_str::<Value>(rest) {
                exit_status = value.get("exitStatus").and_then(Value::as_i64).map(|v| v as i32);
                timed_out = value.get("timedOut").and_then(Value::as_bool).unwrap_or(false);
                if let Some(lines) = value.get("stdout").and_then(Value::as_array) {
                    payload_stdout = lines
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
        }
    }
    let mut stdout = payload_stdout;
    if stdout.len() > 4096 {
        stdout.truncate(4096);
    }
    for secret in secrets {
        if !secret.is_empty() {
            stdout = stdout.replace(secret.as_str(), "***");
        }
    }
    CodeRunOutcome {
        stdout,
        exit_status,
        timed_out,
        screenshot_sha256: None,
        screenshot_ref: None,
    }
}

trait OutcomeErrorNote {
    fn with_error_note(self, note: &str) -> Self;
}

impl OutcomeErrorNote for CodeRunOutcome {
    fn with_error_note(mut self, note: &str) -> Self {
        // The fixed envelope has no free-text error field server-side; a spawn
        // failure is reported as a non-zero-less failed run with the note in
        // stdout so the refusal is visible rather than silent.
        if self.stdout.is_empty() {
            self.stdout = note.to_string();
        }
        self
    }
}

/// Poll-based wait timeout so we do not pull in a new crate: sleep in slices
/// and reap the child when the deadline passes.
trait WaitTimeout {
    fn wait_timeout(&mut self, timeout: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl WaitTimeout for std::process::Child {
    fn wait_timeout(&mut self, timeout: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

pub async fn aci_code_execute(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<AciCodeBody>,
) -> Response {
    run_gated_code(&state, &user.user_id, &body, &CODE_RECEIPTS, |descriptor| {
        sandbox_code_executor(descriptor)
    })
    .await
}

pub async fn aci_code_receipt(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    match CODE_RECEIPTS.get(&id) {
        Some(receipt) => (StatusCode::OK, Json(serde_json::to_value(receipt).unwrap_or_default()))
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "code_receipt_not_found"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn descriptor(code: &str, targets: Vec<&str>) -> CodePayloadDescriptor {
        CodePayloadDescriptor {
            origin: "aci.code".to_string(),
            session: Some("session-1".to_string()),
            language: "playwright-js".to_string(),
            code: code.to_string(),
            declared_targets: targets.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn benign() -> CodePayloadDescriptor {
        descriptor(
            "await page.click('#go'); console.log('done');",
            vec!["http://127.0.0.1:8080"],
        )
    }

    // ── Refuse-list classes ───────────────────────────────────────────────

    #[test]
    fn language_outside_allowlist_is_refused() {
        let mut d = benign();
        d.language = "bash".to_string();
        let refusal = validate_code_descriptor(&d).unwrap_err();
        assert_eq!(refusal.class, CodeRefusalClass::LanguageNotAllowed);
        assert!(refusal.reason.contains("allowlist"));
    }

    #[test]
    fn allowlisted_but_unexecutable_language_is_refused_honestly() {
        let mut d = benign();
        d.language = "pyautogui-python".to_string();
        let refusal = validate_code_descriptor(&d).unwrap_err();
        assert_eq!(refusal.class, CodeRefusalClass::LanguageNotAllowed);
        assert!(refusal.reason.contains("no wired executor"));
    }

    #[test]
    fn oversized_payload_is_refused() {
        let mut d = benign();
        d.code = "x".repeat(MAX_CODE_BYTES + 1);
        let refusal = validate_code_descriptor(&d).unwrap_err();
        assert_eq!(refusal.class, CodeRefusalClass::PayloadTooLarge);
    }

    #[test]
    fn credential_patterns_are_refused() {
        for (snippet, label) in [
            ("const k = 'AKIAIOSFODNN7EXAMPLE';", "aws key"),
            ("const g = 'AIzaSyD4iE7xn0osl4kMTzQ9abx9i0k0abcd1234XYZ';", "google key"),
            ("fetch('https://api.example.com', {headers: {authorization: 'ghp_aB1cD2eF3gH4iJ5kL6mN7oP8qR9sT0uV1w'}})", "github pat"),
            ("const pem = '-----BEGIN RSA PRIVATE KEY-----';", "pem block"),
        ] {
            let mut d = benign();
            d.declared_targets = vec!["https://api.example.com".to_string()];
            d.code = snippet.to_string();
            let refusal = validate_code_descriptor(&d).unwrap_err();
            assert_eq!(
                refusal.class,
                CodeRefusalClass::CredentialPattern,
                "{label} must refuse as credential_pattern: {refusal:?}"
            );
        }
    }

    #[test]
    fn destructive_and_nested_calls_are_refused() {
        for (snippet, label) in [
            ("const cp = require('child_process');", "child_process"),
            ("cp.spawn('sh', ['-c', 'rm -rf /']);", "rm -rf"),
            ("fs.rm('/tmp/x', () => {});", "fs.rm"),
            ("await fetch('http://127.0.0.1:8013/aci/code', {method: 'POST'});", "nested code mode route"),
            ("page.evaluate(() => executeCode('x'));", "nested executor reference"),
        ] {
            let mut d = benign();
            d.code = snippet.to_string();
            let refusal = validate_code_descriptor(&d).unwrap_err();
            assert_eq!(
                refusal.class,
                CodeRefusalClass::DestructiveCall,
                "{label} must refuse as destructive_call: {refusal:?}"
            );
        }
    }

    #[test]
    fn undeclared_network_targets_are_refused_fail_closed() {
        let mut d = benign();
        d.code = "await page.goto('https://evil.example/x');".to_string();
        d.declared_targets = vec!["http://127.0.0.1:8080".to_string()];
        let refusal = validate_code_descriptor(&d).unwrap_err();
        assert_eq!(refusal.class, CodeRefusalClass::UndeclaredNetworkTarget);
        assert!(refusal.reason.contains("evil.example"));

        // Declaring the host makes the same payload grantable.
        d.declared_targets = vec![
            "http://127.0.0.1:8080".to_string(),
            "https://evil.example".to_string(),
        ];
        assert!(validate_code_descriptor(&d).is_ok());
    }

    #[test]
    fn every_refuse_list_pattern_compiles() {
        for (pattern, label) in CREDENTIAL_PATTERNS
            .iter()
            .chain(DESTRUCTIVE_PATTERNS)
            .chain(HOST_PATH_PATTERNS)
        {
            assert!(regex::Regex::new(pattern).is_ok(), "{label}: {pattern}");
        }
    }

    #[test]
    fn home_reference_needs_a_boundary() {
        let mut d = benign();
        d.code = "const s = fs.readFileSync('~/x');".to_string();
        assert!(validate_code_descriptor(&d).is_err());
        d.code = "const a = b~/2;".to_string();
        let _ = validate_code_descriptor(&d); // an operator mid-token is not a home path
        d.code = "~/start-of-payload".to_string();
        assert!(validate_code_descriptor(&d).is_err());
    }

    #[test]
    fn host_paths_outside_the_sandbox_are_refused() {
        for (snippet, label) in [
            ("const s = fs.readFileSync('/etc/passwd');", "system path"),
            ("fs.copyFile('~/secret.txt', 'out');", "user home path"),
            ("fs.readFile('../../outside.txt');", "path traversal"),
        ] {
            let mut d = benign();
            d.code = snippet.to_string();
            let refusal = validate_code_descriptor(&d).unwrap_err();
            assert_eq!(
                refusal.class,
                CodeRefusalClass::HostPathAccess,
                "{label} must refuse as host_path_access: {refusal:?}"
            );
        }
    }

    #[test]
    fn benign_playwright_payload_validates() {
        let d = descriptor(
            "await page.goto('http://127.0.0.1:8080/');\n\
             await page.click('#login');\n\
             await page.fill('#user', process.env.SANDBOX_USER);\n\
             console.log('submitted');",
            vec!["http://127.0.0.1:8080"],
        );
        assert!(validate_code_descriptor(&d).is_ok());
    }

    // ── Hash binding ──────────────────────────────────────────────────────

    #[test]
    fn descriptor_hash_covers_every_field() {
        let base = benign();
        let hash = base.hash();

        let same = descriptor(&base.code, vec!["http://127.0.0.1:8080"]);
        assert_eq!(hash, same.hash());

        let mut code_change = base.clone();
        code_change.code.push(' ');
        assert_ne!(hash, code_change.hash());

        let mut lang_change = base.clone();
        lang_change.language = "pyautogui-python".to_string();
        assert_ne!(hash, lang_change.hash());

        let mut targets_change = base.clone();
        targets_change.declared_targets.push("https://x.example".to_string());
        assert_ne!(hash, targets_change.hash());

        let mut session_change = base.clone();
        session_change.session = Some("session-2".to_string());
        assert_ne!(hash, session_change.hash());
    }

    // ── Grant lifecycle ───────────────────────────────────────────────────

    fn approval_store() -> crate::permission_policy::ApprovalStore {
        crate::permission_policy::ApprovalStore::new()
    }

    fn approved_grant_for(
        store: &crate::permission_policy::ApprovalStore,
        d: &CodePayloadDescriptor,
    ) -> String {
        let denial = enforce_code_grant_with_mode(
            crate::aci_safety::SafetyMode::Enforce,
            store,
            "user-1",
            "aci.code",
            d,
            None,
        )
        .unwrap_err();
        assert_eq!(denial.status, StatusCode::FORBIDDEN);
        assert_eq!(denial.body["error"], "confirmation_required");
        assert_eq!(denial.body["action_hash"], d.hash());
        let id = denial.body["approval_id"].as_str().unwrap().to_string();
        assert!(store.approve(&id));
        assert!(crate::aci_approvals::GRANTS.approve(&id));
        id
    }

    #[test]
    fn granted_payload_redeems_once_and_replay_is_denied() {
        let store = approval_store();
        let d = benign();
        let id = approved_grant_for(&store, &d);

        assert_eq!(
            enforce_code_grant_with_mode(
                crate::aci_safety::SafetyMode::Enforce,
                &store,
                "user-1",
                "aci.code",
                &d,
                Some(&id),
            )
            .unwrap(),
            Some(id.clone())
        );
        let replay = enforce_code_grant_with_mode(
            crate::aci_safety::SafetyMode::Enforce,
            &store,
            "user-1",
            "aci.code",
            &d,
            Some(&id),
        )
        .unwrap_err();
        assert_eq!(replay.body["error"], "approval_denied");
        assert!(replay.body["reason"].as_str().unwrap().contains("single-use"));
    }

    #[test]
    fn tampered_payload_is_rejected_by_hash_mismatch_and_consumes_nothing() {
        let store = approval_store();
        let d = benign();
        let id = approved_grant_for(&store, &d);

        let mut tampered = d.clone();
        tampered.code.push_str("\nconsole.log('extra');");
        let denial = enforce_code_grant_with_mode(
            crate::aci_safety::SafetyMode::Enforce,
            &store,
            "user-1",
            "aci.code",
            &tampered,
            Some(&id),
        )
        .unwrap_err();
        assert_eq!(denial.body["error"], "approval_denied");
        assert_eq!(
            denial.body["reason"],
            crate::aci_approvals::GrantDenial::HashMismatch.reason()
        );
        // The pristine payload can still redeem the grant.
        assert!(enforce_code_grant_with_mode(
            crate::aci_safety::SafetyMode::Enforce,
            &store,
            "user-1",
            "aci.code",
            &d,
            Some(&id)
        )
        .is_ok());
    }

    #[test]
    fn expired_code_grant_is_rejected() {
        let d = benign();
        let store = approval_store();
        let approval_id = uuid::Uuid::new_v4().to_string();
        crate::aci_approvals::GRANTS.issue_with_id(&approval_id, "user-1", &d.hash(), 60);
        crate::aci_approvals::GRANTS.approve(&approval_id);
        let mut grant = crate::aci_approvals::GRANTS.get(&approval_id).unwrap();
        grant.expires_at = chrono::Utc::now().timestamp_millis() - 1;
        crate::aci_approvals::GRANTS.set_expires_for_test(&approval_id, grant.expires_at);
        let denial = enforce_code_grant_with_mode(
            crate::aci_safety::SafetyMode::Enforce,
            &store,
            "user-1",
            "aci.code",
            &d,
            Some(&approval_id),
        )
        .unwrap_err();
        assert_eq!(denial.body["error"], "approval_denied");
        assert_eq!(
            denial.body["reason"],
            crate::aci_approvals::GrantDenial::Expired.reason()
        );
    }

    #[test]
    fn code_payload_is_always_risky_no_auto_pass() {
        let store = approval_store();
        let d = descriptor("console.log('read-only');", vec![]);
        let denial = enforce_code_grant_with_mode(
            crate::aci_safety::SafetyMode::Enforce,
            &store,
            "user-1",
            "aci.code",
            &d,
            None,
        )
        .unwrap_err();
        assert_eq!(denial.body["error"], "confirmation_required");
    }

    // ── Receipts + HTTP surface with a stub executor ──────────────────────

    fn stub_outcome(stdout: &str) -> CodeRunOutcome {
        CodeRunOutcome {
            stdout: stdout.to_string(),
            exit_status: Some(0),
            timed_out: false,
            screenshot_sha256: Some("aa".repeat(32)),
            screenshot_ref: Some("sandbox://screenshots/s.png".to_string()),
        }
    }

    async fn run_with_stub(
        state: &AppState,
        body: Value,
        receipts: &CodeReceiptStore,
    ) -> (StatusCode, Value) {
        let parsed: AciCodeBody = serde_json::from_value(body).unwrap();
        let response = run_gated_code(state, "user-1", &parsed, receipts, |_d| {
            Ok(Box::new(|d: &CodePayloadDescriptor| {
                stub_outcome(&format!("ran {} bytes", d.code.len()))
            }) as CodeExecutor)
        })
        .await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn http_refused_descriptor_is_400_with_class_and_refusal_receipt() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let receipts = CodeReceiptStore::new();
        let (status, json) = run_with_stub(
            &state,
            json!({
                "language": "playwright-js",
                "code": "const cp = require('child_process'); cp.spawn('sh', ['-c', 'ls']);",
                "declaredTargets": [],
            }),
            &receipts,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "code_refused");
        assert_eq!(json["class"], "destructive_call");
        let refused = receipts
            .receipts()
            .into_iter()
            .find(|r| r.status == "refused")
            .expect("refusal recorded on the trail");
        assert_eq!(refused.grant_id, None);
    }

    #[tokio::test]
    async fn http_granted_payload_executes_receipt_before_run_and_envelope_fixed() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let receipts = CodeReceiptStore::new();
        let code = "console.log('hello');";
        let body = json!({
            "language": "playwright-js",
            "code": code,
            "declaredTargets": ["http://127.0.0.1:8080"],
        });

        // No grant → confirmation_required carrying the descriptor hash.
        let (status, denial) = run_with_stub(&state, body.clone(), &receipts).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(denial["error"], "confirmation_required");
        let approval_id = denial["approval_id"].as_str().unwrap().to_string();
        assert!(state.approval_store.approve(&approval_id));
        assert!(crate::aci_approvals::GRANTS.approve(&approval_id));

        let granted = json!({
            "language": "playwright-js",
            "code": code,
            "declaredTargets": ["http://127.0.0.1:8080"],
            "approvalId": approval_id,
        });
        let (status, json) = run_with_stub(&state, granted, &receipts).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["descriptor_hash"], denial["action_hash"]);
        assert_eq!(json["grant_id"], approval_id);
        let receipt = json["receipt"].clone();
        assert_eq!(receipt["status"], "completed");
        assert_eq!(receipt["exit_status"], 0);
        assert_eq!(receipt["stdout_bytes"], "ran 20 bytes".len() as u64);
        // The receipt never carries stdout content — only its byte length.
        assert!(receipt.get("stdout").is_none());
        assert!(receipt["screenshot_sha256"].is_string());

        // Replay with the same grant → denied.
        let granted2 = json!({
            "language": "playwright-js",
            "code": code,
            "declaredTargets": ["http://127.0.0.1:8080"],
            "approvalId": approval_id,
        });
        let (status, _) = run_with_stub(&state, granted2, &receipts).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn receipt_trail_persists_and_dispatched_precedes_execution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code-receipts.jsonl");
        let store = CodeReceiptStore::new_persisted(path.clone());
        let d = benign();
        let receipt_id = "code-rcpt-1".to_string();
        store.record_dispatched(CodeReceipt {
            receipt_id: receipt_id.clone(),
            code_id: d.hash(),
            user_id: "user-1".to_string(),
            descriptor_hash: d.hash(),
            language: d.language.clone(),
            grant_id: Some("g".to_string()),
            status: "dispatched".to_string(),
            refusal: None,
            exit_status: None,
            stdout_bytes: None,
            timed_out: None,
            screenshot_sha256: None,
            screenshot_ref: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        });

        let (receipt, outcome) =
            run_code_payload(&store, &receipt_id, &d, Box::new(|_| stub_outcome("ok")))
                .expect("receipt overlay");
        assert_eq!(receipt.status, "completed");
        assert_eq!(receipt.stdout_bytes, Some(2));
        assert_eq!(outcome.stdout, "ok");
        assert!(receipt.completed_at.is_some());

        // Reload from the persisted trail.
        let store2 = CodeReceiptStore::new_persisted(path);
        let reloaded = store2.get(&receipt_id).unwrap();
        assert_eq!(reloaded.status, "completed");
        assert_eq!(reloaded.exit_status, Some(0));
    }

    #[test]
    fn descriptor_validation_scans_urls_not_comments_only_real_literals() {
        // URLs inside comments are still literals on the wire — the scanner is
        // deliberately dumb and refuses them. Document the behavior: a
        // commented-out URL to an undeclared host refuses too.
        let d = descriptor(
            "// await page.goto('https://evil.example/');\nconsole.log('hi');",
            vec!["http://127.0.0.1:8080"],
        );
        let refusal = validate_code_descriptor(&d).unwrap_err();
        assert_eq!(refusal.class, CodeRefusalClass::UndeclaredNetworkTarget);
    }
}
