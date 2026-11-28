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
use std::io::{BufRead, BufReader, Write};
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

fn serve(stream: UnixStream) {
    let mut out = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    for line in BufReader::new(stream).lines() {
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
    fn denies_malformed() {
        assert_eq!(d("nope"), "deny");
        assert_eq!(d(r#"{"op":"lease"}"#), "deny");
        assert_eq!(d(r#"{"op":"check"}"#), "deny");
    }
}
