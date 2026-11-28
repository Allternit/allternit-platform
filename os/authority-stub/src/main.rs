//! STUB authority service for the AllternitOS boot image.
//!
//! This is NOT the real kernel interface. Plan step 4's `allternit-authority`
//! crate replaces it. It only proves the boot path: PID 1 starts it under its
//! own uid, and a client can ask `check` over a unix socket.
//!
//! Protocol: one JSON object per line.
//!   request:  {"op":"check","action":"...","class":"read"}
//!   response: {"decision":"allow"|"deny","reason":"...","stub":true}
//! Fixed rule: deny class money, deploy, client_message, secret_new_host,
//! irreversible. Everything else is allowed. Malformed requests are denied.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

const DENY_CLASSES: [&str; 5] = ["money", "deploy", "client_message", "secret_new_host", "irreversible"];

fn decide(line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return json!({"decision":"deny","reason":"malformed request","stub":true}),
    };
    if req.get("op").and_then(Value::as_str) != Some("check") {
        return json!({"decision":"deny","reason":"unsupported op","stub":true});
    }
    match req.get("class").and_then(Value::as_str) {
        Some(c) if DENY_CLASSES.contains(&c) => {
            json!({"decision":"deny","reason":format!("class {c} needs human approval"),"stub":true})
        }
        Some(_) => json!({"decision":"allow","reason":"stub rule: class not restricted","stub":true}),
        None => json!({"decision":"deny","reason":"missing class","stub":true}),
    }
}

/// Contract section 2.0 envelope: {"v":1,"id","call":"check","caller","body"}. The class is
/// body.extensions["os.v1"].action_class. Stub rule only; the real crate replaces this.
fn decide_framed(frame: &[u8]) -> Value {
    let req: Value = match serde_json::from_slice(frame) {
        Ok(v) => v,
        Err(_) => return json!({"v":1,"id":null,"ok":false,"error":{"code":"bad_request","message":"not json","receipt_id":null}}),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if req.get("v") != Some(&json!(1)) || req.get("call").and_then(Value::as_str) != Some("check") {
        return json!({"v":1,"id":id,"ok":false,"error":{"code":"bad_request","message":"stub supports v1 check only","receipt_id":null}});
    }
    let class = req.pointer("/body/extensions/os.v1/action_class").and_then(Value::as_str);
    let (decision, reason) = match class {
        Some(c) if DENY_CLASSES.contains(&c) => ("DENY", format!("class {c} needs human approval")),
        Some(_) => ("ALLOW", "stub rule: class not restricted".to_string()),
        None => ("DENY", "missing action_class".to_string()),
    };
    json!({"v":1,"id":id,"ok":true,"body":{"decision":decision,"reason":reason,"receipt_id":null,"stub":true}})
}

fn serve_framed<R: Read>(mut r: R, out: &mut UnixStream) {
    loop {
        let mut len = [0u8; 4];
        if r.read_exact(&mut len).is_err() {
            return;
        }
        let n = u32::from_be_bytes(len) as usize;
        let reply = if n > 1 << 20 {
            json!({"v":1,"id":null,"ok":false,"error":{"code":"frame_too_large","message":"max frame 1 MiB","receipt_id":null}})
        } else {
            let mut buf = vec![0u8; n];
            if r.read_exact(&mut buf).is_err() {
                return;
            }
            decide_framed(&buf)
        };
        let bytes = reply.to_string().into_bytes();
        if out.write_all(&(bytes.len() as u32).to_be_bytes()).is_err() || out.write_all(&bytes).is_err() {
            return;
        }
        if n > 1 << 20 {
            return;
        }
    }
}

/// A first byte of `{` is the old line protocol (init's self-check); anything else is a
/// 4-byte big-endian length frame (contract section 2.0).
fn serve(stream: UnixStream) {
    let mut out = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    let first = match reader.fill_buf() {
        Ok(b) if !b.is_empty() => b[0],
        _ => return,
    };
    if first != b'{' {
        return serve_framed(reader, &mut out);
    }
    for line in reader.lines() {
        let Ok(line) = line else { return };
        let reply = decide(&line);
        if writeln!(out, "{reply}").is_err() {
            return;
        }
    }
}

fn main() {
    let mut path = "/run/allternit/authority.sock".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--socket" {
            if let Some(p) = args.next() {
                path = p;
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("authority-stub: cannot bind {path}: {e}");
            std::process::exit(1);
        }
    };
    // Clients (shell uid 902, init) must connect; the stub holds no secrets.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    eprintln!("authority-stub: STUB listening on {path}");
    for conn in listener.incoming() {
        if let Ok(c) = conn {
            std::thread::spawn(move || serve(c));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> String {
        decide(s)["decision"].as_str().unwrap().to_string()
    }

    #[test]
    fn allows_ordinary_and_denies_restricted() {
        assert_eq!(d(r#"{"op":"check","class":"read"}"#), "allow");
        for c in DENY_CLASSES {
            assert_eq!(d(&format!(r#"{{"op":"check","class":"{c}"}}"#)), "deny");
        }
    }

    #[test]
    fn framed_check_allows_ui_layout_and_denies_money() {
        let f = |c: &str| {
            let r = json!({"v":1,"id":"1","call":"check","caller":{"kind":"service","principal":"s"},
                "body":{"extensions":{"os.v1":{"action_class":c}}}});
            decide_framed(r.to_string().as_bytes())["body"]["decision"].as_str().unwrap().to_string()
        };
        assert_eq!(f("ui_layout"), "ALLOW");
        assert_eq!(f("money"), "DENY");
    }

    #[test]
    fn denies_malformed() {
        assert_eq!(d("nope"), "deny");
        assert_eq!(d(r#"{"op":"lease"}"#), "deny");
        assert_eq!(d(r#"{"op":"check"}"#), "deny");
    }
}
