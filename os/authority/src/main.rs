//! AllternitOS authority service. PID 1 starts it under its own uid; clients ask `check` over a
//! unix socket. Decisions come from the `allternit-authority` crate's policy table
//! (`policy::class_rule`, CONTRACT.md 4.2), not from a local rule list.
//!
//! This service has no approval-token store, receipts or signing yet, so it serves `check` only:
//! a human-only class is DENY (never ALLOW without a redeemed token), an `ask` row is ASK, and every
//! other call fails closed with `kernel_unavailable`.
//!
//! Two protocols on one socket, told apart by the first byte:
//!  * Line protocol (init's self-check), first byte `{`. One JSON object per line.
//!    request:  {"op":"check","class":"read"[,"autonomy_level":"ask_first"]}
//!    response: {"decision":"allow"|"deny","reason":"...","rule":"allow"|"ask"|"human_only"}
//!    An `ask` row is "deny" here: nothing on this path can answer an approval.
//!  * Framed protocol (contract section 2.0), anything else: a 4-byte big-endian length, then the
//!    envelope {"v":1,"id","call":"check","caller","body"}. The class is
//!    body.extensions["x-os-v1"].action_class (the legacy key "os.v1" is still read). A missing
//!    autonomy_level means ask_first, the strictest.

use allternit_authority::calls::KernelResponse;
use allternit_authority::framing::{read_frame, write_frame, FrameError};
use allternit_authority::policy::{class_rule, Rule};
use allternit_authority::{
    ActionClass, AuthorityError, AutonomyLevel, ErrorCode, Response, OS_EXTENSION_KEY, PROTOCOL_VERSION,
};
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

/// Extension key the first boot shell sent, before the contract fixed it as `x-os-v1`.
const LEGACY_EXTENSION_KEY: &str = "os.v1";

/// Policy for one (class, autonomy) pair. Unknown class or autonomy value is an error (deny).
fn rule_for(class: Option<&Value>, autonomy: Option<&Value>) -> Result<(ActionClass, Rule), String> {
    let class = class.ok_or("missing class")?;
    let class: ActionClass =
        serde_json::from_value(class.clone()).map_err(|_| format!("unknown action class {class}"))?;
    let autonomy = match autonomy {
        None | Some(Value::Null) => AutonomyLevel::AskFirst,
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| format!("unknown autonomy level {v}"))?,
    };
    Ok((class, class_rule(class, autonomy)))
}

fn class_name(class: ActionClass) -> String {
    serde_json::to_value(class).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn rule_name(rule: Rule) -> &'static str {
    match rule {
        Rule::Allow => "allow",
        Rule::Ask => "ask",
        Rule::HumanOnly => "human_only",
    }
}

fn reason(class: ActionClass, rule: Rule) -> String {
    let c = class_name(class);
    match rule {
        Rule::Allow => format!("class {c} is allowed by policy"),
        Rule::Ask => format!("class {c} needs an approval"),
        Rule::HumanOnly => format!("class {c} needs human approval"),
    }
}

fn decide(line: &str) -> Value {
    let deny = |why: &str| json!({"decision":"deny","reason":why,"rule":"human_only"});
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return deny("malformed request"),
    };
    if req.get("op").and_then(Value::as_str) != Some("check") {
        return deny("unsupported op");
    }
    match rule_for(req.get("class"), req.get("autonomy_level")) {
        Ok((class, rule)) => {
            let decision = if rule == Rule::Allow { "allow" } else { "deny" };
            json!({"decision":decision,"reason":reason(class, rule),"rule":rule_name(rule)})
        }
        Err(why) => deny(&why),
    }
}

fn to_value(r: KernelResponse) -> Value {
    serde_json::to_value(r).unwrap_or(Value::Null)
}

fn error_frame(id: String, error: AuthorityError) -> Value {
    to_value(KernelResponse::from_result(id, Err(error)))
}

fn bad(id: String, code: ErrorCode, message: &str) -> Value {
    error_frame(id, AuthorityError::without_receipt(code, message))
}

/// One framed request envelope to one response envelope.
fn decide_framed(req: &Value) -> Value {
    let id = req.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    if req.get("v") != Some(&json!(PROTOCOL_VERSION)) {
        return bad(id, ErrorCode::UnsupportedVersion, "only protocol v1 is served");
    }
    match req.get("call").and_then(Value::as_str) {
        Some("check") => {}
        Some(other) => {
            let msg = format!("call {other} is not served by the boot authority (check only)");
            return error_frame(id, AuthorityError::kernel_unavailable(msg));
        }
        None => return bad(id, ErrorCode::BadRequest, "missing call"),
    }
    let exts = req.pointer("/body/extensions");
    let ext = [OS_EXTENSION_KEY, LEGACY_EXTENSION_KEY].iter().find_map(|k| exts.and_then(|e| e.get(*k)));
    let Some(ext) = ext else {
        return bad(id, ErrorCode::BadRequest, "missing os.v1 check extension");
    };
    match rule_for(ext.get("action_class"), ext.get("autonomy_level")) {
        Ok((class, rule)) => {
            let decision = match rule {
                Rule::Allow => "ALLOW",
                Rule::Ask => "ASK",
                // No token store here, so a human-only class can never be redeemed: DENY.
                Rule::HumanOnly => "DENY",
            };
            let mut body = Map::new();
            body.insert("decision".into(), decision.into());
            body.insert("reason".into(), reason(class, rule).into());
            body.insert("receipt_id".into(), Value::Null);
            to_value(KernelResponse::from_result(id, Ok(Response::Check(body))))
        }
        Err(why) => bad(id, ErrorCode::BadRequest, &why),
    }
}

fn serve_framed(mut reader: BufReader<UnixStream>, out: &mut UnixStream) {
    loop {
        let reply = match read_frame::<_, Value>(&mut reader) {
            Ok(req) => decide_framed(&req),
            Err(FrameError::Json(_)) => bad(String::new(), ErrorCode::BadRequest, "not json"),
            Err(FrameError::TooLarge(_)) => {
                let _ = write_frame(out, &bad(String::new(), ErrorCode::FrameTooLarge, "max frame 1 MiB"));
                return;
            }
            Err(_) => return,
        };
        if write_frame(out, &reply).is_err() {
            return;
        }
    }
}

/// A first byte of `{` is the line protocol (init's self-check); anything else is a length frame.
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
        if writeln!(out, "{}", decide(&line)).is_err() {
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
            eprintln!("authority: cannot bind {path}: {e}");
            std::process::exit(1);
        }
    };
    // Clients (shell uid 902, init) must connect. SO_PEERCRED caller binding is a later step, so
    // the socket stays 0666 and the service holds no secrets.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    eprintln!("authority: policy from allternit-authority, listening on {path}");
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

    fn f(class: &str, level: Option<&str>) -> Value {
        let mut ext = json!({"action_class": class, "action_hash": null, "approval_token": null});
        if let Some(l) = level {
            ext["autonomy_level"] = json!(l);
        }
        let mut exts = Map::new();
        exts.insert(OS_EXTENSION_KEY.into(), ext);
        decide_framed(&json!({"v":1,"id":"1","call":"check","caller":{"kind":"service","principal":"s"},
            "body":{"extensions":exts}}))
    }

    #[test]
    fn line_allows_read_and_denies_human_only() {
        assert_eq!(d(r#"{"op":"check","class":"read"}"#), "allow");
        for c in ["money", "deploy", "client_message", "secret_new_host", "irreversible"] {
            assert_eq!(d(&format!(r#"{{"op":"check","class":"{c}"}}"#)), "deny");
        }
    }

    #[test]
    fn line_ask_rows_deny_and_app_install_asks_at_every_level() {
        assert_eq!(d(r#"{"op":"check","class":"file_write"}"#), "deny");
        for l in ["ask_first", "ask_risky", "full"] {
            let r = decide(&format!(r#"{{"op":"check","class":"app_install","autonomy_level":"{l}"}}"#));
            assert_eq!((r["decision"].as_str(), r["rule"].as_str()), (Some("deny"), Some("ask")));
        }
    }

    #[test]
    fn line_denies_malformed_and_unknown() {
        assert_eq!(d("nope"), "deny");
        assert_eq!(d(r#"{"op":"lease"}"#), "deny");
        assert_eq!(d(r#"{"op":"check"}"#), "deny");
        assert_eq!(d(r#"{"op":"check","class":"made_up"}"#), "deny");
    }

    #[test]
    fn framed_ui_layout_allow_money_deny_install_ask() {
        assert_eq!(f("ui_layout", Some("ask_first"))["body"]["decision"], "ALLOW");
        assert_eq!(f("money", Some("full"))["body"]["decision"], "DENY");
        assert_eq!(f("app_install", Some("full"))["body"]["decision"], "ASK");
        assert_eq!(f("ui_layout", None)["ok"], true);
    }

    #[test]
    fn framed_reads_the_legacy_key_and_fails_closed() {
        let legacy = decide_framed(&json!({"v":1,"id":"1","call":"check",
            "body":{"extensions":{"os.v1":{"action_class":"ui_layout","autonomy_level":"ask_first"}}}}));
        assert_eq!(legacy["body"]["decision"], "ALLOW");
        let other = decide_framed(&json!({"v":1,"id":"2","call":"lease.acquire","body":{}}));
        assert_eq!(other["ok"], false);
        assert_eq!(other["error"]["code"], "kernel_unavailable");
        assert_eq!(decide_framed(&json!({"v":2,"id":"3","call":"check"}))["error"]["code"], "unsupported_version");
        assert_eq!(f("made_up", None)["error"]["code"], "bad_request");
    }
}
