//! Contract v2 (`allternit.computer.v2`): the ten structured members every
//! model family gets as function tools — `read_ui`, `act`, `run_batch`,
//! `verify` through the Allternit Driver sidecar, `run_subtask` (the bounded
//! decision loop, `computer_subtask`, with its replay cache), `run_parallel`
//! + Best-of-N (`computer_parallel`), the saved skills a subtask teaches
//! (`run_skill`, `skills`), `request_human` (lease
//! pause/resume around a human window) and `use_credential` (vault-backed
//! typing where the value never enters the model context or the logs).
//!
//! The 17 pixel members stay in `computer_toolset::dispatch`; this module is
//! the v2 path out of `execute`. Approvals and the audit row for these
//! members are the existing lease → policy → approval → audit pipeline in
//! `computer_toolset::execute` (act's text-entering ops upgrade to a human
//! approval on non-sandbox targets, the same rule as `type`/`key`;
//! use_credential asks the person on every target, cloud guests included,
//! unless they chose "Allow for this run" for the same credential + domain).
//! A `read_ui` that lands on a login, 2FA, captcha, bot-block or rate-limit
//! page holds the run for the person (`computer_block`). The two access members (`resolve_access`,
//! `request_access`) also run through here, into `computer_access`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};

use crate::auth::AuthUser;
use crate::computer_control_lease as lease;
use crate::computer_routes::ComputerResponse;
use crate::computer_toolset::{
    agent_holder, guest_exec, screen_info, text, windows_exec, CoordinateSpace, Fail, Frame, Mapping, ScreenInfo,
    Target, Toolset, ToolsetResult,
};
use crate::this_device_input as td;
use crate::AppState;

/// The driver-backed structured members of allternit.computer.v2.
pub const V2_MEMBERS: [&str; 10] =
    ["read_ui", "act", "run_batch", "verify", "request_human", "use_credential", "run_subtask", "run_parallel", "run_skill", "skills"];

/// Is `member` one of the v2 structured members?
pub fn is_v2_member(member: &str) -> bool {
    V2_MEMBERS.contains(&member)
}

/// act / run_batch ops that enter text. On non-sandbox targets they need the
/// person's approval, the same rule as the `type`/`key` members (contract
/// `confirm: non_sandbox`). Clicks, focus and menus stay approval-free, like
/// the pixel click members.
const TEXT_ENTRY_OPS: [&str; 3] = ["set_value", "select", "press"];

/// Whether a v2 call needs a human approval grant on top of the member's own
/// contract rule. `sandboxed` mirrors `Target::sandboxed` (cloud/bot guests
/// and gateway browsers run without per-action approval).
pub fn v2_member_needs_approval(member: &str, input: &Value, sandboxed: bool) -> bool {
    if sandboxed {
        return false;
    }
    let text_op = |v: Option<&Value>| v.and_then(Value::as_str).is_some_and(|o| TEXT_ENTRY_OPS.contains(&o));
    match member {
        "act" => text_op(input.get("op")),
        "run_batch" => input
            .get("steps")
            .and_then(Value::as_array)
            .is_some_and(|steps| steps.iter().any(|s| text_op(s.get("act").and_then(|a| a.get("op"))))),
        _ => false,
    }
}

/// How long one driver call may take (reads are tens of ms; a batch with
/// wait_for/expect conditions can wait seconds per step).
pub(crate) fn driver_timeout(member: &str, input: &Value) -> Duration {
    match member {
        "run_batch" => {
            let steps = input.get("steps").and_then(Value::as_array);
            let count = steps.map_or(0, |s| s.len() as u64);
            let waits = steps.map_or(0, |s| {
                s.iter().filter_map(|st| st.get("timeout_ms").and_then(Value::as_u64)).sum::<u64>()
            });
            Duration::from_secs((30 + waits / 1000 + count * 2).clamp(60, 300))
        }
        // The vision fallback: a first vision read may wait for the worker,
        // and grounding a `target` runs the local model (with a zoom pass).
        "read_ui" if input.get("target").and_then(Value::as_str).is_some_and(|t| !t.is_empty()) => Duration::from_secs(300),
        "read_ui" | "act" => Duration::from_secs(90),
        _ => Duration::from_secs(30),
    }
}

/// The screen size reported by the driver, without a screenshot.
async fn driver_screen_size() -> Option<Frame> {
    let endpoint = td::DriverEndpoint::resolve()?;
    let reply = td::driver_rpc(&endpoint, "pixel_get_screen_size", json!({}), Duration::from_secs(5)).await.ok()?;
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let (w, h) = match (num(&reply, "width"), num(&reply, "height")) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let s = reply.get("text").and_then(Value::as_str)?;
            let (w, h) = s.split_once(['x', ','])?;
            (w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?)
        }
    };
    (w > 0.0 && h > 0.0).then_some(Frame { width: w.round() as u32, height: h.round() as u32 })
}

/// The result's screen block for structured members on this device: the real
/// screen size from the driver (no capture) and the frame a screenshot would
/// use.
async fn screen_block() -> ScreenInfo {
    match driver_screen_size().await {
        Some(f) => screen_info(Some(&Mapping::new(
            f,
            None,
            CoordinateSpace::Pixels,
            &crate::computer_toolset::contract(Toolset::Computer).model_frame,
        ))),
        None => screen_info(None),
    }
}

/// The guest variant of `screen_block`: the screen size comes from the
/// guest's own driver, over the exec channel.
async fn guest_screen_block(target: &Target) -> ScreenInfo {
    match crate::guest_driver::guest_screen_size(target).await {
        Some(f) => screen_info(Some(&Mapping::new(
            f,
            None,
            CoordinateSpace::Pixels,
            &crate::computer_toolset::contract(Toolset::Computer).model_frame,
        ))),
        None => screen_info(None),
    }
}

/// One sidecar op for a structured member. The params pass nearly verbatim;
/// the driver answers JSON (element trees, batch results, verify rows).
pub(crate) async fn driver_op(member: &str, params: &Value) -> Result<Value, Fail> {
    let endpoint = td::DriverEndpoint::resolve().ok_or(
        "The Allternit Driver sidecar isn't running on this computer. Open Allternit Desktop (or update it) and try again.",
    )?;
    td::driver_rpc(&endpoint, member, params.clone(), driver_timeout(member, params))
        .await
        .map_err(|e| Fail::from(String::from(e)))
}

/// One driver call for read_ui / act / run_batch / verify: the local sidecar
/// on this device, the in-guest driver on a guest.
async fn structured_call(target: &Target, member: &str, input: &Value) -> Result<(Value, ScreenInfo), Fail> {
    match target {
        Target::Guest { .. } => {
            let reply = crate::guest_driver::guest_driver_op(target, member, input, driver_timeout(member, input)).await?;
            Ok((reply, guest_screen_block(target).await))
        }
        _ => Ok((driver_op(member, input).await?, screen_block().await)),
    }
}

/// Input key for an authority kernel approval token on a v2 member (not part of the hashed action).
pub const KERNEL_TOKEN_KEY: &str = "kernel_approval_token";

fn decision_needs_token(refusal: &Value) -> bool {
    refusal.get("code").and_then(Value::as_str) == Some(crate::authority::check::APPROVAL_REQUIRED)
}

/// Run one v2 member. Called from `computer_toolset::execute` after the
/// lease, policy, approval and audit steps, so every member keeps the same
/// gate. Returns the toolset result (text content; no screenshots).
pub async fn execute_v2(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    member: &str,
    input: &Value,
    run_id: &str,
) -> Result<ToolsetResult, Fail> {
    // Authority kernel check (plan step 9) before the member runs. The token rides in the input
    // as `kernel_approval_token` and is taken out before hashing and before the driver sees it.
    let mut stripped = input.clone();
    let token = stripped
        .as_object_mut()
        .and_then(|o| o.remove(KERNEL_TOKEN_KEY))
        .and_then(|v| v.as_str().map(str::to_string));
    let input = &stripped;
    {
        use crate::authority::check::{check, Action, CheckCtx, Source};
        let risk = crate::computer_toolset::contract(Toolset::Computer)
            .member(member)
            .or_else(|| crate::computer_toolset::contract(Toolset::Browser).member(member))
            .map(|m| m.risk.as_str());
        // Same chain scope as the kernel routes (`AuthUser::tenant_id`).
        let ctx = CheckCtx::new(state.db.clone(), &user.user_id, user.tenant_id.as_deref())
            .with_run_id(Some(run_id.to_string()))
            .with_token(token);
        let mut args = input.clone();
        if let Some(o) = args.as_object_mut() {
            o.insert("computer_id".into(), json!(computer.id));
        }
        let decision = check(&ctx, &Action::new(Source::Computer, member, &args).with_risk(risk)).await;
        if let Some(refusal) = decision.refusal() {
            let code = if decision_needs_token(&refusal) { "approval_required" } else { "kernel_denied" };
            return Err(Fail { message: refusal.to_string(), browser_state: None, code: Some(code) });
        }
        // secret.use (plan step 10): record the (credential, target) pair once a person approved
        // its first use, receipt every later use.
        if member == "use_credential" {
            let receipt = match &decision { crate::authority::check::Decision::Allow { receipt_id } => receipt_id.clone(), _ => None };
            if let Err(refusal) = crate::authority::secrets::bind_computer_credential(&ctx, &args, receipt).await {
                return Err(Fail { message: refusal, browser_state: None, code: Some("approval_required") });
            }
        }
    }
    match member {
        "read_ui" | "act" | "run_batch" | "verify" => {
            // D1b: this-device reaches the local sidecar; a guest reaches the
            // driver inside the guest image through the guest exec channel
            // (guest_driver). Same lease/policy/safety/audit gate either way —
            // `execute` ran it before dispatching here.
            // A browser run on this device reads its own Safari window when
            // read_ui names none (computer_safari_window).
            let scoped = match target {
                Target::ThisDevice => crate::computer_safari_window::scope_read_ui(member, input, crate::computer_safari_window::window_of(run_id).as_ref()),
                _ => None,
            };
            let input = scoped.as_ref().unwrap_or(input);
            let (mut reply, mut screen) = structured_call(target, member, input).await?;
            // A wall the agent can't pass (login, 2FA, captcha, bot block,
            // rate limit) holds the run until the person answers
            // (computer_block). "continue" re-reads the page they fixed.
            let mut note = None;
            if member == "read_ui" {
                if let Some(det) = crate::computer_block::detect(&reply).filter(|d| crate::computer_block::should_hold(run_id, d)) {
                    let outcome = crate::computer_block::hold(state, user, computer, target, run_id, &det).await?;
                    if let Some(ended) = crate::computer_block::ended_result(&outcome, &det) {
                        // Stopped or moved to the Mac: the run is over here.
                        crate::computer_agent_display::end_run(run_id);
                        return Ok(ended);
                    }
                    if outcome == crate::computer_block::HoldOutcome::Continue {
                        (reply, screen) = structured_call(target, member, input).await?;
                    }
                    note = crate::computer_block::continue_note(&outcome, &det);
                }
            }
            let mut content = vec![text(serde_json::to_string(&reply).unwrap_or_else(|_| "{}".into()))];
            content.extend(note);
            Ok(ToolsetResult { is_error: false, content, browser_state: None, screen, error: None })
        }
        "request_human" if input.get("secret").is_some_and(Value::is_object) => {
            request_secret(user, computer, target, input, run_id).await
        }
        "request_human" => request_human(state, user, computer, target, input, run_id).await,
        "use_credential" => use_credential(user, target, input).await,
        "skills" => crate::computer_subtask::skills(state, user, input),
        // Access tiers: resolve_access looks apps up; request_access records
        // session grants after the person's approval (computer_access).
        "resolve_access" | "request_access" => crate::computer_access::execute(state, user, computer, target, member, input, run_id).await,
        other => Err(Fail::from(format!("{other} isn't a v2 member"))),
    }
}

// ---------------------------------------------------------------------------
// request_human: pause the lease, open the human window, resume on done.
// ---------------------------------------------------------------------------

/// One pending human window per computer. `done` fires when the person
/// signals done from any surface (`POST /computers/:id/human-done`).
struct HumanGate {
    done: Arc<tokio::sync::Notify>,
    reason: String,
    since: std::time::Instant,
}

static HUMAN_GATES: Lazy<Mutex<HashMap<String, HumanGate>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Open (or refresh) the human window's latch for a computer.
fn human_gate_open(computer_id: &str, reason: &str) -> Arc<tokio::sync::Notify> {
    let mut gates = HUMAN_GATES.lock().unwrap_or_else(|p| p.into_inner());
    let gate = gates.entry(computer_id.to_string()).or_insert_with(|| HumanGate {
        done: Arc::new(tokio::sync::Notify::new()),
        reason: reason.to_string(),
        since: std::time::Instant::now(),
    });
    gate.reason = reason.to_string();
    gate.done.clone()
}

/// Signal that the human is done (from the HTTP route). Returns false when
/// no window is open.
pub fn human_gate_done(computer_id: &str) -> bool {
    let done = HUMAN_GATES.lock().unwrap_or_else(|p| p.into_inner()).get(computer_id).map(|g| g.done.clone());
    match done {
        Some(done) => {
            done.notify_waiters();
            HUMAN_GATES.lock().unwrap_or_else(|p| p.into_inner()).remove(computer_id);
            true
        }
        None => false,
    }
}

/// Is a human window open on this computer (status for endpoints/events)?
pub fn human_gate_pending(computer_id: &str) -> Option<(String, u64)> {
    HUMAN_GATES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(computer_id)
        .map(|g| (g.reason.clone(), g.since.elapsed().as_secs()))
}

fn emit_human_event(computer_id: &str, kind: &str, reason: &str, run_id: &str, extra: Map<String, Value>) {
    let mut data = Map::new();
    data.insert("computer_id".into(), json!(computer_id));
    data.insert("reason".into(), json!(reason));
    data.insert("run_id".into(), json!(run_id));
    data.insert("live_view".into(), json!(true));
    data.insert("input_enabled".into(), json!(true));
    for (k, v) in extra {
        data.insert(k, v);
    }
    let event = json!({
        "type": format!("computer.{kind}"),
        "ts": chrono::Utc::now().to_rfc3339(),
        "data": data,
    });
    let _ = crate::computer_toolset::ACTION_EVENTS.send((computer_id.to_string(), event));
}

/// Guests: release the agent's lease so the person can drive during a
/// window (request_human, a block-page hold).
pub(crate) async fn release_for_person(state: &Arc<AppState>, computer_id: &str, caller: &lease::Holder) -> Result<(), Fail> {
    let (db, cid, caller) = (state.db.clone(), computer_id.to_string(), caller.clone());
    let released = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        lease::release(&conn, &cid, &caller, chrono::Utc::now().timestamp())
    })
    .await;
    match released {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(Fail::from(format!("couldn't release control for the human window: {e}"))),
        Err(_) => Err(Fail::from("internal error releasing control")),
    }
}

/// Guests: take the lease back after a person's window. A person who is
/// still driving keeps control (agents never preempt a person): after 15 s
/// this answers `Ok(Some(holder label))` and the agent stays off the input.
pub(crate) async fn take_back_after_person(state: &Arc<AppState>, computer_id: &str, caller: &lease::Holder) -> Result<Option<Option<String>>, Fail> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let (db, cid, caller) = (state.db.clone(), computer_id.to_string(), caller.clone());
        let took = tokio::task::spawn_blocking(move || {
            let conn = db.connect().map_err(|e| lease::TakeError::Db(e.to_string()))?;
            lease::take(&conn, &cid, &caller, chrono::Utc::now().timestamp())
        })
        .await;
        match took {
            Ok(Ok(_)) => return Ok(None),
            Ok(Err(lease::TakeError::Held(held))) => {
                if std::time::Instant::now() >= deadline {
                    return Ok(Some(held.holder.label));
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok(Err(lease::TakeError::Db(e))) => return Err(Fail::from(format!("couldn't take control back after the human window: {e}"))),
            Err(_) => return Err(Fail::from("internal error taking control back")),
        }
    }
}

/// request_human: hand control to the person, wait for their done signal
/// (or the timeout), then take the lease back. On this-device the person
/// already is the controller, so the wait is purely the latch; on guests the
/// agent's lease is released for the window, so the UI shows the computer
/// free and the person can take over without a fight.
async fn request_human(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    input: &Value,
    run_id: &str,
) -> Result<ToolsetResult, Fail> {
    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("The agent needs a person to step in.")
        .to_string();
    let timeout_ms = input
        .get("timeout_ms")
        .and_then(Value::as_f64)
        .unwrap_or(300_000.0)
        .clamp(1_000.0, 1_800_000.0) as u64;
    hold_for_person(state, user, computer, target, run_id, &reason, Map::new(), timeout_ms).await
}

/// The request_human window itself: `computer.human_requested` (with
/// `extra` merged into its data, e.g. a setup card's `steps`), the
/// `aci_human` push, the wait for `POST /computers/:id/human-done` or the
/// timeout, then `computer.human_resumed`. Guests give the lease to the
/// person for the window. Also used by the Safari agent-window setup hold
/// (computer_safari_window).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn hold_for_person(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    run_id: &str,
    reason: &str,
    extra: Map<String, Value>,
    timeout_ms: u64,
) -> Result<ToolsetResult, Fail> {
    let reason = reason.to_string();
    let is_this_device = matches!(target, Target::ThisDevice);
    let caller = agent_holder(user, Some(run_id));

    // Guests: release the agent's lease for the human window.
    if !is_this_device {
        release_for_person(state, &computer.id, &caller).await?;
    }

    let done = human_gate_open(&computer.id, &reason);
    emit_human_event(&computer.id, "human_requested", &reason, run_id, {
        let mut m = extra;
        m.insert("timeout_ms".into(), json!(timeout_ms));
        m
    });
    crate::computer_block::push_to_owner(
        &user.user_id,
        json!({ "kind": "aci_human", "run_id": run_id, "computer_id": computer.id, "reason": reason, "secret": false }),
    );

    let timed_out = tokio::time::timeout(Duration::from_millis(timeout_ms), done.notified()).await.is_err();
    HUMAN_GATES.lock().unwrap_or_else(|p| p.into_inner()).remove(&computer.id);

    // Guests: take the lease back. A person who is still driving keeps
    // control (agents never preempt a person); the next action then answers
    // 423 and the model can request_human again.
    if !is_this_device {
        if let Some(held_by) = take_back_after_person(state, &computer.id, &caller).await? {
            emit_human_event(&computer.id, "human_resumed", &reason, run_id, {
                let mut m = Map::new();
                m.insert("resumed".into(), json!(false));
                m.insert("held_by".into(), json!(held_by));
                m
            });
            return Ok(human_result(&reason, timed_out, false));
        }
    }

    emit_human_event(&computer.id, "human_resumed", &reason, run_id, {
        let mut m = Map::new();
        m.insert("resumed".into(), json!(true));
        m.insert("timed_out".into(), json!(timed_out));
        m
    });
    Ok(human_result(&reason, timed_out, true))
}

// ---------------------------------------------------------------------------
// request_human { secret }: a masked box for a one-off value.
// ---------------------------------------------------------------------------

/// How long the masked box waits for the person.
const SECRET_TTL: Duration = Duration::from_secs(600);

/// What the person sent through `POST /computers/:id/secret`. Not `Debug`:
/// the value must never reach a log line.
pub struct SecretReply {
    /// `None` = the person cancelled.
    pub text: Option<String>,
    pub save_as: Option<String>,
    /// The typing outcome goes back to the route so the box can show an error.
    pub ack: tokio::sync::oneshot::Sender<Result<(), String>>,
}

struct SecretGate {
    tx: tokio::sync::oneshot::Sender<SecretReply>,
    label: String,
    since: std::time::Instant,
}

/// At most one pending secret request per computer.
static SECRET_GATES: Lazy<Mutex<HashMap<String, SecretGate>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Why a secret could not be delivered.
pub enum SecretError {
    /// No request is pending (never asked, answered, cancelled or expired): 409.
    NoPending,
    /// Typing it into the computer failed.
    Failed(String),
}

/// Is a secret request pending on this computer? (label, seconds waiting)
pub fn secret_pending(computer_id: &str) -> Option<(String, u64)> {
    SECRET_GATES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(computer_id)
        .map(|g| (g.label.clone(), g.since.elapsed().as_secs()))
}

/// Hand the person's value (or their cancel) to the waiting `request_human`
/// call, which types it and answers. The value moves through a channel; it is
/// never stored, logged or put in an event.
pub async fn secret_supply(
    computer_id: &str,
    text: Option<String>,
    save_as: Option<String>,
) -> Result<(), SecretError> {
    let gate = SECRET_GATES.lock().unwrap_or_else(|p| p.into_inner()).remove(computer_id);
    let Some(gate) = gate else { return Err(SecretError::NoPending) };
    let (ack, ack_rx) = tokio::sync::oneshot::channel();
    if gate.tx.send(SecretReply { text, save_as, ack }).is_err() {
        return Err(SecretError::NoPending);
    }
    match tokio::time::timeout(Duration::from_secs(45), ack_rx).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(e))) => Err(SecretError::Failed(e)),
        _ => Err(SecretError::Failed("the computer didn't confirm the field was filled".into())),
    }
}

fn secret_result(message: String, is_error: bool) -> ToolsetResult {
    ToolsetResult {
        is_error,
        content: vec![text(message)],
        browser_state: None,
        screen: screen_info(None),
        error: None,
    }
}

/// request_human with `secret`: show the person a masked box, wait for the
/// value, type it into the focused field through the same path as
/// `use_credential`, and answer with only that the field was filled. The agent
/// keeps control of the computer the whole time; the value never enters the
/// model context, the event stream, the audit row or the transcript.
async fn request_secret(
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    input: &Value,
    run_id: &str,
) -> Result<ToolsetResult, Fail> {
    let spec = input.get("secret").cloned().unwrap_or(Value::Null);
    let label = spec
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| Fail::from("secret needs a label that says what to ask the person for"))?
        .chars()
        .take(120)
        .collect::<String>();
    let field = spec.get("field").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty()).map(|f| f.chars().take(80).collect::<String>());
    let domain = spec.get("domain").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()).map(str::to_string);
    let element_id = spec.get("element_id").and_then(Value::as_str).map(str::trim).filter(|e| !e.is_empty()).map(str::to_string);
    // Refuse before asking the person: on this device the value goes into
    // one element only, never into whatever window is in front.
    if matches!(target, Target::ThisDevice) && element_id.is_none() {
        return Err(Fail::from(
            "On this device the masked box writes the value straight into one field. Call read_ui, then request_human again with secret.element_id set to the field's id; if the field has no element id, call plain request_human so the person fills it themselves.",
        ));
    }
    secret_route(target, element_id.as_deref())?;

    let (tx, rx) = tokio::sync::oneshot::channel::<SecretReply>();
    {
        let mut gates = SECRET_GATES.lock().unwrap_or_else(|p| p.into_inner());
        if gates.contains_key(&computer.id) {
            return Err(Fail::from("a secret request is already waiting on this computer"));
        }
        gates.insert(computer.id.clone(), SecretGate { tx, label: label.clone(), since: std::time::Instant::now() });
    }
    // The label and field are what the model asked for; the value is not known yet.
    emit_human_event(&computer.id, "secret_requested", &label, run_id, {
        let mut m = Map::new();
        m.insert("label".into(), json!(label));
        m.insert("field".into(), json!(field));
        m.insert("domain".into(), json!(domain));
        m.insert("ttl_ms".into(), json!(SECRET_TTL.as_millis() as u64));
        m.insert("can_save".into(), json!(true));
        m.insert("input_enabled".into(), json!(false));
        m
    });
    crate::computer_block::push_to_owner(
        &user.user_id,
        json!({ "kind": "aci_human", "run_id": run_id, "computer_id": computer.id, "reason": label, "secret": true }),
    );

    let waited = tokio::time::timeout(SECRET_TTL, rx).await;
    SECRET_GATES.lock().unwrap_or_else(|p| p.into_inner()).remove(&computer.id);

    let reply = match waited {
        Ok(Ok(reply)) => reply,
        Ok(Err(_)) => return Ok(secret_result(format!("The request for '{label}' was withdrawn."), true)),
        Err(_) => {
            emit_human_event(&computer.id, "secret_cancelled", &label, run_id, {
                let mut m = Map::new();
                m.insert("expired".into(), json!(true));
                m
            });
            return Ok(secret_result(
                format!("The person didn't provide '{label}' within 10 minutes. The field is still empty."),
                true,
            ));
        }
    };

    let Some(value) = reply.text else {
        emit_human_event(&computer.id, "secret_cancelled", &label, run_id, {
            let mut m = Map::new();
            m.insert("expired".into(), json!(false));
            m
        });
        let _ = reply.ack.send(Ok(()));
        return Ok(secret_result(format!("The person declined to provide '{label}'. The field is still empty."), true));
    };

    // Screen later screenshots and observations for this value.
    crate::aci_credentials::remember_ephemeral_secret(&user.user_id, &value);

    let typed = type_secret(target, &value, element_id.as_deref()).await;
    let mut saved = false;
    let mut save_note: Option<String> = None;
    if typed.is_ok() {
        if let Some(name) = reply.save_as.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            let created = crate::aci_credentials::CREDENTIALS.create(
                &user.user_id,
                name,
                crate::aci_credentials::CredentialType::Env,
                &value,
                domain.as_deref(),
            );
            saved = created.is_ok();
            if !saved {
                save_note = Some(format!("It was typed, but couldn't be saved as '{name}' (name taken, invalid, or no vault key)."));
            }
        }
    }
    drop(value);

    match typed {
        Ok(()) => {
            emit_human_event(&computer.id, "secret_supplied", &label, run_id, {
                let mut m = Map::new();
                m.insert("saved".into(), json!(saved));
                m
            });
            let _ = reply.ack.send(match save_note {
                Some(note) => Err(note),
                None => Ok(()),
            });
            let into = element_id.as_deref().map_or_else(|| "the focused field".to_string(), |e| format!("element {e}"));
            Ok(secret_result(format!("The person provided '{label}' and it was written into {into}."), false))
        }
        Err(fail) => {
            let _ = reply.ack.send(Err("couldn't fill the field".to_string()));
            Err(fail)
        }
    }
}

fn human_result(reason: &str, timed_out: bool, resumed: bool) -> ToolsetResult {
    ToolsetResult {
        is_error: false,
        content: vec![text(format!(
            "Human window closed ({reason}). {}.{}",
            if timed_out { "Timed out" } else { "The person signaled done" },
            if resumed {
                " The session has control again."
            } else {
                " A person still holds control; your next action will ask them to hand it back."
            },
        ))],
        browser_state: None,
        screen: screen_info(None),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// use_credential: resolve from a backend, write into the element the model picked (this device) or the run's field (guests).
// The value never enters the model context, the logs, or the audit row;
// verification is by outcome (field filled, redacted audit row).
// ---------------------------------------------------------------------------

/// Resolve a credential for computer use: the plaintext plus the record's
/// type and its app/domain binding (when the record carries one).
struct ResolvedForUse {
    credential_type: crate::aci_credentials::CredentialType,
    value: String,
    bind: Option<String>,
}

/// Which credential backends answer on this machine (the credentials guide
/// and the schema endpoint report this). `bw`/`op` are team-vault CLIs.
pub fn credential_backends() -> Vec<(&'static str, bool)> {
    vec![
        ("vault", true),
        ("keychain", OS_KEYCHAIN_SUPPORTED),
        ("bitwarden", which_like("bw")),
        ("onepassword", which_like("op")),
    ]
}

/// The OS credential store backs source "keychain": the macOS Keychain or
/// the Windows Credential Manager. Elsewhere there is none.
const OS_KEYCHAIN_SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// What source "keychain" is called on this OS, for messages.
const OS_KEYCHAIN_NAME: &str = if cfg!(target_os = "windows") {
    "this computer's Windows Credential Manager"
} else {
    "this Mac's Keychain"
};

fn which_like(name: &str) -> bool {
    find_on_path(name).is_some()
}

/// Find a CLI on PATH. PATH is split with the OS's list separator (`:` on
/// Unix, `;` on Windows), not the directory separator; on Windows the
/// executable extensions are tried too (`bw` from npm is `bw.cmd`).
fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    find_in_path_list(&std::env::var_os("PATH")?, name, cfg!(windows))
}

fn find_in_path_list(paths: &std::ffi::OsStr, name: &str, windows: bool) -> Option<std::path::PathBuf> {
    let names: Vec<String> = if windows {
        vec![format!("{name}.exe"), format!("{name}.cmd"), format!("{name}.bat"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(paths)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// macOS Keychain backend: generic passwords under the `allternit.computer`
/// service, looked up by account name (`security find-generic-password`).
#[cfg(target_os = "macos")]
async fn keychain_secret(name: &str) -> Result<Option<String>, Fail> {
    let out = tokio::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", "allternit.computer", "-a", name, "-w"])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Fail::from(format!("couldn't reach the Keychain: {e}")))?;
    if !out.status.success() {
        return Ok(None); // No such item (or access denied): fall through.
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string()))
}

/// Windows Credential Manager backend: generic credentials whose target is
/// `allternit.computer/<name>` (add one with
/// `cmdkey /generic:allternit.computer/<name> /user:<name> /pass:<value>`).
/// Only "not found" falls through; any other failure is an error.
#[cfg(target_os = "windows")]
async fn keychain_secret(name: &str) -> Result<Option<String>, Fail> {
    let target = format!("allternit.computer/{name}");
    tokio::task::spawn_blocking(move || wincred_read(&target))
        .await
        .map_err(|e| Fail::from(format!("couldn't reach the Windows Credential Manager: {e}")))?
}

#[cfg(target_os = "windows")]
fn wincred_read(target: &str) -> Result<Option<String>, Fail> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};

    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and outlives the call; `cred` is an out-pointer.
    let ok = unsafe { CredReadW(wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
    if ok == 0 {
        // SAFETY: reads this thread's last-error value.
        let err = unsafe { GetLastError() };
        if err == ERROR_NOT_FOUND {
            return Ok(None);
        }
        return Err(Fail::from(format!("couldn't read the Windows Credential Manager (error {err})")));
    }
    // SAFETY: CredReadW succeeded, so `cred` points at a valid CREDENTIALW whose
    // blob is CredentialBlobSize bytes; it is freed exactly once below.
    let bytes = unsafe {
        let c = &*cred;
        if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        }
    };
    unsafe { CredFree(cred as *const std::ffi::c_void) };
    Ok(Some(decode_credential_blob(&bytes)))
}

/// Credential Manager blobs written by cmdkey and the Control Panel are
/// UTF-16LE; other tools store UTF-8. A NUL byte means UTF-16 (UTF-8 text
/// never has one); otherwise valid UTF-8 wins, then UTF-16.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_credential_blob(bytes: &[u8]) -> String {
    let utf16 = || {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16(&units).ok()
    };
    if bytes.contains(&0) {
        if let Some(s) = utf16() {
            return s.trim_end_matches('\0').to_string();
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => utf16().unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// No OS credential store here: an explicit `source: "keychain"` is an
/// error, never a silent "not found".
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn keychain_secret(_name: &str) -> Result<Option<String>, Fail> {
    Err(Fail::from(
        "source \"keychain\" isn't available on this OS (it uses the macOS Keychain or the Windows Credential Manager); use source \"vault\", \"bitwarden\" or \"onepassword\"",
    ))
}

/// Bitwarden CLI adapter (team vaults). Needs `bw` on PATH and an unlocked
/// session (`BW_SESSION`); the item is looked up by name.
async fn bitwarden_secret(name: &str) -> Result<Option<String>, Fail> {
    let Some(bw) = find_on_path("bw") else {
        return Err(Fail::from(
            "the Bitwarden CLI (bw) isn't installed on this computer; install it or use source \"vault\" or \"keychain\"",
        ));
    };
    let out = tokio::process::Command::new(bw)
        .args(["get", "password", name])
        .env("BW_SESSION", std::env::var("BW_SESSION").unwrap_or_default())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Fail::from(format!("couldn't run the Bitwarden CLI: {e}")))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string()))
}

/// 1Password CLI adapter (team vaults). Needs `op` on PATH and a signed-in
/// account; reads the item's password field.
async fn onepassword_secret(name: &str) -> Result<Option<String>, Fail> {
    let Some(op) = find_on_path("op") else {
        return Err(Fail::from(
            "the 1Password CLI (op) isn't installed on this computer; install it or use source \"vault\" or \"keychain\"",
        ));
    };
    let out = tokio::process::Command::new(op)
        .args(["item", "get", name, "--fields", "label=password", "--reveal"])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Fail::from(format!("couldn't run the 1Password CLI: {e}")))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string()))
}

/// Resolve the credential from the requested source (the sealed vault, then
/// the OS credential store — macOS Keychain / Windows Credential Manager — by default). Never logs the value.
async fn resolve_credential(user: &AuthUser, input: &Value) -> Result<ResolvedForUse, Fail> {
    let name = input.get("name").and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() {
        return Err(Fail::from("use_credential needs the credential's name"));
    }
    let source = input.get("source").and_then(Value::as_str);

    if source.is_none() || source == Some("vault") {
        if let Some((credential_type, value, bind)) =
            crate::aci_credentials::CREDENTIALS.open_for_computer_use(&user.user_id, name)
        {
            return Ok(ResolvedForUse { credential_type, value, bind });
        }
    }
    if (source.is_none() && OS_KEYCHAIN_SUPPORTED) || source == Some("keychain") {
        if let Some(value) = keychain_secret(name).await? {
            return Ok(ResolvedForUse { credential_type: crate::aci_credentials::CredentialType::Env, value, bind: None });
        }
    }
    if source == Some("bitwarden") {
        if let Some(value) = bitwarden_secret(name).await? {
            return Ok(ResolvedForUse { credential_type: crate::aci_credentials::CredentialType::Env, value, bind: None });
        }
    }
    if source == Some("onepassword") {
        if let Some(value) = onepassword_secret(name).await? {
            return Ok(ResolvedForUse { credential_type: crate::aci_credentials::CredentialType::Env, value, bind: None });
        }
    }
    let where_ = match source {
        Some(s) => format!("source '{s}'"),
        None if OS_KEYCHAIN_SUPPORTED => format!("the vault or {OS_KEYCHAIN_NAME}"),
        None => "the vault".to_string(),
    };
    let os_hint = if cfg!(target_os = "windows") {
        " or `cmdkey /generic:allternit.computer/<name> /user:<name> /pass:<value>`"
    } else if cfg!(target_os = "macos") {
        " or `security add-generic-password -s allternit.computer -a <name> -w`"
    } else {
        ""
    };
    Err(Fail::from(format!(
        "no credential named '{name}' in {where_}. Add one with POST /api/aci/credentials{os_hint}."
    )))
}

/// The refusal on this device when no element id was given. The model gets
/// the way forward in the same message.
pub(crate) const SECRET_NEEDS_ELEMENT: &str = "On this device a secret is written straight into one field, never typed into whatever window is in front. Call read_ui, find the password or text field, and call again with element_id set to that field's id. If the field has no element id, call request_human with secret { label, element_id } so the person types it into the masked box, or plain request_human so they fill the field themselves.";

/// How a secret reaches the computer. Never "type into the front window" on
/// this device: the Safari spike (2026-11-26) put a dummy secret into a
/// window that wasn't the target through exactly that route.
#[derive(Debug, PartialEq)]
pub(crate) enum SecretRoute {
    /// This device: an accessibility value write on that element, with no
    /// focus step (focusing steals the key window) and no key fallback
    /// (driver `act` `{op: "set_value", focus: false}`).
    ElementValue { element_id: String },
    /// A guest: the element write when an id is given, else typed on the
    /// run's own display (or the guest's one desktop).
    Guest { element_id: Option<String> },
}

/// Pick the route, or refuse. Pure, so the "never desktop typing on this
/// device" rule is tested without a driver.
pub(crate) fn secret_route(target: &Target, element_id: Option<&str>) -> Result<SecretRoute, Fail> {
    let id = element_id.map(str::trim).filter(|e| !e.is_empty()).map(str::to_string);
    match target {
        Target::ThisDevice => match id {
            // A vision element is a screen position: writing to it means
            // clicking and typing at the screen.
            Some(e) if e.starts_with('v') => Err(Fail::from(
                "element_id must be an accessibility element from read_ui (not a vision element 'v…'); a secret is never typed at a screen position on this device",
            )),
            Some(e) => Ok(SecretRoute::ElementValue { element_id: e }),
            None => Err(Fail::from(SECRET_NEEDS_ELEMENT)),
        },
        Target::Guest { .. } => Ok(SecretRoute::Guest { element_id: id }),
        Target::Browser { .. } => Err(Fail::from("use_credential doesn't run on the browser toolset")),
    }
}

/// The driver call for a value-only element write. The value travels only in
/// this request (local driver socket or the base64 guest RPC line).
pub(crate) fn element_value_params(element_id: &str, value: &str) -> Value {
    json!({ "id": element_id, "op": "set_value", "value": value, "focus": false })
}

/// Write the resolved value into a field, per target. On this device it is
/// an element value write through the local driver sidecar only (no CLI
/// fallback, whose argv would carry the value). On a guest it is the element
/// write when an id is given, then typing scoped to the run's own display
/// (`computer_agent_display`; the guest driver adds `display`, the xdotool
/// path exports it), as before. The value is never written to logs, the
/// audit row, or the model result; the driver's reply is dropped unread.
async fn type_secret(target: &Target, value: &str, element_id: Option<&str>) -> Result<(), Fail> {
    match secret_route(target, element_id)? {
        SecretRoute::ElementValue { element_id } => {
            driver_op("act", &element_value_params(&element_id, value)).await.map(|_| ()).map_err(|f| {
                Fail::from(format!(
                    "couldn't write the secret into element {element_id}: {}. Nothing was typed. Call read_ui for a fresh id, or call request_human so the person fills it.",
                    f.message
                ))
            })
        }
        SecretRoute::Guest { element_id } => type_secret_guest(target, value, element_id.as_deref()).await,
    }
}

async fn type_secret_guest(target: &Target, value: &str, element_id: Option<&str>) -> Result<(), Fail> {
    let driver_up = crate::guest_driver::guest_driver_available(target).await;
    if driver_up {
        if let Some(id) = element_id {
            match crate::guest_driver::guest_driver_raw(target, "act", element_value_params(id, value), Duration::from_secs(30)).await {
                Ok(_) => return Ok(()),
                // Guests keep their typing path: one agent per display, so
                // the focused field is the run's own.
                Err(e) => tracing::debug!(error = %e.message, "guest element write refused; typing on the run's display"),
            }
        }
        return crate::guest_driver::guest_driver_raw(
            target,
            "pixel_type_text",
            json!({ "scope": "desktop", "text": value }),
            Duration::from_secs(30),
        )
        .await
        .map(|_| ());
    }
    match target {
        Target::Guest { os, .. } if os == "windows" => {
            let script = format!(
                "$t = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))\n$t = [regex]::Replace($t, '[+^%~(){{}}\\[\\]]', '{{$0}}')\nAdd-Type -AssemblyName System.Windows.Forms\n[System.Windows.Forms.SendKeys]::SendWait($t)",
                B64.encode(value.as_bytes())
            );
            windows_exec(target, &script).await.map(|_| ()).map_err(Fail::from)
        }
        _ => {
            let b64 = B64.encode(value.as_bytes());
            guest_exec(
                target,
                &format!("printf %s '{b64}' | base64 -d > /tmp/.allternit-cred && xdotool type --delay 12 -- \"$(cat /tmp/.allternit-cred)\"; rc=$?; rm -f /tmp/.allternit-cred; exit $rc"),
            )
            .await
            .map(|_| ())
            .map_err(Fail::from)
        }
    }
}

async fn use_credential(user: &AuthUser, target: &Target, input: &Value) -> Result<ToolsetResult, Fail> {
    let name = input.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("secret");
    let element_id = input.get("element_id").and_then(Value::as_str).map(str::trim).filter(|e| !e.is_empty()).map(str::to_string);
    // Where it goes is settled before the secret is opened: on this device
    // only an element value write, never typing into the front window.
    secret_route(target, element_id.as_deref())?;
    let resolved = resolve_credential(user, input).await?;
    // Every tool result is screened for this value from now on (keychain / Bitwarden / 1Password
    // values are not in the vault's screening set).
    crate::authority::secrets::attach_for_user(&user.user_id, &resolved.value);

    // The binding check: a bound credential only types into its app/domain.
    if let Some(bind) = &resolved.bind {
        let declared = input.get("domain").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty());
        match declared {
            Some(d) if d.eq_ignore_ascii_case(bind) => {}
            Some(d) => return Err(Fail::from(format!("credential '{name}' is bound to '{bind}', not '{d}'"))),
            None => return Err(Fail::from(format!("credential '{name}' is bound to '{bind}'; pass domain '{bind}' to confirm where it types"))),
        }
    }

    use crate::aci_credentials::CredentialType;
    let value = match (kind, resolved.credential_type) {
        ("totp", _) => crate::aci_credentials::totp_code(&resolved.value, chrono::Utc::now().timestamp() as u64, 6)
            .ok_or_else(|| Fail::from(format!("couldn't generate a TOTP code from credential '{name}' (is it a base32 seed?)")))?,
        ("secret", CredentialType::TotpSecret) => {
            return Err(Fail::from(format!("credential '{name}' is a TOTP seed; use kind \"totp\" (the seed itself is never typed)")))
        }
        ("secret", _) => resolved.value,
        (other, _) => return Err(Fail::from(format!("unknown use_credential kind '{other}' (secret or totp)"))),
    };

    type_secret(target, &value, element_id.as_deref()).await?;
    // The result names the credential, never the value.
    let into = element_id.as_deref().map_or_else(|| "the focused field".to_string(), |e| format!("element {e}"));
    Ok(ToolsetResult {
        is_error: false,
        content: vec![text(format!("Credential '{name}' written into {into}."))],
        browser_state: None,
        screen: screen_info(None),
        error: None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn path_lookup_splits_on_the_list_separator_not_the_dir_separator() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("bw"), b"").unwrap();
        let list = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(super::find_in_path_list(&list, "bw", false), Some(b.path().join("bw")));
        assert_eq!(super::find_in_path_list(&list, "op", false), None);
        // A directory named like the tool is not the tool.
        std::fs::create_dir(a.path().join("op")).unwrap();
        assert_eq!(super::find_in_path_list(&list, "op", false), None);
    }

    #[test]
    fn path_lookup_tries_windows_executable_extensions() {
        let a = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("bw.cmd"), b"").unwrap();
        std::fs::write(a.path().join("op.exe"), b"").unwrap();
        let list = std::env::join_paths([a.path()]).unwrap();
        assert_eq!(super::find_in_path_list(&list, "bw", true), Some(a.path().join("bw.cmd")));
        assert_eq!(super::find_in_path_list(&list, "op", true), Some(a.path().join("op.exe")));
        assert_eq!(super::find_in_path_list(&list, "bw", false), None);
    }

    #[test]
    fn credential_blobs_decode_utf16_from_cmdkey_and_utf8_from_others() {
        let utf16: Vec<u8> = "hunter2".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(super::decode_credential_blob(&utf16), "hunter2");
        assert_eq!(super::decode_credential_blob(b"abcd"), "abcd");
        assert_eq!(super::decode_credential_blob("pässwörd".as_bytes()), "pässwörd");
        assert_eq!(super::decode_credential_blob(b""), "");
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn v2_member_list_is_the_spec_set() {
        assert_eq!(V2_MEMBERS, ["read_ui", "act", "run_batch", "verify", "request_human", "use_credential", "run_subtask", "run_parallel", "run_skill", "skills"]);
        assert!(is_v2_member("read_ui"));
        assert!(!is_v2_member("left_click"));
        assert!(!is_v2_member("screenshot"));
    }

    #[test]
    fn text_entry_ops_need_approval_only_off_sandbox() {
        // act: set_value/select/press type text; click/focus/menu don't.
        assert!(v2_member_needs_approval("act", &json!({ "op": "set_value" }), false));
        assert!(v2_member_needs_approval("act", &json!({ "op": "press" }), false));
        assert!(!v2_member_needs_approval("act", &json!({ "op": "click" }), false));
        assert!(!v2_member_needs_approval("act", &json!({ "op": "menu" }), false));
        // Sandboxed targets run without per-action approval.
        assert!(!v2_member_needs_approval("act", &json!({ "op": "set_value" }), true));
        // run_batch: approval when any act step enters text.
        let batch = json!({ "steps": [
            { "act": { "id": "e1", "op": "click" } },
            { "act": { "id": "e2", "op": "set_value", "value": "hi" }, "expect": { "id": "e2", "value": "hi" } },
        ] });
        assert!(v2_member_needs_approval("run_batch", &batch, false));
        let clicks_only = json!({ "steps": [{ "act": { "id": "e1", "op": "click" } }] });
        assert!(!v2_member_needs_approval("run_batch", &clicks_only, false));
        // verify / read_ui / request_human never upgrade.
        for m in ["verify", "read_ui", "request_human", "use_credential"] {
            assert!(!v2_member_needs_approval(m, &json!({}), false), "{m}");
        }
    }

    #[test]
    fn human_gate_opens_and_closes_once() {
        assert!(!human_gate_done("c-test"));
        let latch = human_gate_open("c-test", "do the captcha");
        assert!(human_gate_pending("c-test").is_some());
        assert!(human_gate_done("c-test"));
        assert!(!human_gate_done("c-test"), "the window is single-shot");
        // The latch still wakes its waiter even though the gate is gone.
        latch.notify_waiters();
        assert!(human_gate_pending("c-test").is_none());
    }

    /// Security regression (Safari spike, 2026-11-26): use_credential on
    /// this device sent the secret through desktop-scope typing, which types
    /// into whatever window is in front; a dummy secret landed in a window
    /// that wasn't the target. On this device every input must either be
    /// refused or become a value-only write on one element.
    #[test]
    fn a_secret_never_goes_through_desktop_typing_on_this_device() {
        use crate::computer_toolset::Target;
        for element_id in [None, Some(""), Some("   "), Some("v3"), Some("e12"), Some(" e7 ")] {
            match super::secret_route(&Target::ThisDevice, element_id) {
                Ok(super::SecretRoute::ElementValue { element_id: id }) => {
                    assert!(!id.is_empty() && !id.starts_with('v'), "{element_id:?} -> {id}");
                    let params = super::element_value_params(&id, "S3cret-test-123");
                    assert_eq!(params["op"], "set_value");
                    assert_eq!(params["focus"], false, "no focus step: focusing steals the key window");
                    assert_eq!(params["id"], id.as_str());
                    assert!(params.get("scope").is_none() && params.get("text").is_none(), "never a typing call: {params}");
                }
                Ok(other) => panic!("{element_id:?} took {other:?} on this device"),
                Err(fail) => assert!(element_id.map(str::trim).filter(|e| !e.is_empty() && !e.starts_with('v')).is_none(), "{element_id:?} refused: {}", fail.message),
            }
        }
        let refused = super::secret_route(&Target::ThisDevice, None).unwrap_err().message;
        assert!(refused.contains("element_id") && refused.contains("request_human"), "the refusal says what to do: {refused}");
        assert_eq!(
            super::secret_route(&Target::ThisDevice, Some(" e7 ")).unwrap(),
            super::SecretRoute::ElementValue { element_id: "e7".into() }
        );
    }

    #[test]
    fn guests_run_structured_members_through_the_guest_driver() {
        // D1b: the guest image ships the driver, so guests no longer refuse
        // structured members statically; whether the driver answers is
        // probed at runtime (guest_structured_down / guest_driver).
        assert!(crate::computer_toolset::unsupported_reason("guest_linux", crate::computer_toolset::Toolset::Computer, "read_ui").is_none());
        assert!(crate::computer_toolset::unsupported_reason("guest_windows", crate::computer_toolset::Toolset::Computer, "use_credential").is_none());
        assert!(crate::computer_toolset::unsupported_reason("guest_linux", crate::computer_toolset::Toolset::Computer, "left_click").is_none());
        assert!(crate::computer_toolset::unsupported_reason("this_device", crate::computer_toolset::Toolset::Computer, "read_ui").is_none());
        assert!(crate::computer_toolset::unsupported_reason("browser_gateway", crate::computer_toolset::Toolset::Computer, "read_ui").is_some());
    }

    #[test]
    fn run_batch_timeout_scales_with_waits() {
        let small = json!({ "steps": [{ "act": { "id": "e", "op": "click" } }] });
        let big = json!({ "steps": [{ "wait_for": { "name": "Go" }, "timeout_ms": 60_000 }] });
        assert!(driver_timeout("run_batch", &small) >= Duration::from_secs(60));
        assert!(driver_timeout("run_batch", &big) > driver_timeout("run_batch", &small));
        assert!(driver_timeout("read_ui", &json!({ "target": "the Export button" })) >= Duration::from_secs(240));
        assert!(driver_timeout("read_ui", &json!({})) >= Duration::from_secs(60));
        assert!(driver_timeout("verify", &json!({})) < driver_timeout("run_batch", &small));
    }
}
