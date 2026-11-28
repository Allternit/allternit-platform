//! AllternitOS boot shell loop (plan step 29, gate G2c).
//!
//! 1. Wait for the model's /health.   2. Send one boot prompt with an OpenAI-style `shell`
//! tool.   3. Parse the tool call.   4. Send it to the authority as a contract `check`
//! (4-byte big-endian length + JSON, section 2.0).   5. Print
//! `shell-tool-call ok action=<a> decision=<allow|deny>` and `model tok/s=<n>`.
//! 6. Exit 0 so init powers off. Any failure exits 1 and prints a reason.
//!
//! std plus serde_json only: a blocking HTTP/1.1 client over TCP localhost.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const ACTIONS: [&str; 8] = ["open", "close", "focus", "arrange", "start_run", "stop_run", "install_app", "fork_app"];
const HEALTH_LIMIT: Duration = Duration::from_secs(1200);

/// Contract section 4.1 class map for ShellCommandV1 actions.
fn action_class(action: &str) -> Option<&'static str> {
    match action {
        "open" | "close" | "focus" | "arrange" => Some("ui_layout"),
        "start_run" | "stop_run" => Some("run_control"),
        "install_app" | "fork_app" => Some("app_install"),
        _ => None,
    }
}

fn tools() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "shell",
            "description": "Control the Allternit shell: open or close views, focus a window, start or stop a run, install or fork an app.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ACTIONS},
                    "view_id": {"type": "string", "description": "view to open, for open"},
                    "instance_id": {"type": "string", "description": "window to close or focus"},
                    "run_id": {"type": "string", "description": "run to stop"}
                },
                "required": ["action"]
            }
        }
    }])
}

/// Split an HTTP response into (status, body), decoding a chunked body.
fn parse_http(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header end")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("bad status line")?;
    let body = &raw[split + 4..];
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        Ok((status, dechunk(body)?))
    } else {
        Ok((status, body.to_vec()))
    }
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").ok_or("bad chunk")?;
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..eol]).split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "bad chunk size")?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size + 2 {
            return Err("short chunk".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

fn http(addr: &str, method: &str, path: &str, body: Option<&str>, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let payload = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| format!("read: {e}"))?;
    parse_http(&raw)
}

fn wait_health(addr: &str) -> Result<(), String> {
    let start = Instant::now();
    let mut last = 0;
    while start.elapsed() < HEALTH_LIMIT {
        if let Ok((200, _)) = http(addr, "GET", "/health", None, Duration::from_secs(5)) {
            return Ok(());
        }
        if start.elapsed().as_secs() / 30 > last {
            last = start.elapsed().as_secs() / 30;
            println!("shell: waiting for the model ({}s)", start.elapsed().as_secs());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err("model /health never became ready".into())
}

/// One chat call. Returns (action, tok/s).
fn ask_model(addr: &str, attempt: u32) -> Result<(String, f64), String> {
    let req = json!({
        "messages": [
            {"role": "system", "content": "You control the Allternit shell. Answer only by calling the shell tool."},
            {"role": "user", "content": "The machine just booted. Open the Files view."}
        ],
        "tools": tools(),
        "tool_choice": "required",
        "temperature": 0,
        "seed": attempt,
        "max_tokens": 128
    });
    let (code, body) = http(addr, "POST", "/v1/chat/completions", Some(&req.to_string()), Duration::from_secs(900))?;
    let resp: Value = serde_json::from_slice(&body).map_err(|e| format!("model reply is not json: {e}"))?;
    if code != 200 {
        return Err(format!("model http {code}: {resp}"));
    }
    let call = resp
        .pointer("/choices/0/message/tool_calls/0/function")
        .ok_or_else(|| format!("no tool call in reply: {resp}"))?;
    if call["name"].as_str() != Some("shell") {
        return Err(format!("tool name is not shell: {}", call["name"]));
    }
    // OpenAI style carries arguments as a JSON string; accept an object too.
    let args: Value = match &call["arguments"] {
        Value::String(s) => serde_json::from_str(s).map_err(|e| format!("arguments not json: {e}"))?,
        v => v.clone(),
    };
    let action = args["action"].as_str().ok_or("tool call has no action")?.to_string();
    let tps = resp.pointer("/timings/predicted_per_second").and_then(Value::as_f64).unwrap_or(0.0);
    Ok((action, tps))
}

fn authority_check(sock: &str, action: &str) -> Result<String, String> {
    let class = action_class(action).ok_or_else(|| format!("unknown action {action}"))?;
    let req = json!({
        "v": 1, "id": "boot-1", "call": "check",
        "caller": {"kind": "service", "principal": "shell"},
        "body": {
            "action": format!("shell.{action}"),
            "extensions": {"os.v1": {"action_class": class, "action_hash": null, "approval_token": null, "autonomy_level": "ask_first"}}
        }
    });
    let bytes = req.to_string().into_bytes();
    let mut s = UnixStream::connect(sock).map_err(|e| format!("connect {sock}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    s.write_all(&(bytes.len() as u32).to_be_bytes()).and_then(|_| s.write_all(&bytes)).map_err(|e| format!("write: {e}"))?;
    let mut len = [0u8; 4];
    s.read_exact(&mut len).map_err(|e| format!("read length: {e}"))?;
    let n = u32::from_be_bytes(len) as usize;
    if n > 1 << 20 {
        return Err("authority frame too large".into());
    }
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).map_err(|e| format!("read frame: {e}"))?;
    let resp: Value = serde_json::from_slice(&buf).map_err(|e| format!("authority reply: {e}"))?;
    if resp["ok"] != json!(true) || resp["id"] != json!("boot-1") {
        return Err(format!("authority error: {resp}"));
    }
    match resp.pointer("/body/decision").and_then(Value::as_str) {
        Some("ALLOW") | Some("ALLOW_WITH_LIMITS") => Ok("allow".into()),
        Some("DENY") | Some("ASK") => Ok("deny".into()),
        other => Err(format!("unknown decision {other:?}")),
    }
}

fn run() -> Result<(), String> {
    let url = std::env::var("ALLTERNIT_MODEL_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let addr = url.trim_start_matches("http://").trim_end_matches('/').to_string();
    let sock = std::env::var("ALLTERNIT_AUTHORITY_SOCK").unwrap_or_else(|_| "/run/allternit/authority.sock".into());
    wait_health(&addr)?;
    println!("shell: model is ready");
    let mut last_err = String::new();
    for attempt in 0..3 {
        match ask_model(&addr, attempt) {
            Ok((action, tps)) => {
                let decision = authority_check(&sock, &action)?;
                println!("shell-tool-call ok action={action} decision={decision}");
                println!("model tok/s={tps:.2}");
                return Ok(());
            }
            Err(e) => {
                println!("shell: attempt {attempt} failed: {e}");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

fn main() {
    match run() {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("shell: FAILED: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_cover_every_action() {
        for a in ACTIONS {
            assert!(action_class(a).is_some(), "{a}");
        }
        assert_eq!(action_class("open"), Some("ui_layout"));
        assert_eq!(action_class("fork_app"), Some("app_install"));
        assert_eq!(action_class("rm"), None);
    }

    #[test]
    fn parses_plain_and_chunked_http() {
        let (c, b) = parse_http(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
        assert_eq!((c, b.as_slice()), (200, &b"ok"[..]));
        let (c, b) = parse_http(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap();
        assert_eq!((c, b.as_slice()), (200, &b"abcde"[..]));
    }
}
