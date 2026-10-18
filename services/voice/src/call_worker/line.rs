//! Which participant is the caller, the call's attributes, and the call's line
//! (CONTRACT-linked-phone-voice-2026-10-18 C1/C2). Pure logic so it's tested
//! without LiveKit; [`super::room`] applies it.
//!
//! A call room's caller is one of:
//! - the LiveKit SIP participant (a Telnyx number call);
//! - a Standard participant `linked-phone-<deviceId>`: Allternit Desktop
//!   bridging a bot's real phone through an audio cable;
//! - for an in-app call a bot auto-answers (`source:"inapp"`), the ringing
//!   user `user-<callerUserId>` (`inapp_calls::ring` mints that identity).

use std::collections::HashMap;

use serde_json::{json, Value};

use super::controls::Control;

/// Identity prefix of the Desktop participant that carries a linked phone's audio.
pub const LINKED_PHONE_PREFIX: &str = "linked-phone-";
/// Identity prefix cloud-api gives app users in in-app call rooms.
pub const INAPP_USER_PREFIX: &str = "user-";

/// `line.kind` of a call on a bot's linked real phone.
pub const LINKED_PHONE_KIND: &str = "linked_phone";

/// Is this participant the caller? `attrs` are the job's attributes merged with
/// its metadata ([`merge_metadata`]); they pick out the in-app caller.
pub fn is_caller(is_sip: bool, identity: &str, attrs: &HashMap<String, String>) -> bool {
    if is_sip || identity.starts_with(LINKED_PHONE_PREFIX) {
        return true;
    }
    if attrs.get("source").map(String::as_str) != Some("inapp") {
        return false;
    }
    match attrs.get("callerUserId").filter(|u| !u.is_empty()) {
        Some(user) => identity.strip_prefix(INAPP_USER_PREFIX) == Some(user.as_str()),
        None => identity.starts_with(INAPP_USER_PREFIX),
    }
}

/// The job's attributes on top of its metadata. cloud-api's
/// `create_room_with_agent` passes the call's fields as job metadata (a JSON
/// object); SIP dispatch rules set attributes. On a conflict the attribute wins.
/// String values are taken as-is, other values (the `line` object, numbers,
/// booleans) as compact JSON; nulls are skipped. Metadata that isn't a JSON
/// object is ignored.
pub fn merge_metadata(attributes: &HashMap<String, String>, metadata: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(metadata) {
        for (k, v) in map {
            match v {
                Value::Null => {}
                Value::String(s) => {
                    out.insert(k, s);
                }
                other => {
                    out.insert(k, other.to_string());
                }
            }
        }
    }
    out.extend(attributes.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

/// The call's line (C1). A `line` attribute holding a valid line object wins;
/// otherwise a number call gets `{kind:"number", numberId}`. `None` when the
/// call has neither (an in-app call).
pub fn call_line(get: &dyn Fn(&str) -> Option<String>) -> Option<Value> {
    let parsed = get("line").and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(valid_line);
    parsed.or_else(|| get("numberId").map(|n| json!({ "kind": "number", "numberId": n })))
}

fn valid_line(v: &Value) -> bool {
    let nonempty = |k: &str| v.get(k).and_then(Value::as_str).is_some_and(|s| !s.is_empty());
    match v.get("kind").and_then(Value::as_str) {
        Some("number") => nonempty("numberId"),
        Some(LINKED_PHONE_KIND) => nonempty("deviceId"),
        _ => false,
    }
}

pub fn is_linked_phone(line: Option<&Value>) -> bool {
    line.and_then(|l| l.get("kind")).and_then(Value::as_str) == Some(LINKED_PHONE_KIND)
}

/// The control's action name when the line can't do it (C2: DTMF, transfer and
/// hold aren't supported on a linked phone in v1). `None` = apply it.
pub fn unsupported_control(line: Option<&Value>, control: &Control) -> Option<&'static str> {
    if !is_linked_phone(line) {
        return None;
    }
    match control {
        Control::Dtmf(_) => Some("dtmf"),
        Control::Transfer { .. } => Some("transfer"),
        Control::Hold => Some("hold"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_worker::controls::Target;
    use crate::call_worker::events::TransferMode;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn caller_is_sip_or_linked_phone() {
        let none = HashMap::new();
        assert!(is_caller(true, "sip_+15551230000", &none));
        assert!(is_caller(false, "linked-phone-ZY22ABC", &none));
        assert!(!is_caller(false, "human-u1", &none));
        assert!(!is_caller(false, "listener-u1-abc", &none));
        assert!(!is_caller(false, "user-u1", &none));
    }

    #[test]
    fn inapp_caller_is_the_ringing_user() {
        let inapp = map(&[("source", "inapp"), ("callerUserId", "u1")]);
        assert!(is_caller(false, "user-u1", &inapp));
        assert!(!is_caller(false, "user-u2", &inapp));
        assert!(!is_caller(false, "human-u1", &inapp));
        // Older metadata without callerUserId: any app user.
        let bare = map(&[("source", "inapp")]);
        assert!(is_caller(false, "user-u9", &bare));
        assert!(!is_caller(false, "human-u9", &bare));
    }

    #[test]
    fn metadata_merges_under_attributes() {
        let meta = r#"{"source":"linked_phone","botId":"b1","ownerId":"o1","direction":"inbound",
            "from":"+15551230000","to":null,"line":{"kind":"linked_phone","deviceId":"ZY22","label":"Moto","platform":"android"}}"#;
        let attrs = map(&[("botId", "b-attr")]);
        let m = merge_metadata(&attrs, meta);
        assert_eq!(m.get("botId").map(String::as_str), Some("b-attr"));
        assert_eq!(m.get("source").map(String::as_str), Some("linked_phone"));
        assert_eq!(m.get("from").map(String::as_str), Some("+15551230000"));
        assert!(!m.contains_key("to"));
        let line: Value = serde_json::from_str(m.get("line").unwrap()).unwrap();
        assert_eq!(line["deviceId"], "ZY22");
    }

    #[test]
    fn bad_metadata_is_ignored() {
        let attrs = map(&[("numberId", "n1")]);
        for meta in ["", "not json", "[1,2]", "\"str\""] {
            assert_eq!(merge_metadata(&attrs, meta), attrs);
        }
        let m = merge_metadata(&HashMap::new(), r#"{"n":3,"ok":true}"#);
        assert_eq!((m["n"].as_str(), m["ok"].as_str()), ("3", "true"));
    }

    fn line_of(pairs: &[(&str, &str)]) -> Option<Value> {
        let m = map(pairs);
        call_line(&|k: &str| m.get(k).cloned().filter(|v| !v.is_empty()))
    }

    #[test]
    fn builds_the_line() {
        assert_eq!(line_of(&[("numberId", "n1")]), Some(json!({"kind": "number", "numberId": "n1"})));
        let lp = r#"{"kind":"linked_phone","deviceId":"ZY22","serial":"emulator-5554","label":"Moto","platform":"android"}"#;
        let line = line_of(&[("line", lp), ("numberId", "n1")]).unwrap();
        assert!(is_linked_phone(Some(&line)));
        assert_eq!(line["serial"], "emulator-5554");
        // Invalid line objects fall back to the number, else nothing.
        assert_eq!(line_of(&[("line", r#"{"kind":"linked_phone"}"#), ("numberId", "n1")]).unwrap()["kind"], "number");
        assert_eq!(line_of(&[("line", "garbage")]), None);
        assert_eq!(line_of(&[]), None);
        assert!(!is_linked_phone(None));
    }

    #[test]
    fn linked_phone_refuses_dtmf_transfer_hold() {
        let lp = json!({"kind": "linked_phone", "deviceId": "ZY22", "label": "Moto", "platform": "android"});
        let num = json!({"kind": "number", "numberId": "n1"});
        let transfer = Control::Transfer { to: "+15105550100".into(), mode: TransferMode::Warm, consent_ref: None };
        assert_eq!(unsupported_control(Some(&lp), &Control::Dtmf("1".into())), Some("dtmf"));
        assert_eq!(unsupported_control(Some(&lp), &transfer), Some("transfer"));
        assert_eq!(unsupported_control(Some(&lp), &Control::Hold), Some("hold"));
        for ok in [Control::Hangup, Control::Mute(Target::Bot), Control::Listen, Control::Resume] {
            assert_eq!(unsupported_control(Some(&lp), &ok), None);
        }
        assert_eq!(unsupported_control(Some(&num), &Control::Hold), None);
        assert_eq!(unsupported_control(None, &transfer), None);
    }
}
