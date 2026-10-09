//! Mouse and keyboard on this computer (the Mac running Allternit Desktop),
//! for whoever holds its control lease (Eoj 2026-09-28: taking control of
//! this Mac from the Computer view drives the real screen).
//!
//! Input goes through the Allternit Driver sidecar the desktop app runs
//! (`domains/computer-use/driver`, socket in `ALLTERNIT_DRIVER_SOCKET`): its
//! `pixel_*` ops are Cua Driver's screen-wide `desktop` scope, so coordinates
//! are real screen pixels and keys go to the frontmost app. The sidecar owns
//! input per window and logs every op to its router audit. When an older
//! desktop app runs without the sidecar, calls fall back to Cua Driver's CLI
//! (`ALLTERNIT_CUA_DRIVER_PATH` / `_SOCKET`, computer-use-driver-manager.ts).

use axum::{http::StatusCode, response::IntoResponse, response::Response, Json};
use serde_json::{json, Value};
use tracing::warn;

use crate::bot_desktop_input::{KeyboardInput, MouseInput};

const INSTALLED_CUA_DRIVER: &str = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver";

/// A Cua Driver tool call: tool name and its arguments.
pub type DriverCall = (&'static str, Value);

/// Map a mouse request onto a driver call in screen coordinates.
pub fn mouse_call(input: &MouseInput) -> Result<DriverCall, String> {
    let xy = || match (input.x, input.y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("{} needs x and y", input.action)),
    };
    let button = input.button.clone().unwrap_or_else(|| "left".into());
    match input.action.as_str() {
        "move" => {
            let (x, y) = xy()?;
            Ok(("move_cursor", json!({ "scope": "desktop", "x": x, "y": y })))
        }
        "click" | "rightclick" | "doubleclick" => {
            let (x, y) = xy()?;
            let button = if input.action == "rightclick" {
                "right".to_string()
            } else {
                button
            };
            let count = if input.action == "doubleclick" { 2 } else { 1 };
            Ok((
                "click",
                json!({ "scope": "desktop", "x": x, "y": y, "button": button, "count": count }),
            ))
        }
        "drag" => {
            let (x, y) = xy()?;
            let (Some(end_x), Some(end_y)) = (input.end_x, input.end_y) else {
                return Err("drag needs end_x and end_y".into());
            };
            Ok((
                "drag",
                json!({ "scope": "desktop", "from_x": x, "from_y": y, "to_x": end_x, "to_y": end_y }),
            ))
        }
        "scroll" => {
            let direction = match button.as_str() {
                "up" | "down" | "left" | "right" => button.clone(),
                _ => "down".into(),
            };
            let mut args = json!({
                "scope": "desktop",
                "direction": direction,
                "amount": input.amount.unwrap_or(3).clamp(1, 50),
            });
            if let (Some(x), Some(y)) = (input.x, input.y) {
                args["x"] = json!(x);
                args["y"] = json!(y);
            }
            Ok(("scroll", args))
        }
        other => Err(format!(
            "unsupported mouse action on this computer: {other}"
        )),
    }
}

const MODIFIERS: [&str; 7] = ["cmd", "shift", "option", "alt", "ctrl", "control", "fn"];

/// Map a keyboard request onto a driver call. `key` may carry modifiers as
/// `cmd+shift+t`; those become a hotkey chord.
pub fn keyboard_call(input: &KeyboardInput) -> Result<DriverCall, String> {
    match input.action.as_str() {
        "type" => {
            let text = input
                .text
                .clone()
                .filter(|t| !t.is_empty())
                .ok_or("type needs text")?;
            Ok(("type_text", json!({ "scope": "desktop", "text": text })))
        }
        "key" => {
            let key = input
                .key
                .clone()
                .filter(|k| !k.is_empty())
                .ok_or("key needs a key")?;
            let parts: Vec<String> = key
                .split('+')
                .map(|p| p.trim().to_lowercase())
                .filter(|p| !p.is_empty())
                .collect();
            let has_modifier = parts.len() > 1
                && parts[..parts.len() - 1]
                    .iter()
                    .all(|p| MODIFIERS.contains(&p.as_str()));
            if has_modifier {
                Ok(("hotkey", json!({ "scope": "desktop", "keys": parts })))
            } else {
                Ok((
                    "press_key",
                    json!({ "scope": "desktop", "key": key.to_lowercase() }),
                ))
            }
        }
        other => Err(format!(
            "unsupported keyboard action on this computer: {other}"
        )),
    }
}

/// macOS virtual key code for a contract key name (xdotool-style names, as
/// the toolset uses them: `Return`, `Page_Up`, `shift`, `a`, `F5`).
pub fn mac_keycode(name: &str) -> Option<u16> {
    let n = name.trim().to_lowercase().replace('-', "_");
    let code = match n.as_str() {
        "a" => 0, "s" => 1, "d" => 2, "f" => 3, "h" => 4, "g" => 5, "z" => 6, "x" => 7, "c" => 8, "v" => 9,
        "b" => 11, "q" => 12, "w" => 13, "e" => 14, "r" => 15, "y" => 16, "t" => 17, "1" => 18, "2" => 19,
        "3" => 20, "4" => 21, "6" => 22, "5" => 23, "=" | "equal" => 24, "9" => 25, "7" => 26,
        "-" | "minus" => 27, "8" => 28, "0" => 29, "]" | "bracketright" => 30, "o" => 31, "u" => 32,
        "[" | "bracketleft" => 33, "i" => 34, "p" => 35, "return" | "enter" | "kp_enter" => 36, "l" => 37,
        "j" => 38, "'" | "apostrophe" => 39, "k" => 40, ";" | "semicolon" => 41, "\\" | "backslash" => 42,
        "," | "comma" => 43, "/" | "slash" => 44, "n" => 45, "m" => 46, "." | "period" => 47, "tab" => 48,
        "space" | " " => 49, "`" | "grave" => 50, "backspace" => 51, "escape" | "esc" => 53,
        "cmd" | "command" | "super" | "super_l" | "meta" | "meta_l" | "win" => 55,
        "shift" | "shift_l" => 56, "caps_lock" | "capslock" => 57, "alt" | "alt_l" | "option" | "opt" => 58,
        "ctrl" | "control" | "control_l" | "ctrl_l" => 59, "shift_r" => 60, "alt_r" => 61,
        "control_r" | "ctrl_r" => 62, "super_r" | "cmd_r" => 54, "fn" => 63,
        "f1" => 122, "f2" => 120, "f3" => 99, "f4" => 118, "f5" => 96, "f6" => 97, "f7" => 98, "f8" => 100,
        "f9" => 101, "f10" => 109, "f11" => 103, "f12" => 111, "home" => 115,
        "page_up" | "pageup" | "prior" => 116, "delete" => 117, "end" => 119,
        "page_down" | "pagedown" | "next" => 121, "left" => 123, "right" => 124, "down" => 125, "up" => 126,
        _ => return None,
    };
    Some(code)
}

/// CGEvent modifier flag a key sets while held (0 for ordinary keys).
fn mac_flag(code: u16) -> u64 {
    match code {
        56 | 60 => 0x20000,  // shift
        59 | 62 => 0x40000,  // control
        58 | 61 => 0x80000,  // option
        55 | 54 => 0x100000, // command
        _ => 0,
    }
}

/// The JXA script that holds a chord (`shift`, `cmd+a`) for `secs` seconds:
/// keys go down in order, stay down, then come up in reverse, each event
/// carrying the modifier flags held at that moment.
pub fn hold_key_script(spec: &str, secs: f64) -> Result<String, String> {
    let codes: Vec<u16> = spec
        .split('+')
        .filter(|p| !p.trim().is_empty())
        .map(|p| mac_keycode(p).ok_or_else(|| format!("not a key name this Mac understands: {p}")))
        .collect::<Result<_, _>>()?;
    if codes.is_empty() {
        return Err("text is required".into());
    }
    let mut lines = vec!["ObjC.import('CoreGraphics');".to_string(), "function k(c, d, f) { const e = $.CGEventCreateKeyboardEvent(null, c, d); $.CGEventSetFlags(e, f); $.CGEventPost(0, e); }".to_string()];
    let mut flags = 0u64;
    for c in &codes {
        flags |= mac_flag(*c);
        lines.push(format!("k({c}, true, {flags});"));
    }
    lines.push(format!("delay({:.3});", secs.clamp(0.0, 30.0)));
    for c in codes.iter().rev() {
        flags &= !mac_flag(*c);
        lines.push(format!("k({c}, false, {flags});"));
    }
    Ok(lines.join("\n"))
}

/// Hold a key or chord on this Mac. Cua Driver 0.34 has no press-and-hold
/// call on macOS, so this posts the key-down/key-up events itself through
/// CoreGraphics (keys go to the frontmost app, like the driver's desktop
/// scope). Needs the Accessibility permission Allternit Desktop already has.
pub async fn hold_key(spec: &str, secs: f64) -> Result<(), String> {
    let script = hold_key_script(spec, secs)?;
    let wait = std::time::Duration::from_secs_f64(secs.clamp(0.0, 30.0) + 10.0);
    let out = tokio::time::timeout(
        wait,
        tokio::process::Command::new("/usr/bin/osascript").args(["-l", "JavaScript", "-e", &script]).output(),
    )
    .await
    .map_err(|_| "holding the key didn't finish in time".to_string())?
    .map_err(|e| format!("couldn't post key events: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("couldn't hold the key: {}", err.trim().chars().take(300).collect::<String>()));
    }
    Ok(())
}

fn driver_command() -> Option<(String, Vec<String>)> {
    let path = std::env::var("ALLTERNIT_CUA_DRIVER_PATH")
        .ok()
        .filter(|p| std::path::Path::new(p).exists())
        .or_else(|| {
            std::path::Path::new(INSTALLED_CUA_DRIVER)
                .exists()
                .then(|| INSTALLED_CUA_DRIVER.to_string())
        })?;
    let mut flags = Vec::new();
    if let Ok(socket) = std::env::var("ALLTERNIT_CUA_DRIVER_SOCKET") {
        if !socket.is_empty() {
            flags.push("--socket".into());
            flags.push(socket);
        }
    }
    if std::env::var("ALLTERNIT_CUA_DRIVER_EMBEDDED").as_deref() == Ok("true") {
        flags.push("--embedded".into());
    }
    Some((path, flags))
}

/// True when a driver reply is a result, not a refusal. Refusals are either
/// plain text ("Missing required integer field: pid") or JSON with `code`
/// ("invalid_action_target"), and both exit 0.
pub fn driver_accepted(stdout: &[u8]) -> bool {
    match serde_json::from_slice::<Value>(stdout) {
        Ok(Value::Object(map)) => !map.contains_key("code"),
        _ => false,
    }
}

/// Run one driver call and answer the route.
pub async fn run(call: Result<DriverCall, String>) -> Response {
    let (tool, args) = match call {
        Ok(call) => call,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "bad_input", "message": message })),
            )
                .into_response()
        }
    };
    match call_driver(tool, args).await {
        Ok(_) => Json(json!({ "success": true })).into_response(),
        Err(DriverFailure::Unavailable(message)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "computer_use_unavailable", "message": message })),
        )
            .into_response(),
        Err(DriverFailure::Timeout) => (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "error": "driver_timeout" }))).into_response(),
        Err(DriverFailure::Refused(message)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "driver_failed", "message": message.chars().take(300).collect::<String>() })),
        )
            .into_response(),
    }
}

/// Why a driver call didn't succeed.
#[derive(Debug)]
pub enum DriverFailure {
    Unavailable(String),
    Timeout,
    Refused(String),
}

impl From<DriverFailure> for String {
    fn from(f: DriverFailure) -> String {
        match f {
            DriverFailure::Unavailable(m) => m,
            DriverFailure::Timeout => "the driver didn't answer in time".into(),
            DriverFailure::Refused(m) => m,
        }
    }
}

const DRIVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The Allternit Driver sidecar's socket, when the desktop app runs it.
fn allternit_driver_socket() -> Option<String> {
    std::env::var("ALLTERNIT_DRIVER_SOCKET")
        .ok()
        .filter(|s| !s.is_empty() && std::path::Path::new(s).exists())
}

/// One JSON-RPC call to the Allternit Driver (one JSON object per line).
#[cfg(unix)]
pub async fn driver_rpc(socket: &str, method: &str, params: Value) -> Result<Value, DriverFailure> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let call = async {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| DriverFailure::Unavailable(format!("The computer-use driver isn't running on this computer ({e}).")))?;
        let mut line = serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
            .map_err(|e| DriverFailure::Refused(e.to_string()))?;
        line.push(b'\n');
        stream.write_all(&line).await.map_err(|e| DriverFailure::Refused(e.to_string()))?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).await.map_err(|e| DriverFailure::Refused(e.to_string()))?;
        let reply: Value = serde_json::from_str(&reply).map_err(|_| DriverFailure::Refused("the driver closed the connection".into()))?;
        match reply.get("error") {
            Some(err) => Err(DriverFailure::Refused(format!(
                "the driver refused {method}: {}",
                err.get("message").and_then(Value::as_str).unwrap_or("unknown error")
            ))),
            None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
        }
    };
    tokio::time::timeout(DRIVER_TIMEOUT, call).await.map_err(|_| DriverFailure::Timeout)?
}

/// One driver call for the toolset executor and the input routes: a Cua
/// Driver desktop-scope tool (`click`, `type_text`, `get_cursor_position`…),
/// sent as the Allternit Driver's `pixel_<tool>` op, or through Cua's CLI
/// when the sidecar isn't running. The driver's JSON reply on success.
pub async fn call_driver(tool: &str, args: Value) -> Result<Value, DriverFailure> {
    #[cfg(unix)]
    if let Some(socket) = allternit_driver_socket() {
        return driver_rpc(&socket, &format!("pixel_{tool}"), args).await.inspect_err(|e| warn!(tool, ?e, "allternit driver call failed"));
    }
    let (path, flags) = driver_command()
        .ok_or_else(|| DriverFailure::Unavailable("The computer-use driver isn't running on this computer.".into()))?;
    let output = tokio::process::Command::new(&path)
        .arg("call")
        .arg(tool)
        .arg(args.to_string())
        .args(&flags)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(DRIVER_TIMEOUT, output).await {
        // The CLI exits 0 on refusals too; only a JSON result without an
        // error `code` is a success.
        Ok(Ok(out)) if out.status.success() && driver_accepted(&out.stdout) => {
            Ok(serde_json::from_slice(&out.stdout).unwrap_or(Value::Null))
        }
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = if stderr.trim().is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { stderr.trim().to_string() };
            warn!(tool, %detail, "computer-use driver call failed");
            Err(DriverFailure::Refused(format!("the driver refused {tool}: {}", detail.chars().take(300).collect::<String>())))
        }
        Ok(Err(e)) => Err(DriverFailure::Refused(format!("couldn't run the driver: {e}"))),
        Err(_) => Err(DriverFailure::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse(action: &str, x: Option<i32>, y: Option<i32>) -> MouseInput {
        MouseInput {
            action: action.into(),
            x,
            y,
            button: None,
            end_x: None,
            end_y: None,
            amount: None,
        }
    }

    fn key(action: &str, text: Option<&str>, key: Option<&str>) -> KeyboardInput {
        KeyboardInput {
            action: action.into(),
            text: text.map(Into::into),
            key: key.map(Into::into),
        }
    }

    #[test]
    fn clicks_are_screen_wide() {
        let (tool, args) = mouse_call(&mouse("doubleclick", Some(10), Some(20))).unwrap();
        assert_eq!(tool, "click");
        assert_eq!(
            args,
            json!({ "scope": "desktop", "x": 10, "y": 20, "button": "left", "count": 2 })
        );
        let (_, args) = mouse_call(&mouse("rightclick", Some(1), Some(2))).unwrap();
        assert_eq!(args["button"], "right");
        assert!(mouse_call(&mouse("click", None, Some(2))).is_err());
        assert!(mouse_call(&mouse("mousedown", Some(1), Some(2))).is_err());
    }

    #[test]
    fn moves_are_screen_wide_without_a_target() {
        // `target` together with `scope: desktop` is refused as
        // invalid_action_target; scope alone routes through global input.
        let (tool, args) = mouse_call(&mouse("move", Some(4), Some(5))).unwrap();
        assert_eq!(tool, "move_cursor");
        assert_eq!(args, json!({ "scope": "desktop", "x": 4, "y": 5 }));
    }

    #[test]
    fn driver_refusals_are_failures() {
        assert!(driver_accepted(
            br#"{"route":"global_input","effect":"unverifiable"}"#
        ));
        assert!(!driver_accepted(br#"{"code":"invalid_action_target"}"#));
        assert!(!driver_accepted(b"Missing required integer field: pid"));
        assert!(!driver_accepted(b""));
    }

    #[test]
    fn scroll_uses_the_button_as_direction() {
        let mut input = mouse("scroll", Some(5), Some(6));
        input.button = Some("up".into());
        input.amount = Some(99);
        let (tool, args) = mouse_call(&input).unwrap();
        assert_eq!(tool, "scroll");
        assert_eq!(
            args,
            json!({ "scope": "desktop", "direction": "up", "amount": 50, "x": 5, "y": 6 })
        );
    }

    #[test]
    fn hold_key_presses_then_releases_in_reverse() {
        let s = hold_key_script("cmd+shift+a", 0.5).unwrap();
        let downs: Vec<&str> = s.lines().filter(|l| l.contains("true")).collect();
        assert_eq!(downs, ["k(55, true, 1048576);", "k(56, true, 1179648);", "k(0, true, 1179648);"]);
        assert!(s.contains("delay(0.500);"));
        assert!(s.trim_end().ends_with("k(55, false, 0);"));
        assert!(hold_key_script("hyper", 1.0).is_err());
    }

    #[test]
    fn keys_and_chords() {
        assert_eq!(
            keyboard_call(&key("type", Some("hi"), None)).unwrap(),
            ("type_text", json!({ "scope": "desktop", "text": "hi" }))
        );
        assert_eq!(
            keyboard_call(&key("key", None, Some("Return"))).unwrap(),
            ("press_key", json!({ "scope": "desktop", "key": "return" }))
        );
        assert_eq!(
            keyboard_call(&key("key", None, Some("cmd+shift+T"))).unwrap(),
            (
                "hotkey",
                json!({ "scope": "desktop", "keys": ["cmd", "shift", "t"] })
            )
        );
        // A literal "+" is a key, not a chord.
        assert_eq!(
            keyboard_call(&key("key", None, Some("+"))).unwrap().0,
            "press_key"
        );
        assert!(keyboard_call(&key("type", Some(""), None)).is_err());
    }
}
