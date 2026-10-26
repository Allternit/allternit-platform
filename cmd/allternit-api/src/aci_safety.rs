//! Backend safety policy enforcement for the Agent-Computer Interface (ACI).
//!
//! Mirrors the extension-side `browser-agent/safety/` layer so that a
//! compromised or misconfigured client cannot bypass host allowlisting,
//! sensitive-data masking, or circuit-breaker limits when proxying to the ACU
//! computer-use gateway.
//!
//! Also hosts the product-scoped confirmation taxonomy (design decision D2):
//! every computer-use entry route — the ACU loop, the direct
//! `/api/v1/computers/:id/*` control routes, and the `/tools/execute`
//! capability path — classifies its actions as reversible vs.
//! risky/irreversible and routes the latter through hash-bound approval
//! grants (see `aci_approvals`).

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Safety mode: disabled, audit-only (log but allow), or enforce (default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyMode {
    Off,
    Audit,
    Enforce,
}

impl SafetyMode {
    pub fn from_env() -> Self {
        match std::env::var("ALLTERNIT_ACI_SAFETY_MODE")
            .ok()
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            Some("off") | Some("disabled") | Some("false") => SafetyMode::Off,
            Some("audit") | Some("warn") => SafetyMode::Audit,
            _ => SafetyMode::Enforce,
        }
    }

    pub fn is_off(&self) -> bool {
        matches!(self, SafetyMode::Off)
    }
}

/// Kinds of actions that require human handoff instead of autonomous execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveActionType {
    FormSubmit,
    Payment,
    Captcha,
    IdentityVerification,
    Download,
    FileUpload,
}

impl SensitiveActionType {
    fn detect(goal: &str) -> Vec<Self> {
        let lower = goal.to_lowercase();
        let mut found = Vec::new();
        if lower.contains("captcha") {
            found.push(Self::Captcha);
        }
        if lower.contains("payment") || lower.contains("checkout") || lower.contains("credit card") || lower.contains("billing") {
            found.push(Self::Payment);
        }
        if lower.contains("identity verification") || lower.contains("verify identity") || lower.contains("kyc") || lower.contains("ssn") {
            found.push(Self::IdentityVerification);
        }
        if lower.contains("submit form") || lower.contains("click submit") || lower.contains("confirm order") {
            found.push(Self::FormSubmit);
        }
        if lower.contains("download") && (lower.contains("file") || lower.contains("attachment")) {
            found.push(Self::Download);
        }
        if lower.contains("upload") && (lower.contains("file") || lower.contains("attachment")) {
            found.push(Self::FileUpload);
        }
        found
    }

    /// The one-task grant action (`task_grants`) this class falls under, if any.
    /// `payment` is listed so callers see it, but no grant ever covers it.
    pub fn grant_action(&self) -> Option<&'static str> {
        match self {
            Self::FormSubmit => Some("submit"),
            Self::FileUpload => Some("upload"),
            Self::Payment => Some("payment"),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::FormSubmit => "form submission",
            Self::Payment => "payment",
            Self::Captcha => "CAPTCHA",
            Self::IdentityVerification => "identity verification",
            Self::Download => "file download",
            Self::FileUpload => "file upload",
        }
    }
}

/// Per-run safety decision returned to the caller.
#[derive(Debug, Serialize)]
pub struct SafetyDecision {
    pub allowed: bool,
    pub sanitized_goal: String,
    pub reason: Option<String>,
    pub handoff_required: bool,
    pub sensitive_actions: Vec<SensitiveActionType>,
}

/// Host allowlist/blocklist policy loaded from environment.
#[derive(Debug, Clone)]
pub struct HostPolicy {
    pub mode: SafetyMode,
    pub allowed_hosts: Vec<String>,
    pub blocked_hosts: Vec<String>,
    pub allowed_patterns: Vec<String>,
}

impl HostPolicy {
    pub fn from_env() -> Self {
        let allowed_hosts = parse_host_list("ALLTERNIT_ACI_ALLOWED_HOSTS");
        let blocked_hosts = parse_host_list("ALLTERNIT_ACI_BLOCKED_HOSTS");
        let allowed_patterns = parse_host_list("ALLTERNIT_ACI_ALLOWED_PATTERNS");
        Self {
            mode: SafetyMode::from_env(),
            allowed_hosts,
            blocked_hosts,
            allowed_patterns,
        }
    }

    pub fn allows(&self, url: &str) -> bool {
        let Some(host) = extract_host(url) else {
            return false;
        };

        if self.blocked_hosts.iter().any(|b| host.eq_ignore_ascii_case(b)) {
            return false;
        }

        if self.mode == SafetyMode::Off || self.allowed_hosts.is_empty() {
            return true;
        }

        if self.allowed_hosts.iter().any(|a| host.eq_ignore_ascii_case(a)) {
            return true;
        }

        if self
            .allowed_patterns
            .iter()
            .any(|p| matches_wildcard(&host, p))
        {
            return true;
        }

        false
    }
}

fn parse_host_list(env_var: &str) -> Vec<String> {
    std::env::var(env_var)
        .ok()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(|h| h.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn extract_host(url: &str) -> Option<String> {
    let url = url.trim();
    // Try to parse as a full URL first.
    if let Ok(parsed) = url::Url::parse(url) {
        return parsed.host_str().map(|h| h.to_lowercase());
    }
    // Fallback: extract a bare hostname or host:port.
    let host = url
        .split_once('/')
        .map(|(h, _)| h)
        .unwrap_or(url)
        .split_once(':')
        .map(|(h, _)| h)
        .unwrap_or(url)
        .to_lowercase();
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host)
}

fn matches_wildcard(host: &str, pattern: &str) -> bool {
    if pattern.starts_with("*.") {
        let suffix = &pattern[2..];
        return host == suffix || host.ends_with(&format!(".{}", suffix));
    }
    host == pattern
}

/// Mask sensitive tokens, keys, and credentials in the goal text.
pub fn mask_sensitive_data(goal: &str) -> String {
    let mut out = goal.to_string();

    // API keys / bearer tokens / secrets.
    static KEY_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"(?i)\b(?:api[_-]?key|apikey|secret|token|password|passwd|pwd)\s*[:=]\s*['"]?([A-Za-z0-9_\-./+=]{8,})['"]?"#).unwrap()
    });
    out = KEY_RE
        .replace_all(&out, |caps: &regex::Captures| {
            let key = &caps[1];
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "***".to_string()
            };
            caps[0].replace(&caps[1], &masked)
        })
        .into_owned();

    // Credit card numbers.
    static CC_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b").unwrap());
    out = CC_RE.replace_all(&out, "[CREDIT-CARD-REDACTED]").into_owned();

    // SSN-like patterns.
    static SSN_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\b\d{3}[\s\-]\d{2}[\s\-]\d{4}\b").unwrap());
    out = SSN_RE.replace_all(&out, "[SSN-REDACTED]").into_owned();

    out
}

/// Extract likely URLs from the goal text.
pub fn extract_urls(text: &str) -> Vec<String> {
    static URL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"https?://[^\s<>"{}|\\^`\[\]]+|(?:[a-zA-Z0-9][a-zA-Z0-9\-]{1,63}\.)+[a-zA-Z]{2,}(?::\d+)?(?:/[^\s<>"{}|\\^`\[\]]*)?"#).unwrap()
    });
    URL_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

// ─── Circuit-breaker / rate-limit state ─────────────────────────────────────

#[derive(Debug, Default)]
struct WindowState {
    actions: Vec<Instant>,
    consecutive_errors: u32,
    cooldown_until: Option<Instant>,
}

/// In-memory rate limiter keyed by organization/user id.
pub struct CircuitBreaker {
    max_per_minute: u32,
    max_per_hour: u32,
    cooldown_after_burst_ms: u64,
    cooldown_after_errors_ms: u64,
    consecutive_error_threshold: u32,
    state: Mutex<HashMap<String, WindowState>>,
}

impl CircuitBreaker {
    pub fn from_env() -> Self {
        Self {
            max_per_minute: env_u32("ALLTERNIT_ACI_MAX_ACTIONS_PER_MINUTE", 30),
            max_per_hour: env_u32("ALLTERNIT_ACI_MAX_ACTIONS_PER_HOUR", 300),
            cooldown_after_burst_ms: env_u64("ALLTERNIT_ACI_BURST_COOLDOWN_MS", 5000),
            cooldown_after_errors_ms: env_u64("ALLTERNIT_ACI_ERROR_COOLDOWN_MS", 10000),
            consecutive_error_threshold: env_u32("ALLTERNIT_ACI_ERROR_THRESHOLD", 5),
            state: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, key: &str) -> Result<(), String> {
        let mut store = self.state.lock().unwrap();
        let entry = store.entry(key.to_string()).or_default();
        let now = Instant::now();

        // Prune old actions.
        let one_minute_ago = now - Duration::from_secs(60);
        let one_hour_ago = now - Duration::from_secs(3600);
        entry.actions.retain(|t| *t > one_hour_ago);

        // Check cooldown.
        if let Some(until) = entry.cooldown_until {
            if now < until {
                let wait = (until - now).as_secs();
                return Err(format!("rate limited: cooldown for {} more seconds", wait));
            }
            entry.cooldown_until = None;
        }

        // Enforce limits.
        let last_minute = entry.actions.iter().filter(|t| **t > one_minute_ago).count();
        if last_minute >= self.max_per_minute as usize {
            entry.cooldown_until = Some(now + Duration::from_millis(self.cooldown_after_burst_ms));
            return Err(format!(
                "rate limit exceeded: {} actions per minute",
                self.max_per_minute
            ));
        }
        let last_hour = entry.actions.len();
        if last_hour >= self.max_per_hour as usize {
            return Err(format!(
                "rate limit exceeded: {} actions per hour",
                self.max_per_hour
            ));
        }

        entry.actions.push(now);
        Ok(())
    }

    pub fn record_error(&self, key: &str) {
        let mut store = self.state.lock().unwrap();
        let entry = store.entry(key.to_string()).or_default();
        entry.consecutive_errors += 1;
        if entry.consecutive_errors >= self.consecutive_error_threshold {
            entry.cooldown_until =
                Some(Instant::now() + Duration::from_millis(self.cooldown_after_errors_ms));
            entry.consecutive_errors = 0;
        }
    }

    pub fn record_success(&self, key: &str) {
        let mut store = self.state.lock().unwrap();
        if let Some(entry) = store.get_mut(key) {
            entry.consecutive_errors = 0;
        }
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

// ─── Global enforcer ────────────────────────────────────────────────────────

pub(crate) static HOST_POLICY: Lazy<HostPolicy> = Lazy::new(HostPolicy::from_env);
static BREAKER: Lazy<CircuitBreaker> = Lazy::new(CircuitBreaker::from_env);

/// Evaluate an ACI run request against the backend safety policy.
///
/// `actor_key` should uniquely identify the caller (e.g. `org_id:user_id`).
pub fn evaluate_request(goal: &str, actor_key: &str) -> SafetyDecision {
    let mode = HOST_POLICY.mode;
    let sanitized_goal = mask_sensitive_data(goal);
    let sensitive_actions = SensitiveActionType::detect(goal);
    let handoff_required = !sensitive_actions.is_empty();

    if handoff_required && mode == SafetyMode::Enforce {
        return SafetyDecision {
            allowed: false,
            sanitized_goal,
            reason: Some(format!(
                "human handoff required for: {}",
                sensitive_actions
                    .iter()
                    .map(|a| a.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            handoff_required: true,
            sensitive_actions,
        };
    }

    if handoff_required && mode == SafetyMode::Audit {
        tracing::warn!(
            actor = %actor_key,
            actions = ?sensitive_actions,
            "aci safety: sensitive action detected in audit mode"
        );
    }

    // Rate limit check.
    if let Err(reason) = BREAKER.check(actor_key) {
        if mode == SafetyMode::Enforce {
            return SafetyDecision {
                allowed: false,
                sanitized_goal,
                reason: Some(reason),
                handoff_required: false,
                sensitive_actions: Vec::new(),
            };
        }
        tracing::warn!(actor = %actor_key, %reason, "aci safety: rate limit breached");
    }

    // Host allowlist check.
    if mode != SafetyMode::Off {
        let urls = extract_urls(goal);
        for url in &urls {
            if !HOST_POLICY.allows(url) {
                let reason = format!("host not allowed: {}", url);
                if mode == SafetyMode::Enforce {
                    return SafetyDecision {
                        allowed: false,
                        sanitized_goal,
                        reason: Some(reason),
                        handoff_required: false,
                        sensitive_actions: Vec::new(),
                    };
                }
                tracing::warn!(actor = %actor_key, url = %url, "aci safety: blocked host in audit mode");
            }
        }
    }

    SafetyDecision {
        allowed: true,
        sanitized_goal,
        reason: None,
        handoff_required: false,
        sensitive_actions: Vec::new(),
    }
}

pub fn record_aci_error(actor_key: &str) {
    BREAKER.record_error(actor_key);
}

pub fn record_aci_success(actor_key: &str) {
    BREAKER.record_success(actor_key);
}

// ─── Confirmation taxonomy (product-scoped, all entry routes) ───────────────

/// How much human confirmation a computer-use action needs. Checked on EVERY
/// computer-use entry route (ACU loop, direct `/api/v1/computers/:id/*`
/// control routes, `/tools/execute` `computer_*` capability path), not just
/// the ACU loop path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationClass {
    /// Read-only or trivially undoable (move the cursor, read a file).
    Reversible,
    /// Mutates state but is usually undoable (click, type + submit, write a
    /// user file, run a mutating command).
    Risky,
    /// Destructive or hard to undo (`rm -rf`, writes to system paths,
    /// repartitioning, shutdown).
    Irreversible,
}

impl ConfirmationClass {
    pub fn requires_confirmation(&self) -> bool {
        !matches!(self, Self::Reversible)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Reversible => "reversible",
            Self::Risky => "risky",
            Self::Irreversible => "irreversible",
        }
    }
}

/// Classify a mouse input action.
pub fn classify_mouse_action(action: &str) -> ConfirmationClass {
    match action.to_lowercase().as_str() {
        // Moving/hovering/scrolls only change transient UI state.
        "move" | "hover" | "scroll" => ConfirmationClass::Reversible,
        // Clicks activate whatever is under the cursor (buttons, submits).
        _ => ConfirmationClass::Risky,
    }
}

/// Classify a keyboard input action.
pub fn classify_keyboard_action(
    action: &str,
    text: Option<&str>,
    key: Option<&str>,
) -> ConfirmationClass {
    match action.to_lowercase().as_str() {
        "type" => {
            // A trailing/embedded newline frequently submits the focused form.
            match text {
                Some(t) if t.contains('\n') || t.contains('\r') => ConfirmationClass::Risky,
                _ => ConfirmationClass::Reversible,
            }
        }
        "key" => match key.map(|k| k.to_lowercase()) {
            Some(k) if k.contains("return") || k.contains("enter") => ConfirmationClass::Risky,
            Some(_) => ConfirmationClass::Reversible,
            None => ConfirmationClass::Risky,
        },
        // Unknown keyboard actions default to requiring confirmation.
        _ => ConfirmationClass::Risky,
    }
}

/// Classify a guest shell command by its argv.
pub fn classify_shell_command(command: &[String]) -> ConfirmationClass {
    let joined = command.join(" ");
    let lower = joined.to_lowercase();

    // Destructive / hard-to-undo operations.
    const IRREVERSIBLE: &[&str] = &[
        "mkfs", "fdisk", "wipefs", "parted", "shutdown", "poweroff", "reboot",
        "halt", ":(){", "dd if=", "rm -rf /", "rm -rf ~", "rm -rf /*",
        "format c:", "del /f /s /q c:", "> /dev/sd",
    ];
    if IRREVERSIBLE.iter().any(|p| lower.contains(p)) {
        return ConfirmationClass::Irreversible;
    }

    // State-mutating or dual-use operations.
    const RISKY_TOKENS: &[&str] = &[
        "rm", "rmdir", "del", "delete", "drop", "truncate", "chmod", "chown",
        "chgrp", "sudo", "doas", "apt", "apt-get", "dnf", "yum", "brew", "pip",
        "pip3", "npm", "pnpm", "yarn", "gem", "ssh", "scp", "sftp", "kill",
        "killall", "pkill", "mount", "umount", "crontab", "passwd", "useradd",
        "userdel", "usermod", "groupadd", "tee", "systemctl", "service",
        "launchctl", "git", "curl", "wget",
    ];
    let tokens: Vec<String> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    if RISKY_TOKENS.iter().any(|t| tokens.iter().any(|tok| tok == t)) {
        return ConfirmationClass::Risky;
    }
    // Shell redirection writes files even when the binary itself is read-only.
    if lower.contains('>') {
        return ConfirmationClass::Risky;
    }
    ConfirmationClass::Reversible
}

/// Classify a guest file write by destination path.
pub fn classify_file_write(path: &str) -> ConfirmationClass {
    let lower = path.to_lowercase();
    const SYSTEM_PREFIXES: &[&str] = &[
        "/etc/", "/usr/", "/bin/", "/sbin/", "/boot/", "/lib/", "/lib64/",
        "/root/", "~/.ssh", "c:\\windows", "c:\\program files",
    ];
    if SYSTEM_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return ConfirmationClass::Irreversible;
    }
    ConfirmationClass::Risky
}

/// Denial produced by [`enforce_confirmation`], ready to become an HTTP
/// response on any entry route.
#[derive(Debug)]
pub struct ConfirmationDenial {
    pub status: axum::http::StatusCode,
    pub body: serde_json::Value,
}

/// Enforce the confirmation taxonomy for one action on any computer-use
/// entry route. `descriptor` is the canonical action payload that gets hashed
/// into the grant; it must be built deterministically from the request (see
/// `computer_control::control_action_descriptor`).
///
/// Returns `Ok(())` when the action may proceed. When confirmation is required
/// and no valid grant is presented, returns `Err` with a fresh
/// `confirmation_required` denial whose `approval_id` the client routes
/// through the handoff endpoints; when a grant IS presented but does not
/// match, returns `Err` with an `approval_denied` denial.
pub fn enforce_confirmation(
    approval_store: &crate::permission_policy::ApprovalStore,
    user_id: &str,
    route: &str,
    class: ConfirmationClass,
    descriptor: &serde_json::Value,
    approval_id: Option<&str>,
) -> Result<(), ConfirmationDenial> {
    enforce_confirmation_with_mode(
        HOST_POLICY.mode,
        approval_store,
        user_id,
        route,
        class,
        descriptor,
        approval_id,
    )
}

/// Mode-injectable core of [`enforce_confirmation`], so tests are hermetic
/// regardless of `ALLTERNIT_ACI_SAFETY_MODE`.
pub fn enforce_confirmation_with_mode(
    mode: SafetyMode,
    approval_store: &crate::permission_policy::ApprovalStore,
    user_id: &str,
    route: &str,
    class: ConfirmationClass,
    descriptor: &serde_json::Value,
    approval_id: Option<&str>,
) -> Result<(), ConfirmationDenial> {
    use serde_json::json;

    if !class.requires_confirmation() || mode == SafetyMode::Off {
        return Ok(());
    }
    if mode == SafetyMode::Audit {
        tracing::warn!(
            actor = %user_id,
            route = %route,
            class = %class.label(),
            descriptor = %descriptor,
            "aci safety: confirmation-required action allowed in audit mode"
        );
        return Ok(());
    }

    let action_hash = crate::aci_approvals::hash_action_payload(descriptor);
    if let Some(id) = approval_id {
        return match crate::aci_approvals::GRANTS.redeem(user_id, id, &action_hash) {
            Ok(_) => Ok(()),
            Err(denial) => Err(ConfirmationDenial {
                status: axum::http::StatusCode::FORBIDDEN,
                body: json!({
                    "error": "approval_denied",
                    "approval_id": id,
                    "action_hash": action_hash,
                    "reason": denial.reason(),
                }),
            }),
        };
    }

    // No grant presented: create the handoff record and the hash-bound grant
    // under one shared id so `/api/aci/handoff/:id/*` drives both.
    let approval_id = approval_store.create(
        user_id,
        "computer.confirmation_required",
        &json!({
            "route": route,
            "confirmation_class": class.label(),
            "action_hash": action_hash,
        }),
    );
    crate::aci_approvals::GRANTS.issue_with_id(
        &approval_id,
        user_id,
        &action_hash,
        crate::aci_approvals::grant_ttl_secs(),
    );
    Err(ConfirmationDenial {
        status: axum::http::StatusCode::FORBIDDEN,
        body: json!({
            "error": "confirmation_required",
            "approval_id": approval_id,
            "action_hash": action_hash,
            "confirmation_class": class.label(),
            "message": format!("{} action requires human approval before execution", class.label()),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_sensitive_data() {
        let goal = "log in with api_key=sk_test_1234567890abcdef and ssn 123-45-6789";
        let masked = mask_sensitive_data(goal);
        assert!(!masked.contains("sk_test_1234567890abcdef"));
        assert!(masked.contains("[SSN-REDACTED]"));
        assert!(masked.contains("api_key="));
    }

    #[test]
    fn test_extract_urls() {
        let text = "go to https://example.com/path and also checkout github.com/allternit";
        let urls = extract_urls(text);
        assert!(urls.iter().any(|u| u.contains("example.com")));
        assert!(urls.iter().any(|u| u.contains("github.com")));
    }

    #[test]
    fn test_host_policy_allows() {
        let policy = HostPolicy {
            mode: SafetyMode::Enforce,
            allowed_hosts: vec!["example.com".to_string()],
            blocked_hosts: vec!["evil.com".to_string()],
            allowed_patterns: vec!["*.safe.com".to_string()],
        };
        assert!(policy.allows("https://example.com/foo"));
        assert!(policy.allows("https://app.safe.com/bar"));
        assert!(!policy.allows("https://evil.com"));
        assert!(!policy.allows("https://other.com"));
    }

    #[test]
    fn test_circuit_breaker_limits() {
        let breaker = CircuitBreaker {
            max_per_minute: 2,
            max_per_hour: 10,
            cooldown_after_burst_ms: 10,
            cooldown_after_errors_ms: 10,
            consecutive_error_threshold: 2,
            state: Mutex::new(HashMap::new()),
        };
        let key = "org:user";
        assert!(breaker.check(key).is_ok());
        assert!(breaker.check(key).is_ok());
        assert!(breaker.check(key).is_err());
    }

    #[test]
    fn test_classify_mouse_actions() {
        assert_eq!(classify_mouse_action("move"), ConfirmationClass::Reversible);
        assert_eq!(classify_mouse_action("SCROLL"), ConfirmationClass::Reversible);
        assert_eq!(classify_mouse_action("click"), ConfirmationClass::Risky);
        assert_eq!(classify_mouse_action("doubleclick"), ConfirmationClass::Risky);
    }

    #[test]
    fn test_classify_keyboard_actions() {
        assert_eq!(
            classify_keyboard_action("type", Some("hello world"), None),
            ConfirmationClass::Reversible
        );
        assert_eq!(
            classify_keyboard_action("type", Some("hello\n"), None),
            ConfirmationClass::Risky
        );
        assert_eq!(
            classify_keyboard_action("key", None, Some("Return")),
            ConfirmationClass::Risky
        );
        assert_eq!(
            classify_keyboard_action("key", None, Some("Left")),
            ConfirmationClass::Reversible
        );
        assert_eq!(
            classify_keyboard_action("mash", None, None),
            ConfirmationClass::Risky
        );
    }

    #[test]
    fn test_classify_shell_commands() {
        use ConfirmationClass::*;
        assert_eq!(
            classify_shell_command(&["ls".into(), "-la".into()]),
            Reversible
        );
        assert_eq!(
            classify_shell_command(&["cat".into(), "/tmp/log".into()]),
            Reversible
        );
        assert_eq!(
            classify_shell_command(&["echo".into(), "hi".into()]),
            Reversible
        );
        assert_eq!(
            classify_shell_command(&["rm".into(), "/tmp/scratch".into()]),
            Risky
        );
        assert_eq!(
            classify_shell_command(&["apt".into(), "install".into(), "vim".into()]),
            Risky
        );
        assert_eq!(
            classify_shell_command(&["git".into(), "push".into()]),
            Risky
        );
        assert_eq!(
            classify_shell_command(&["echo".into(), "x".into(), ">".into(), "/tmp/f".into()]),
            Risky
        );
        assert_eq!(
            classify_shell_command(&["rm".into(), "-rf".into(), "/".into()]),
            Irreversible
        );
        assert_eq!(
            classify_shell_command(&["mkfs.ext4".into(), "/dev/sda1".into()]),
            Irreversible
        );
        // Read-only binaries must not trip on substrings (e.g. "delve").
        assert_eq!(
            classify_shell_command(&["cat".into(), "deliverable.txt".into()]),
            Reversible
        );
    }

    #[test]
    fn test_classify_file_writes() {
        use ConfirmationClass::*;
        assert_eq!(classify_file_write("/home/user/notes.txt"), Risky);
        assert_eq!(classify_file_write("C:\\Users\\joe\\out.txt"), Risky);
        assert_eq!(classify_file_write("/etc/passwd"), Irreversible);
        assert_eq!(classify_file_write("~/.ssh/authorized_keys"), Irreversible);
        assert_eq!(classify_file_write("c:\\windows\\system32\\x.dll"), Irreversible);
    }

    #[test]
    fn test_enforce_confirmation_reversible_never_blocks() {
        let store = crate::permission_policy::ApprovalStore::new();
        let descriptor = serde_json::json!({"route": "computer.mouse", "action": "move"});
        assert!(enforce_confirmation_with_mode(
            SafetyMode::Enforce,
            &store,
            "user-1",
            "computer.mouse",
            ConfirmationClass::Reversible,
            &descriptor,
            None,
        )
        .is_ok());
    }

    #[test]
    fn test_enforce_confirmation_enforce_mode_requires_grant() {
        let store = crate::permission_policy::ApprovalStore::new();
        let descriptor = serde_json::json!({"route": "computer.shell", "command": ["rm", "x"]});
        let denial = enforce_confirmation_with_mode(
            SafetyMode::Enforce,
            &store,
            "user-1",
            "computer.shell",
            ConfirmationClass::Risky,
            &descriptor,
            None,
        )
        .expect_err("risky action without a grant must be denied");
        assert_eq!(denial.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(denial.body["error"], "confirmation_required");
        let approval_id = denial.body["approval_id"].as_str().unwrap().to_string();

        // The denial minted a pending grant under the same id; an approved
        // grant bound to the same hash must redeem cleanly.
        assert!(crate::aci_approvals::GRANTS.approve(&approval_id));
        assert!(enforce_confirmation_with_mode(
            SafetyMode::Enforce,
            &store,
            "user-1",
            "computer.shell",
            ConfirmationClass::Risky,
            &descriptor,
            Some(&approval_id),
        )
        .is_ok());

        // Single-use: a second redemption with the same grant is denied.
        let second = enforce_confirmation_with_mode(
            SafetyMode::Enforce,
            &store,
            "user-1",
            "computer.shell",
            ConfirmationClass::Risky,
            &descriptor,
            Some(&approval_id),
        )
        .expect_err("grant must be single-use");
        assert_eq!(second.body["error"], "approval_denied");

        // A different payload hash never matches the grant.
        let other = serde_json::json!({"route": "computer.shell", "command": ["rm", "y"]});
        let third = enforce_confirmation_with_mode(
            SafetyMode::Enforce,
            &store,
            "user-1",
            "computer.shell",
            ConfirmationClass::Risky,
            &other,
            Some(&approval_id),
        )
        .expect_err("hash mismatch must be denied");
        assert_eq!(third.body["error"], "approval_denied");
    }

    #[test]
    fn test_enforce_confirmation_audit_and_off_modes_allow() {
        let store = crate::permission_policy::ApprovalStore::new();
        let descriptor = serde_json::json!({"route": "computer.shell", "command": ["rm", "x"]});
        for mode in [SafetyMode::Audit, SafetyMode::Off] {
            assert!(enforce_confirmation_with_mode(
                mode,
                &store,
                "user-1",
                "computer.shell",
                ConfirmationClass::Risky,
                &descriptor,
                None,
            )
            .is_ok());
        }
    }
}
