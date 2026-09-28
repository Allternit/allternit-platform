//! Mouse and keyboard on this computer (the Mac running Allternit Desktop),
//! for whoever holds its control lease (Eoj 2026-09-28: taking control of
//! this Mac from the Computer view drives the real screen).
//!
//! Input goes through the computer-use driver the desktop app already runs
//! (Cua Driver), in its screen-wide `desktop` scope: coordinates are real
//! screen pixels (`get_desktop_state`), keys go to the frontmost app. The
//! desktop passes the driver's path and socket to this API
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
    let Some((path, flags)) = driver_command() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "computer_use_unavailable", "message": "The computer-use driver isn't running on this computer." })),
        )
            .into_response();
    };
    let output = tokio::process::Command::new(&path)
        .arg("call")
        .arg(tool)
        .arg(args.to_string())
        .args(&flags)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(10), output).await {
        // The driver exits 0 on refusals too; only a JSON result without an
        // error `code` is a success.
        Ok(Ok(out)) if out.status.success() && driver_accepted(&out.stdout) => {
            Json(json!({ "success": true })).into_response()
        }
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            warn!(tool, %stderr, "computer-use driver call failed");
            let detail = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            (StatusCode::BAD_GATEWAY, Json(json!({ "error": "driver_failed", "message": detail.chars().take(300).collect::<String>() }))).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "driver_failed", "message": e.to_string() })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({ "error": "driver_timeout" })),
        )
            .into_response(),
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
