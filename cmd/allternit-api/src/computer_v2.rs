//! Contract v2 (`allternit.computer.v2`): the six structured members every
//! model family gets as function tools — `read_ui`, `act`, `run_batch`,
//! `verify` through the Allternit Driver sidecar, `request_human` (lease
//! pause/resume around a human window) and `use_credential` (vault-backed
//! typing where the value never enters the model context or the logs).
//!
//! The 17 pixel members stay in `computer_toolset::dispatch`; this module is
//! the v2 path out of `execute`. Approvals and the audit row for these
//! members are the existing lease → policy → approval → audit pipeline in
//! `computer_toolset::execute` (act's text-entering ops and use_credential
//! upgrade to a human approval on non-sandbox targets, the same rule as
//! `type`/`key`).

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
pub const V2_MEMBERS: [&str; 6] = ["read_ui", "act", "run_batch", "verify", "request_human", "use_credential"];

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
fn driver_timeout(member: &str, input: &Value) -> Duration {
    match member {
        "run_batch" => {
            let steps = input.get("steps").and_then(Value::as_array);
            let count = steps.map_or(0, |s| s.len() as u64);
            let waits = steps.map_or(0, |s| {
                s.iter().filter_map(|st| st.get("timeout_ms").and_then(Value::as_u64)).sum::<u64>()
            });
            Duration::from_secs((30 + waits / 1000 + count * 2).clamp(60, 300))
        }
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

/// The result's screen block for structured members: the real screen size
/// from the driver (no capture) and the frame a screenshot would use.
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

/// One sidecar op for a structured member. The params pass nearly verbatim;
/// the driver answers JSON (element trees, batch results, verify rows).
async fn driver_op(member: &str, params: &Value) -> Result<Value, Fail> {
    let endpoint = td::DriverEndpoint::resolve().ok_or(
        "The Allternit Driver sidecar isn't running on this computer. Open Allternit Desktop (or update it) and try again.",
    )?;
    td::driver_rpc(&endpoint, member, params.clone(), driver_timeout(member, params))
        .await
        .map_err(|e| Fail::from(String::from(e)))
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
    match member {
        "read_ui" | "act" | "run_batch" | "verify" => {
            let reply = driver_op(member, input).await?;
            Ok(ToolsetResult {
                is_error: false,
                content: vec![text(serde_json::to_string(&reply).unwrap_or_else(|_| "{}".into()))],
                browser_state: None,
                screen: screen_block().await,
                error: None,
            })
        }
        "request_human" => request_human(state, user, computer, target, input, run_id).await,
        "use_credential" => use_credential(user, target, input).await,
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

    let is_this_device = matches!(target, Target::ThisDevice);
    let caller = agent_holder(user, Some(run_id));

    // Guests: release the agent's lease for the human window.
    if !is_this_device {
        let (db, cid, caller) = (state.db.clone(), computer.id.clone(), caller.clone());
        let released = tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            lease::release(&conn, &cid, &caller, chrono::Utc::now().timestamp())
        })
        .await;
        match released {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(Fail::from(format!("couldn't release control for the human window: {e}"))),
            Err(_) => return Err(Fail::from("internal error releasing control")),
        }
    }

    let done = human_gate_open(&computer.id, &reason);
    emit_human_event(&computer.id, "human_requested", &reason, run_id, {
        let mut m = Map::new();
        m.insert("timeout_ms".into(), json!(timeout_ms));
        m
    });

    let timed_out = tokio::time::timeout(Duration::from_millis(timeout_ms), done.notified()).await.is_err();
    HUMAN_GATES.lock().unwrap_or_else(|p| p.into_inner()).remove(&computer.id);

    // Guests: take the lease back. A person who is still driving keeps
    // control (agents never preempt a person); the next action then answers
    // 423 and the model can request_human again.
    if !is_this_device {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let (db, cid, caller) = (state.db.clone(), computer.id.clone(), caller.clone());
            let took = tokio::task::spawn_blocking(move || {
                let conn = db.connect().map_err(|e| lease::TakeError::Db(e.to_string()))?;
                lease::take(&conn, &cid, &caller, chrono::Utc::now().timestamp())
            })
            .await;
            match took {
                Ok(Ok(_)) => break,
                Ok(Err(lease::TakeError::Held(held))) => {
                    if std::time::Instant::now() >= deadline {
                        emit_human_event(&computer.id, "human_resumed", &reason, run_id, {
                            let mut m = Map::new();
                            m.insert("resumed".into(), json!(false));
                            m.insert("held_by".into(), json!(held.holder.label));
                            m
                        });
                        return Ok(human_result(&reason, timed_out, false));
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Ok(Err(lease::TakeError::Db(e))) => {
                    return Err(Fail::from(format!("couldn't take control back after the human window: {e}")))
                }
                Err(_) => return Err(Fail::from("internal error taking control back")),
            }
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
// use_credential: resolve from a backend, type into the focused field.
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
        ("keychain", cfg!(target_os = "macos")),
        ("bitwarden", which_like("bw")),
        ("onepassword", which_like("op")),
    ]
}

fn which_like(name: &str) -> bool {
    let paths = std::env::var("PATH").unwrap_or_default();
    paths.split(std::path::MAIN_SEPARATOR).any(|dir| {
        let mut p = std::path::PathBuf::from(dir);
        p.push(name);
        #[cfg(windows)]
        {
            if p.exists() {
                return true;
            }
            p.set_extension("exe");
        }
        p.exists()
    })
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

#[cfg(not(target_os = "macos"))]
async fn keychain_secret(_name: &str) -> Result<Option<String>, Fail> {
    Ok(None)
}

/// Bitwarden CLI adapter (team vaults). Needs `bw` on PATH and an unlocked
/// session (`BW_SESSION`); the item is looked up by name.
async fn bitwarden_secret(name: &str) -> Result<Option<String>, Fail> {
    if !which_like("bw") {
        return Err(Fail::from(
            "the Bitwarden CLI (bw) isn't installed on this computer; install it or use source \"vault\" or \"keychain\"",
        ));
    }
    let out = tokio::process::Command::new("bw")
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
    if !which_like("op") {
        return Err(Fail::from(
            "the 1Password CLI (op) isn't installed on this computer; install it or use source \"vault\" or \"keychain\"",
        ));
    }
    let out = tokio::process::Command::new("op")
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
/// this Mac's Keychain by default). Never logs the value.
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
    if source.is_none() || source == Some("keychain") {
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
        None => "the vault or this Mac's Keychain".to_string(),
    };
    Err(Fail::from(format!(
        "no credential named '{name}' in {where_}. Add one with POST /api/aci/credentials or `security add-generic-password -s allternit.computer -a <name> -w`."
    )))
}

/// Type the resolved value into the focused field, per target. The value
/// rides the local driver socket or the guest's exec channel (base64-wrapped
/// there, in a temp file that is removed at once); it is never written to
/// logs, the audit row, or the model result.
async fn type_secret(target: &Target, value: &str) -> Result<(), Fail> {
    match target {
        Target::ThisDevice => td::call_driver("type_text", json!({ "scope": "desktop", "text": value }))
            .await
            .map(|_| ())
            .map_err(|e| Fail::from(String::from(e))),
        Target::Guest { os, .. } if os == "windows" => {
            let script = format!(
                "$t = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))\n$t = [regex]::Replace($t, '[+^%~(){{}}\\[\\]]', '{{$0}}')\nAdd-Type -AssemblyName System.Windows.Forms\n[System.Windows.Forms.SendKeys]::SendWait($t)",
                B64.encode(value.as_bytes())
            );
            windows_exec(target, &script).await.map(|_| ()).map_err(Fail::from)
        }
        Target::Guest { .. } => {
            let b64 = B64.encode(value.as_bytes());
            guest_exec(
                target,
                &format!("printf %s '{b64}' | base64 -d > /tmp/.allternit-cred && xdotool type --delay 12 -- \"$(cat /tmp/.allternit-cred)\"; rc=$?; rm -f /tmp/.allternit-cred; exit $rc"),
            )
            .await
            .map(|_| ())
            .map_err(Fail::from)
        }
        Target::Browser { .. } => Err(Fail::from("use_credential doesn't run on the browser toolset")),
    }
}

async fn use_credential(user: &AuthUser, target: &Target, input: &Value) -> Result<ToolsetResult, Fail> {
    let name = input.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("secret");
    let resolved = resolve_credential(user, input).await?;

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

    type_secret(target, &value).await?;
    // The result names the credential, never the value.
    Ok(ToolsetResult {
        is_error: false,
        content: vec![text(format!("Credential '{name}' typed into the focused field."))],
        browser_state: None,
        screen: screen_info(None),
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn v2_member_list_is_the_spec_set() {
        assert_eq!(V2_MEMBERS, ["read_ui", "act", "run_batch", "verify", "request_human", "use_credential"]);
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

    #[test]
    fn guests_do_not_run_structured_members_yet() {
        assert!(crate::computer_toolset::unsupported_reason("guest_linux", crate::computer_toolset::Toolset::Computer, "read_ui").is_some());
        assert!(crate::computer_toolset::unsupported_reason("guest_windows", crate::computer_toolset::Toolset::Computer, "use_credential").is_some());
        assert!(crate::computer_toolset::unsupported_reason("guest_linux", crate::computer_toolset::Toolset::Computer, "left_click").is_none());
        assert!(crate::computer_toolset::unsupported_reason("this_device", crate::computer_toolset::Toolset::Computer, "read_ui").is_none());
    }

    #[test]
    fn run_batch_timeout_scales_with_waits() {
        let small = json!({ "steps": [{ "act": { "id": "e", "op": "click" } }] });
        let big = json!({ "steps": [{ "wait_for": { "name": "Go" }, "timeout_ms": 60_000 }] });
        assert!(driver_timeout("run_batch", &small) >= Duration::from_secs(60));
        assert!(driver_timeout("run_batch", &big) > driver_timeout("run_batch", &small));
        assert!(driver_timeout("verify", &json!({})) < driver_timeout("run_batch", &small));
    }
}
