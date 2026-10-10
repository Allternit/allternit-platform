//! Guest driver RPC (phase D1b): the Allternit Driver ships inside every
//! guest image and serves the structured computer-toolset members
//! (`read_ui`, `act`, `run_batch`, `verify`, plus its pixel/screenshot/safety
//! methods) on a loopback socket. This module reaches it through the guest
//! exec channel that `ExecutionDriver::exec` already provides (Incus exec,
//! tart exec, or the Firecracker guest agent's `execute`): one base64
//! JSON-RPC line in, one line out, via the image's `allternit-driver-rpc`
//! forwarder (`allternit_driver/guest_rpc.py`).
//!
//! The lease → policy → safety → approval → audit pipeline in
//! `computer_toolset::execute` is target-agnostic and unchanged: this is only
//! the transport under it. xdotool/scrot and the PowerShell input helpers
//! remain the explicit pixel fallback when the driver doesn't answer.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::computer_toolset::{guest_exec_timed, windows_exec_timed, Fail, Frame, Target};

/// The driver-backed structured members of allternit.computer.v2.
pub const STRUCTURED_MEMBERS: [&str; 4] = ["read_ui", "act", "run_batch", "verify"];

/// In-guest forwarder paths (installed by the image build, see
/// domains/computer-use/driver/packaging/).
const RPC_LINUX: &str = "/usr/local/bin/allternit-driver-rpc";
const RPC_WINDOWS: &str = r"C:\Program Files\Allternit\Driver\guest-rpc.py";

/// How long a capability probe answer is trusted (the probe costs one exec).
const PROBE_TTL: Duration = Duration::from_secs(30);
/// How long the probe itself may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

static PROBES: Lazy<Mutex<HashMap<String, (Instant, bool)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Structured members route through the guest driver (the v2 routing in
/// `computer_v2::execute_v2` handles the rest of the v2 members itself).
pub fn is_structured_member(member: &str) -> bool {
    STRUCTURED_MEMBERS.contains(&member)
}

fn cache_key(target: &Target) -> Option<String> {
    let Target::Guest { handle, os, .. } = target else { return None };
    Some(format!("{os}:{}", handle.id))
}

/// One JSON-RPC round trip to the guest's driver, through the exec channel.
/// Returns the parsed reply (which may carry a JSON-RPC `error`).
async fn rpc(target: &Target, request: Value, timeout: Duration) -> Result<Value, String> {
    let line = serde_json::to_string(&request).map_err(|e| format!("couldn't encode the driver request: {e}"))?;
    let payload = B64.encode(line.as_bytes());
    // The payload is base64: safe inside both single-quoted sh and
    // single-quoted PowerShell strings, so nothing is shell-escaped here.
    let out = match target {
        Target::Guest { os, .. } if os == "windows" => {
            let script = format!("$p='{payload}'; & python \"{RPC_WINDOWS}\" $p");
            windows_exec_timed(target, &script, timeout + Duration::from_secs(15)).await?
        }
        Target::Guest { .. } => {
            let script = format!("printf %s '{payload}' | base64 -d | {RPC_LINUX}");
            guest_exec_timed(target, &script, timeout + Duration::from_secs(10)).await?
        }
        _ => return Err("not a guest target".into()),
    };
    let last = out.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or_default();
    serde_json::from_str(last).map_err(|e| format!("the computer's driver sent a bad reply: {e}"))
}

/// Whether the guest's driver is up and serves structured reads. Probed with
/// the driver's `hello` capability report and cached per computer for
/// `PROBE_TTL`; every caller (execute, the schema endpoint, subtask steps,
/// the pixel fallback) shares one cache so a schema sweep doesn't hammer the
/// exec channel.
pub async fn guest_driver_available(target: &Target) -> bool {
    let Some(key) = cache_key(target) else { return false };
    if let Some((at, ok)) = PROBES.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        if at.elapsed() < PROBE_TTL {
            return *ok;
        }
    }
    let reply = rpc(target, json!({ "jsonrpc": "2.0", "id": 1, "method": "hello", "params": {} }), PROBE_TIMEOUT).await;
    let ok = reply
        .ok()
        .and_then(|v| v.get("result").cloned())
        .and_then(|r| r.get("hello").and_then(|h| h.get("read_ui")).and_then(Value::as_bool))
        .unwrap_or(false);
    PROBES.lock().unwrap_or_else(|p| p.into_inner()).insert(key, (Instant::now(), ok));
    ok
}

/// Run one structured member on the guest's driver. The member name is the
/// driver's JSON-RPC method verbatim, params pass nearly unchanged, and the
/// driver's JSON answer returns as the member's result.
pub async fn guest_driver_op(target: &Target, member: &str, params: &Value, timeout: Duration) -> Result<Value, Fail> {
    if !is_structured_member(member) {
        return Err(Fail::from(format!("{member} isn't a driver-served member")));
    }
    guest_driver_raw(target, member, params.clone(), timeout).await
}

/// Any driver method (structured members, `pixel_*`, `screenshot`,
/// `context`, ...). Prefer `guest_driver_op` for the structured members so
/// the member set stays greppable.
pub async fn guest_driver_raw(target: &Target, method: &str, params: Value, timeout: Duration) -> Result<Value, Fail> {
    let reply = rpc(target, json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }), timeout).await
        .map_err(Fail::from)?;
    if let Some(err) = reply.get("error") {
        let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
        return Err(Fail::from(format!("the computer's driver refused {method}: {message}")));
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

/// The guest screen size from the driver, without a screenshot.
pub async fn guest_screen_size(target: &Target) -> Option<Frame> {
    let reply = guest_driver_raw(target, "pixel_get_screen_size", json!({}), Duration::from_secs(8)).await.ok()?;
    let num = |k: &str| reply.get(k).and_then(Value::as_f64);
    let (w, h) = match (num("width"), num("height")) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let s = reply.get("text").and_then(Value::as_str)?;
            let (w, h) = s.split_once(['x', ','])?;
            (w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?)
        }
    };
    (w > 0.0 && h > 0.0).then_some(Frame { width: w.round() as u32, height: h.round() as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_member_set_is_the_driver_members() {
        assert_eq!(STRUCTURED_MEMBERS, ["read_ui", "act", "run_batch", "verify"]);
        assert!(is_structured_member("read_ui"));
        assert!(!is_structured_member("screenshot"));
        assert!(!is_structured_member("left_click"));
    }
}
