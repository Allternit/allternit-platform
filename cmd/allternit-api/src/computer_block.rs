//! Blocked-page detection for agent runs (agent-browser plan, decision 3).
//!
//! After a `read_ui` (and a browser `get_page_text` / `read_page`) the result
//! is checked for the pages an agent can't get past on its own: a login wall,
//! a two-factor prompt, a captcha frame, a bot-challenge or "access denied"
//! page (Cloudflare, Akamai, PerimeterX, Incapsula, DataDome) and a
//! rate-limit page. A match emits `computer.block_detected` on the toolset
//! events stream, sends an `aci_blocked` push, and holds the run on a gate
//! like `request_human`'s until the person answers
//! `POST /computers/:id/block-resolve`:
//!
//! - `continue`: the person dealt with it (signed in, solved the captcha);
//!   the run goes on. The same domain + kind won't hold this run again.
//! - `stop`: the run ends here.
//! - `rerun`: the run ends here and the CLIENT starts a new run on the
//!   person's Mac with `rerun_of: <this run id>`. The server never moves a
//!   run between computers, and cloud logins don't carry over.
//!
//! Detection is a heuristic. It has false positives and misses, so a match
//! only pauses and asks; it never ends a run by itself (a hold nobody answers
//! lets the run go on with a note). Nothing here reads or stores a value the
//! person types.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::computer_routes::ComputerResponse;
use crate::computer_toolset::{screen_info, text, Fail, Target, ToolsetResult};
use crate::AppState;

// ---------------------------------------------------------------------------
// Detection.
// ---------------------------------------------------------------------------

/// What kind of wall the page is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Login,
    TwoFactor,
    Captcha,
    BlockPage,
    RateLimited,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Login => "login",
            BlockKind::TwoFactor => "two_factor",
            BlockKind::Captcha => "captcha",
            BlockKind::BlockPage => "block_page",
            BlockKind::RateLimited => "rate_limited",
        }
    }
}

/// One detected wall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub kind: BlockKind,
    /// The page's domain (lower case, no `www.`) when the result names a URL.
    pub domain: Option<String>,
    /// The phrase that matched, for logs and tests.
    pub signal: &'static str,
}

/// Pages longer than this (characters of collected text) are real content,
/// not a wall: generic phrases ("access denied", "verification code") only
/// count on short pages. Vendor signatures count at any length.
const SHORT_PAGE: usize = 6_000;
/// How much text one check reads.
const MAX_TEXT: usize = 64 * 1024;

/// Vendor signatures: these only appear on the wall itself.
const RATE_LIMITED_STRONG: &[&str] = &[
    "429 too many requests",
    "rate limit exceeded",
    "error 1015",
    "you are being rate limited",
    "you have been rate limited",
    "you've been rate limited",
];
const BLOCK_PAGE_STRONG: &[&str] = &[
    "attention required! | cloudflare",
    "cloudflare ray id",
    "checking your browser before accessing",
    "checking if the site connection is secure",
    "performance & security by cloudflare",
    "widget containing a cloudflare security challenge",
    "errors.edgesuite.net",
    "perimeterx",
    "px-captcha",
    "_incapsula_resource",
    "incapsula incident id",
    "sorry, you have been blocked",
    "captcha-delivery.com",
];
const CAPTCHA_STRONG: &[&str] = &[
    "i'm not a robot",
    "i am not a robot",
    "recaptcha challenge",
    "recaptcha-checkbox",
    "hcaptcha security challenge",
    "cf-turnstile",
    "funcaptcha",
    "arkoselabs",
    "geetest",
    "select all images with",
    "select all squares with",
];
/// Generic phrases: short pages only.
const RATE_LIMITED_SHORT: &[&str] = &["too many requests", "you're doing that too often"];
const BLOCK_PAGE_SHORT: &[&str] = &[
    "access denied",
    "access to this page has been denied",
    "you don't have permission to access",
    "403 forbidden",
    "please verify you are a human",
    "press & hold",
    "press and hold",
    "just a moment...",
    "unusual traffic from your computer network",
    "are you a robot",
];
const CAPTCHA_SHORT: &[&str] = &["verify you are human", "verify you're human", "prove you're human", "complete the captcha", "solve the captcha", "enter the characters you see"];
const TWO_FACTOR_SHORT: &[&str] = &[
    "two-factor authentication",
    "two-step verification",
    "2-step verification",
    "two-factor",
    "verification code",
    "authenticator app",
    "enter the code we sent",
    "enter the 6-digit code",
    "one-time code",
    "one-time passcode",
];
const LOGIN_WORDS: &[&str] = &["sign in", "sign-in", "signin", "log in", "log-in", "login"];

/// Check a `read_ui` reply (or any structured result): every string in it is
/// read as page text, a secure text field counts as a password field, and the
/// first URL names the domain.
pub fn detect(reply: &Value) -> Option<Detection> {
    let mut scan = Scan::default();
    scan.walk(reply, None);
    let url = scan.url.clone();
    classify(&scan.text, scan.secure_field, url.as_deref())
}

/// Check plain page text (a browser `get_page_text` / `read_page` result, or
/// the extension's DOM text), with the page's URL when known.
pub fn detect_text(page_text: &str, url: Option<&str>) -> Option<Detection> {
    let lower = normalize(&page_text.chars().take(MAX_TEXT).collect::<String>());
    let secure = lower.contains("type=\"password\"") || lower.contains("type=password");
    classify(&lower, secure, url)
}

/// Lower case, curly quotes made straight.
fn normalize(s: &str) -> String {
    s.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'")
}

#[derive(Default)]
struct Scan {
    text: String,
    secure_field: bool,
    url: Option<String>,
}

impl Scan {
    fn walk(&mut self, v: &Value, key: Option<&str>) {
        if self.text.len() >= MAX_TEXT {
            return;
        }
        match v {
            Value::String(s) => {
                let lower = normalize(s);
                let k = key.map(str::to_ascii_lowercase);
                if matches!(k.as_deref(), Some("role" | "subrole" | "type" | "input_type"))
                    && (lower.contains("securetextfield") || lower == "password" || lower == "passwordfield")
                {
                    self.secure_field = true;
                }
                if self.url.is_none() && (lower.starts_with("http://") || lower.starts_with("https://")) {
                    let url_key = matches!(k.as_deref(), Some("url" | "page_url" | "document_url" | "axurl" | "axdocument" | "href"));
                    if url_key || key.is_none() || k.as_deref() == Some("value") {
                        self.url = Some(lower.clone());
                    }
                }
                self.text.push_str(&lower);
                self.text.push('\n');
            }
            Value::Bool(true) if key.is_some_and(|k| k.eq_ignore_ascii_case("secure")) => self.secure_field = true,
            Value::Array(items) => items.iter().for_each(|i| self.walk(i, key)),
            Value::Object(map) => map.iter().for_each(|(k, v)| self.walk(v, Some(k))),
            _ => {}
        }
    }
}

fn first_match(hay: &str, needles: &[&'static str]) -> Option<&'static str> {
    needles.iter().copied().find(|n| hay.contains(n))
}

fn classify(hay: &str, secure_field: bool, url: Option<&str>) -> Option<Detection> {
    let short = hay.len() <= SHORT_PAGE;
    let pick = |strong: &[&'static str], generic: &[&'static str]| first_match(hay, strong).or_else(|| short.then(|| first_match(hay, generic)).flatten());
    let found = pick(RATE_LIMITED_STRONG, RATE_LIMITED_SHORT)
        .map(|s| (BlockKind::RateLimited, s))
        .or_else(|| pick(BLOCK_PAGE_STRONG, BLOCK_PAGE_SHORT).map(|s| (BlockKind::BlockPage, s)))
        .or_else(|| pick(CAPTCHA_STRONG, CAPTCHA_SHORT).map(|s| (BlockKind::Captcha, s)))
        .or_else(|| short.then(|| first_match(hay, TWO_FACTOR_SHORT)).flatten().map(|s| (BlockKind::TwoFactor, s)))
        .or_else(|| {
            // A login wall: a password field next to sign-in words. Without
            // the field, a short page has to say "password" itself.
            let words = first_match(hay, LOGIN_WORDS)?;
            (short && (secure_field || hay.contains("password"))).then_some((BlockKind::Login, words))
        });
    let (kind, signal) = found?;
    let domain = url.map(|u| crate::aci_credentials::normalize_credential_domain(Some(u))).filter(|d| !d.is_empty());
    Some(Detection { kind, domain, signal })
}

// ---------------------------------------------------------------------------
// The gate.
// ---------------------------------------------------------------------------

/// How long a hold waits for the person before the run goes on with a note.
const BLOCK_TTL: Duration = Duration::from_secs(600);
/// How long a "continue" keeps the same wall from holding the run again.
const CONTINUED_TTL: Duration = Duration::from_secs(6 * 3600);

/// The person's answer (`POST /computers/:id/block-resolve`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockAction {
    Continue,
    Stop,
    Rerun { target: Option<String> },
}

impl BlockAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockAction::Continue => "continue",
            BlockAction::Stop => "stop",
            BlockAction::Rerun { .. } => "rerun",
        }
    }
}

/// How a hold ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldOutcome {
    Continue,
    /// Nobody answered in time (or a newer hold replaced this one).
    TimedOut,
    Stop,
    Rerun { target: Option<String> },
}

/// A Mac the run can be re-run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunTarget {
    pub computer_id: String,
    pub name: String,
}

impl RerunTarget {
    fn to_json(&self) -> Value {
        json!({ "computer_id": self.computer_id, "name": self.name })
    }
}

struct BlockGate {
    tx: tokio::sync::oneshot::Sender<BlockAction>,
    run_id: String,
    domain: Option<String>,
    kind: BlockKind,
    rerun_targets: Vec<RerunTarget>,
    since: Instant,
}

/// At most one hold per computer.
static BLOCK_GATES: Lazy<Mutex<HashMap<String, BlockGate>>> = Lazy::new(|| Mutex::new(HashMap::new()));
/// (run, domain, kind) the person already said "continue" to.
static CONTINUED: Lazy<Mutex<HashMap<(String, String, BlockKind), Instant>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// What a resolve answered (for the route's reply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub run_id: String,
    pub domain: Option<String>,
    pub kind: BlockKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No hold is waiting on this computer (never held, answered, timed out).
    NoPending,
    /// `rerun_target` is not one of the hold's `rerun_targets`.
    UnknownTarget,
}

/// Answer the pending hold on a computer.
pub fn resolve(computer_id: &str, action: BlockAction) -> Result<Resolved, ResolveError> {
    let mut gates = BLOCK_GATES.lock().unwrap_or_else(|p| p.into_inner());
    if let (BlockAction::Rerun { target: Some(t) }, Some(gate)) = (&action, gates.get(computer_id)) {
        if !gate.rerun_targets.iter().any(|r| &r.computer_id == t) {
            return Err(ResolveError::UnknownTarget);
        }
    }
    let gate = gates.remove(computer_id).ok_or(ResolveError::NoPending)?;
    let resolved = Resolved { run_id: gate.run_id, domain: gate.domain, kind: gate.kind };
    // A waiter that already timed out has dropped its receiver; that is the
    // same as no pending hold.
    gate.tx.send(action).map_err(|_| ResolveError::NoPending)?;
    Ok(resolved)
}

/// The pending hold on a computer: (run id, domain, kind, seconds waiting).
pub fn pending(computer_id: &str) -> Option<(String, Option<String>, &'static str, u64)> {
    BLOCK_GATES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(computer_id)
        .map(|g| (g.run_id.clone(), g.domain.clone(), g.kind.as_str(), g.since.elapsed().as_secs()))
}

fn continued_key(run_id: &str, det: &Detection) -> (String, String, BlockKind) {
    (run_id.to_string(), det.domain.clone().unwrap_or_default(), det.kind)
}

/// Should this detection hold the run? Not when the person already said
/// "continue" to the same wall in this run.
pub fn should_hold(run_id: &str, det: &Detection) -> bool {
    let continued = CONTINUED.lock().unwrap_or_else(|p| p.into_inner());
    !continued.get(&continued_key(run_id, det)).is_some_and(|at| at.elapsed() < CONTINUED_TTL)
}

fn mark_continued(run_id: &str, det: &Detection) {
    let mut continued = CONTINUED.lock().unwrap_or_else(|p| p.into_inner());
    continued.retain(|_, at| at.elapsed() < CONTINUED_TTL);
    continued.insert(continued_key(run_id, det), Instant::now());
}

/// The run ended: drop what it kept in memory (continue marks, run-scoped
/// credential approvals).
pub fn forget_run(run_id: &str) {
    CONTINUED.lock().unwrap_or_else(|p| p.into_inner()).retain(|(r, _, _), _| r != run_id);
    crate::aci_credentials::RUN_CREDENTIAL_ALLOWS.forget_run(run_id);
}

/// The person's online Macs registered as this-device computers, other than
/// the one the run is on. Empty when none is online.
pub async fn rerun_targets(state: &Arc<AppState>, user_id: &str, exclude_computer: &str) -> Vec<RerunTarget> {
    let (db, owner, exclude) = (state.db.clone(), user_id.to_string(), exclude_computer.to_string());
    tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<RerunTarget>> {
        let conn = db.connect()?;
        let mut q = conn.prepare(
            "SELECT id, name FROM computers WHERE kind = 'local' AND provider = ?1 AND owner_type = 'user'
             AND owner_id = ?2 AND os = 'macos' AND status = 'running' AND id != ?3 ORDER BY last_activity_at DESC",
        )?;
        let rows = q.query_map(rusqlite::params![crate::computer_routes::THIS_DEVICE_PROVIDER, owner, exclude], |r| {
            Ok(RerunTarget { computer_id: r.get(0)?, name: r.get(1)? })
        })?;
        let out: rusqlite::Result<Vec<RerunTarget>> = rows.collect();
        out
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default()
}

fn emit(computer_id: &str, kind: &str, data: Value) {
    let event = json!({ "type": format!("computer.{kind}"), "ts": chrono::Utc::now().to_rfc3339(), "data": data });
    let _ = crate::computer_toolset::ACTION_EVENTS.send((computer_id.to_string(), event));
}

/// Hold the run at a detected wall until the person answers (or the hold
/// times out). Guests release the agent's lease for the hold so the person
/// can sign in or solve the captcha on the live view, and take it back on
/// "continue" or a timeout.
pub async fn hold(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    run_id: &str,
    det: &Detection,
) -> Result<HoldOutcome, Fail> {
    let targets = rerun_targets(state, &user.user_id, &computer.id).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    BLOCK_GATES.lock().unwrap_or_else(|p| p.into_inner()).insert(
        computer.id.clone(),
        BlockGate { tx, run_id: run_id.to_string(), domain: det.domain.clone(), kind: det.kind, rerun_targets: targets.clone(), since: Instant::now() },
    );

    let is_guest = matches!(target, Target::Guest { .. });
    let caller = crate::computer_toolset::agent_holder(user, Some(run_id));
    if is_guest {
        if let Err(e) = crate::computer_v2::release_for_person(state, &computer.id, &caller).await {
            BLOCK_GATES.lock().unwrap_or_else(|p| p.into_inner()).remove(&computer.id);
            return Err(e);
        }
    }

    let domain = det.domain.clone();
    emit(
        &computer.id,
        "block_detected",
        json!({
            "run_id": run_id,
            "computer_id": computer.id,
            "domain": domain,
            "kind": det.kind.as_str(),
            "rerun_targets": targets.iter().map(RerunTarget::to_json).collect::<Vec<_>>(),
            "timeout_ms": BLOCK_TTL.as_millis() as u64,
        }),
    );
    push_to_owner(
        &user.user_id,
        json!({
            "kind": "aci_blocked",
            "run_id": run_id,
            "computer_id": computer.id,
            "domain": domain,
            "block_kind": det.kind.as_str(),
            "rerun_targets": targets.iter().map(|t| t.computer_id.clone()).collect::<Vec<_>>(),
        }),
    );

    let answer = tokio::time::timeout(BLOCK_TTL, rx).await;
    {
        // Ours only: a newer hold on the same computer may have replaced it.
        let mut gates = BLOCK_GATES.lock().unwrap_or_else(|p| p.into_inner());
        if gates.get(&computer.id).is_some_and(|g| g.run_id == run_id && g.since.elapsed() >= BLOCK_TTL) {
            gates.remove(&computer.id);
        }
    }
    let outcome = match answer {
        Ok(Ok(BlockAction::Continue)) => HoldOutcome::Continue,
        Ok(Ok(BlockAction::Stop)) => HoldOutcome::Stop,
        Ok(Ok(BlockAction::Rerun { target })) => HoldOutcome::Rerun { target },
        Ok(Err(_)) | Err(_) => HoldOutcome::TimedOut,
    };

    match &outcome {
        HoldOutcome::Continue | HoldOutcome::TimedOut => {
            mark_continued(run_id, det);
            if is_guest {
                // A person still driving keeps control; the next action then
                // answers 423 and the model can request_human.
                let _ = crate::computer_v2::take_back_after_person(state, &computer.id, &caller).await?;
            }
        }
        HoldOutcome::Stop | HoldOutcome::Rerun { .. } => forget_run(run_id),
    }

    let (action, rerun_target) = match &outcome {
        HoldOutcome::Continue => ("continue", None),
        HoldOutcome::TimedOut => ("timed_out", None),
        HoldOutcome::Stop => ("stop", None),
        HoldOutcome::Rerun { target } => ("rerun", target.clone()),
    };
    emit(
        &computer.id,
        "block_resolved",
        json!({
            "run_id": run_id,
            "computer_id": computer.id,
            "domain": det.domain,
            "kind": det.kind.as_str(),
            "action": action,
            "rerun_target": rerun_target,
        }),
    );
    Ok(outcome)
}

/// The model's result for a run the person ended at a wall.
pub fn ended_result(outcome: &HoldOutcome, det: &Detection) -> Option<ToolsetResult> {
    let place = det.domain.as_deref().unwrap_or("this site");
    let (code, message) = match outcome {
        HoldOutcome::Stop => ("run_stopped", format!("The person stopped this run at the {} page on {place}. Stop now and tell them where you got to; don't retry.", det.kind.as_str())),
        HoldOutcome::Rerun { .. } => (
            "rerun_elsewhere",
            format!("The person is re-running this task on their Mac because of the {} page on {place}. This run ends here: stop now and don't retry.", det.kind.as_str()),
        ),
        HoldOutcome::Continue | HoldOutcome::TimedOut => return None,
    };
    Some(ToolsetResult { is_error: true, content: vec![text(message)], browser_state: None, screen: screen_info(None), error: Some(code.to_string()) })
}

/// The note added when the run goes on after a hold.
pub fn continue_note(outcome: &HoldOutcome, det: &Detection) -> Option<Value> {
    let place = det.domain.as_deref().unwrap_or("this site");
    match outcome {
        HoldOutcome::Continue => Some(text(format!("The person dealt with the {} page on {place} and said continue. The page above is the fresh read.", det.kind.as_str()))),
        HoldOutcome::TimedOut => Some(text(format!(
            "This looks like a {} page on {place} and nobody answered. Don't type into it or retry it in a loop: call request_human so a person can step in.",
            det.kind.as_str()
        ))),
        _ => None,
    }
}
// ---------------------------------------------------------------------------
// Push (aci_approval, aci_human, aci_blocked).
// ---------------------------------------------------------------------------

/// The cloud-api receiver's path for this runtime (`routes/aci_push.rs`).
pub fn push_path(runtime_id: &str) -> String {
    format!("/push/notify/{runtime_id}")
}

/// Send one push for a run event to the owner's devices, in the
/// background: `POST {cloud}/push/notify/{runtime_id}` on cloud-api,
/// relay-signed like `runtime_events` (cloud-api fans it out to web push,
/// APNs and FCM). Sent only when this runtime is paired to `owner`, so it
/// never reaches another account. `data` carries `kind` and the fields the
/// receiver allowlists; a credential goes by name + domain only, never its
/// value. A failure is logged and dropped: the stream event is the source
/// of truth and every surface also reads it.
pub fn push_to_owner(owner: &str, data: Value) {
    let secret = crate::relay_auth::process_secret();
    let token = crate::relay_auth::RelaySecret::device_token(secret.as_ref());
    let paired = crate::relay_auth::RelaySecret::paired_owner(secret.as_ref());
    let runtime_id = crate::relay_auth::RelaySecret::runtime_id(secret.as_ref());
    let kind = data.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    let (Some(token), Some(paired), Some(runtime_id)) = (token, paired, runtime_id) else {
        tracing::debug!(%kind, "aci push skipped: this runtime isn't paired");
        return;
    };
    if paired != owner {
        tracing::debug!(%kind, "aci push skipped: the runtime is paired to another account");
        return;
    }
    let bot = data.get("run_id").and_then(Value::as_str).and_then(|r| crate::computer_toolset::run_actor(r).name);
    let Ok(body) = serde_json::to_vec(&push_payload(data, bot.as_deref())) else { return };
    let path = push_path(&runtime_id);
    let mut headers: Vec<(String, String)> =
        crate::relay_auth::signed_headers(&token, &paired, "POST", &path, &body).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    headers.push((crate::relay_auth::RUNTIME_ID_HEADER.to_string(), runtime_id));
    let url = format!("{}{path}", crate::phone_sync::cloud_base());
    tokio::spawn(async move {
        let mut req = crate::net_client::client().post(url).timeout(Duration::from_secs(20)).header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(k, v);
        }
        match req.body(body).send().await {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::debug!(%kind, status = %r.status(), "aci push refused"),
            Err(e) => tracing::debug!(%kind, error = %e, "aci push failed"),
        }
    });
}

/// The receiver's body: `{ data, bot_name? }`. `data` is one of
/// - aci_approval: `{kind, run_id, computer_id, approval_id, approval_kind, credential?, message}`
/// - aci_human: `{kind, run_id, computer_id, reason, secret}`
/// - aci_blocked: `{kind, run_id, computer_id, domain, block_kind, rerun_targets}` (computer ids)
pub fn push_payload(data: Value, bot_name: Option<&str>) -> Value {
    let mut body = json!({ "data": data });
    if let Some(bot) = bot_name.map(str::trim).filter(|b| !b.is_empty()) {
        body["bot_name"] = json!(bot);
    }
    body
}

// ---------------------------------------------------------------------------
// rerun_of: a run started again on another computer.
// ---------------------------------------------------------------------------

static RERUN_LINKS: Lazy<Mutex<HashMap<String, (String, Instant)>>> = Lazy::new(|| Mutex::new(HashMap::new()));
const RERUN_LINK_TTL: Duration = Duration::from_secs(6 * 3600);

/// A run id as the wire may carry it: trimmed, at most 128 safe characters.
pub fn clean_run_id(id: Option<&str>) -> Option<String> {
    let id = id?.trim();
    (!id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))).then(|| id.to_string())
}

/// Record that `run_id` re-runs `rerun_of` (first one wins for a run).
pub fn remember_rerun_of(run_id: &str, rerun_of: &str) {
    if run_id == rerun_of {
        return;
    }
    let mut links = RERUN_LINKS.lock().unwrap_or_else(|p| p.into_inner());
    links.retain(|_, (_, at)| at.elapsed() < RERUN_LINK_TTL);
    links.entry(run_id.to_string()).or_insert_with(|| (rerun_of.to_string(), Instant::now()));
}

/// The run this run re-runs, if any.
pub fn rerun_of(run_id: &str) -> Option<String> {
    RERUN_LINKS.lock().unwrap_or_else(|p| p.into_inner()).get(run_id).map(|(r, _)| r.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str, name: &str) -> Value {
        json!({ "role": role, "name": name })
    }

    #[test]
    fn detects_cloudflare_challenge_as_block_page() {
        let reply = json!({
            "url": "https://www.shop.example.com/cart",
            "elements": [el("heading", "Just a moment..."), el("text", "Checking if the site connection is secure"), el("text", "Performance & security by Cloudflare"), el("text", "Cloudflare Ray ID: 8c1f")],
        });
        let d = detect(&reply).expect("a Cloudflare challenge is a wall");
        assert_eq!(d.kind, BlockKind::BlockPage);
        assert_eq!(d.domain.as_deref(), Some("shop.example.com"));
    }

    #[test]
    fn detects_akamai_and_perimeterx_pages() {
        let akamai = json!({ "elements": [el("heading", "Access Denied"), el("text", "You don't have permission to access \"http://www.store.com/\" on this server."), el("text", "Reference #18.4f; https://errors.edgesuite.net/18.4f")] });
        assert_eq!(detect(&akamai).map(|d| d.kind), Some(BlockKind::BlockPage));
        let px = detect_text("Access to this page has been denied. Press & Hold to confirm you are a human (and not a bot). PerimeterX", Some("https://tickets.example.org/e/1"));
        assert_eq!(px.as_ref().map(|d| d.kind), Some(BlockKind::BlockPage));
        assert_eq!(px.and_then(|d| d.domain).as_deref(), Some("tickets.example.org"));
    }

    #[test]
    fn detects_captcha_frame() {
        let reply = json!({ "elements": [el("group", "reCAPTCHA"), el("checkbox", "I’m not a robot"), el("link", "Privacy - Terms")] });
        assert_eq!(detect(&reply).map(|d| d.kind), Some(BlockKind::Captcha));
    }

    #[test]
    fn detects_rate_limit_page() {
        let d = detect_text("429 Too Many Requests\nYou have sent too many requests in a given amount of time.", Some("https://api.example.net/x")).unwrap();
        assert_eq!((d.kind, d.domain.as_deref()), (BlockKind::RateLimited, Some("api.example.net")));
    }

    #[test]
    fn detects_two_factor_prompt() {
        let reply = json!({ "elements": [el("heading", "2-Step Verification"), el("text", "Enter the 6-digit code from your authenticator app"), el("textfield", "Enter code"), el("button", "Next")] });
        assert_eq!(detect(&reply).map(|d| d.kind), Some(BlockKind::TwoFactor));
    }

    #[test]
    fn detects_login_wall_by_secure_field_not_by_recaptcha_footer() {
        // Many sign-in forms carry a "protected by reCAPTCHA" line; that is a
        // login wall, not a captcha.
        let reply = json!({
            "document_url": "https://accounts.example.com/signin",
            "elements": [el("heading", "Sign in"), el("textfield", "Email"), json!({ "role": "AXSecureTextField", "name": "Password" }), el("button", "Log in"), el("text", "This site is protected by reCAPTCHA and the Google Privacy Policy.")],
        });
        let d = detect(&reply).unwrap();
        assert_eq!((d.kind, d.domain.as_deref()), (BlockKind::Login, Some("accounts.example.com")));
        let secure_flag = json!({ "elements": [el("button", "Sign in"), json!({ "role": "textfield", "name": "", "secure": true })] });
        assert_eq!(detect(&secure_flag).map(|d| d.kind), Some(BlockKind::Login));
    }

    #[test]
    fn ordinary_pages_are_not_walls() {
        // A store page with a "Sign in" link and no password field.
        let store = json!({ "url": "https://shop.example.com/", "elements": [el("link", "Sign in"), el("heading", "New arrivals"), el("button", "Add to cart"), el("text", "Free shipping over $50")] });
        assert_eq!(detect(&store), None);
        // A long article that only mentions the words.
        let article = format!("How rate limits and access denied errors work.\n{}", "Our API returns clear errors and you can sign in with a password. ".repeat(120));
        assert_eq!(detect_text(&article, Some("https://docs.example.com/limits")), None);
        // An empty tree and a non-text reply.
        assert_eq!(detect(&json!({ "elements": [] })), None);
        assert_eq!(detect(&json!({ "version": 3, "count": 0 })), None);
        // A checkout form's card "security code" is not a two-factor prompt.
        let checkout = json!({ "elements": [el("textfield", "Card number"), el("textfield", "Security code"), el("button", "Pay now")] });
        assert_eq!(detect(&checkout), None);
    }

    #[test]
    fn a_continue_stops_the_same_wall_holding_the_run_again() {
        let det = Detection { kind: BlockKind::Login, domain: Some("example.com".into()), signal: "sign in" };
        let run = format!("run-{}", uuid::Uuid::new_v4().simple());
        assert!(should_hold(&run, &det));
        mark_continued(&run, &det);
        assert!(!should_hold(&run, &det), "same run, same wall: no second hold");
        assert!(should_hold(&run, &Detection { kind: BlockKind::Captcha, ..det.clone() }), "a different wall still holds");
        assert!(should_hold("another-run", &det), "another run still holds");
        forget_run(&run);
        assert!(should_hold(&run, &det));
    }

    #[test]
    fn resolve_needs_a_pending_hold_and_a_listed_target() {
        let cid = format!("cmp-{}", uuid::Uuid::new_v4().simple());
        assert_eq!(resolve(&cid, BlockAction::Continue), Err(ResolveError::NoPending));
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        BLOCK_GATES.lock().unwrap().insert(
            cid.clone(),
            BlockGate { tx, run_id: "r1".into(), domain: Some("example.com".into()), kind: BlockKind::BlockPage, rerun_targets: vec![RerunTarget { computer_id: "mac-1".into(), name: "Mac".into() }], since: Instant::now() },
        );
        assert_eq!(resolve(&cid, BlockAction::Rerun { target: Some("mac-9".into()) }), Err(ResolveError::UnknownTarget));
        let ok = resolve(&cid, BlockAction::Rerun { target: Some("mac-1".into()) }).unwrap();
        assert_eq!((ok.run_id.as_str(), ok.kind), ("r1", BlockKind::BlockPage));
        assert_eq!(rx.try_recv().unwrap(), BlockAction::Rerun { target: Some("mac-1".into()) });
        assert_eq!(resolve(&cid, BlockAction::Stop), Err(ResolveError::NoPending), "answered once");
    }
}
