//! JSON lines over stdio, the contract with Allternit Desktop
//! (surfaces/allternit-desktop/src/main/phone-ios-runner.ts, `SideloadSigner`).
//! Keep the two in sync.
//!
//! Desktop → sidecar (stdin), one JSON object per line:
//!   {"op":"sign","apple_id","password"?,"udid","device_name","runner_ipa","host_ipa",
//!    "out_dir","anisette_url"?,"bundle_prefix"?}
//!   {"op":"two_factor","code":"123456"}
//!   {"op":"two_factor_sms"}              (text the code to the trusted number instead)
//!   {"op":"cancel"}                      (closing stdin does the same)
//!
//! Sidecar → Desktop (stdout):
//!   {"event":"progress","fraction":0.4,"step":"Registering this iPhone"}
//!   {"event":"two_factor_required","sms":false,"phone":"•••• 12","retry_reason"?}
//!     (sms false + phone: the code went to trusted devices; that number can get a text instead)
//!   {"event":"done","runner_ipa","host_ipa","bundle_prefix","runner_bundle_id",
//!    "host_bundle_id","team_id","expires_at","session_reused"}
//!   {"event":"error","code":"…","message":"…"}
//!
//! `password` may be omitted (or empty) when a saved Apple session for that
//! Apple ID is still valid; if it isn't, the sidecar answers
//! `{"event":"error","code":"password_required"}`.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use zeroize::Zeroizing;

use crate::bundle_ids;
use crate::errors::{ErrorCode, Failure};

/// A secret string (password, 2FA code). Zeroed on drop, never printed.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Secret(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // The String is moved (not copied) into the Zeroizing wrapper.
        String::deserialize(deserializer).map(Secret::new)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Sign(SignRequest),
    TwoFactor { code: Secret },
    /// Text the code to the account's trusted phone number instead.
    TwoFactorSms,
    Cancel,
}

/// What Desktop answers a 2FA prompt with.
#[derive(Debug)]
pub enum TwoFactorInput {
    Code(Secret),
    Text,
}

#[derive(Debug, Deserialize)]
pub struct SignRequest {
    pub apple_id: String,
    #[serde(default)]
    pub password: Option<Secret>,
    pub udid: String,
    pub device_name: String,
    pub runner_ipa: PathBuf,
    pub host_ipa: PathBuf,
    pub out_dir: PathBuf,
    /// Overrides --anisette-url / ALLTERNIT_ANISETTE_URL for this request.
    #[serde(default)]
    pub anisette_url: Option<String>,
    /// The bundle-id prefix the unsigned build carries (manifest.json
    /// `bundlePrefix`); default `com.allternit.phonelink`.
    #[serde(default)]
    pub bundle_prefix: Option<String>,
}

impl SignRequest {
    /// The password, if one was sent and isn't empty.
    pub fn password(&self) -> Option<&Secret> {
        self.password.as_ref().filter(|p| !p.is_empty())
    }

    pub fn source_prefix(&self) -> &str {
        self.bundle_prefix.as_deref().unwrap_or(bundle_ids::DEFAULT_SOURCE_PREFIX)
    }

    pub fn validate(&self) -> Result<(), Failure> {
        let bad = |what: &str| Err(Failure::new(ErrorCode::InvalidRequest, format!("Invalid {what}.")));
        let id = self.apple_id.trim();
        if id.is_empty() || id.len() > 254 || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return bad("Apple ID");
        }
        let udid_ok = (24..=40).contains(&self.udid.len()) && self.udid.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !udid_ok {
            return bad("device UDID");
        }
        if self.device_name.trim().is_empty() || self.device_name.len() > 128 || self.device_name.chars().any(char::is_control) {
            return bad("device name");
        }
        for (what, p) in [("runner_ipa", &self.runner_ipa), ("host_ipa", &self.host_ipa), ("out_dir", &self.out_dir)] {
            if !p.is_absolute() {
                return bad(what);
            }
        }
        if let Some(url) = &self.anisette_url {
            if !is_allowed_anisette_url(url) {
                return bad("anisette_url (must be https, or http on 127.0.0.1/localhost)");
            }
        }
        if !bundle_ids::is_valid_bundle_id(self.source_prefix()) {
            return bad("bundle_prefix");
        }
        Ok(())
    }
}

/// https anywhere; plain http only to this machine (a local anisette server).
pub fn is_allowed_anisette_url(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty() && !rest.starts_with('/');
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        return host == "127.0.0.1" || host == "localhost";
    }
    false
}

/// Parse one stdin line. Errors never echo the line (it may hold a password).
pub fn parse_request(line: &str) -> Result<Request, Failure> {
    let request: Request = serde_json::from_str(line.trim()).map_err(|e| {
        // serde_json's message names the field/position, never the value.
        Failure::new(ErrorCode::InvalidRequest, format!("Malformed request: {}", e.classify_message()))
    })?;
    match &request {
        Request::Sign(sign) => sign.validate()?,
        Request::TwoFactor { code } => {
            if !is_valid_two_factor_code(code.expose()) {
                return Err(Failure::new(ErrorCode::TwoFactorFailed, "The verification code must be 6 digits."));
            }
        }
        Request::TwoFactorSms | Request::Cancel => {}
    }
    Ok(request)
}

trait ClassifyMessage {
    fn classify_message(&self) -> &'static str;
}

impl ClassifyMessage for serde_json::Error {
    fn classify_message(&self) -> &'static str {
        use serde_json::error::Category;
        match self.classify() {
            Category::Io => "read error",
            Category::Syntax => "not valid JSON",
            Category::Data => "unknown op or missing/invalid field",
            Category::Eof => "truncated JSON",
        }
    }
}

pub fn is_valid_two_factor_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Done {
    pub runner_ipa: String,
    pub host_ipa: String,
    pub bundle_prefix: String,
    pub runner_bundle_id: String,
    pub host_bundle_id: String,
    pub team_id: String,
    /// RFC 3339; the earlier of the two profiles' expiry (free accounts: 7 days).
    pub expires_at: Option<String>,
    /// true when a saved Apple session was used (no password, no 2FA).
    pub session_reused: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Progress {
        fraction: f32,
        step: String,
    },
    TwoFactorRequired {
        sms: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        phone: Option<String>,
        /// Set when Apple rejected the previous code and wants another.
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_reason: Option<String>,
    },
    Done(Done),
    Error {
        code: ErrorCode,
        message: String,
    },
}

impl Event {
    pub fn progress(fraction: f32, step: &str) -> Self {
        Event::Progress { fraction, step: step.to_string() }
    }

    pub fn to_line(&self) -> String {
        // Serializing these plain structs can't fail.
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"event":"error","code":"signing_failed","message":"internal"}"#.to_string())
    }
}

impl From<Failure> for Event {
    fn from(f: Failure) -> Self {
        Event::Error { code: f.code, message: f.message }
    }
}

/// "•••• 12" from Apple's trusted-number record (Apple already masks the rest).
pub fn mask_phone(last_two_digits: &str) -> Option<String> {
    let digits: String = last_two_digits.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        Some(format!("•••• {}", &digits[digits.len().saturating_sub(2)..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID: &str = "00008110-001A2B3C4D5E801E";

    fn sign_line(extra: &str) -> String {
        format!(
            r#"{{"op":"sign","apple_id":"me@example.com","password":"hunter2","udid":"{UDID}","device_name":"Eoj's iPhone","runner_ipa":"/tmp/r.ipa","host_ipa":"/tmp/h.ipa","out_dir":"/tmp/out"{extra}}}"#
        )
    }

    #[test]
    fn parses_sign_request_exactly_as_desktop_sends_it() {
        let req = parse_request(&sign_line(r#","anisette_url":"https://anisette.allternit.com""#)).unwrap();
        let Request::Sign(s) = req else { panic!("not sign") };
        assert_eq!(s.apple_id, "me@example.com");
        assert_eq!(s.password().unwrap().expose(), "hunter2");
        assert_eq!(s.udid, UDID);
        assert_eq!(s.runner_ipa, PathBuf::from("/tmp/r.ipa"));
        assert_eq!(s.anisette_url.as_deref(), Some("https://anisette.allternit.com"));
        assert_eq!(s.source_prefix(), "com.allternit.phonelink");
    }

    #[test]
    fn desktop_omits_undefined_anisette_url() {
        // JSON.stringify drops `undefined`, so the key may be missing entirely.
        let Request::Sign(s) = parse_request(&sign_line("")).unwrap() else { panic!() };
        assert!(s.anisette_url.is_none());
    }

    #[test]
    fn password_is_optional_and_empty_means_none() {
        let line = sign_line("").replace(r#""password":"hunter2","#, "");
        let Request::Sign(s) = parse_request(&line).unwrap() else { panic!() };
        assert!(s.password().is_none());
        let line = sign_line("").replace("hunter2", "");
        let Request::Sign(s) = parse_request(&line).unwrap() else { panic!() };
        assert!(s.password().is_none());
    }

    #[test]
    fn parses_two_factor_and_cancel() {
        match parse_request(r#"{"op":"two_factor","code":"123456"}"#).unwrap() {
            Request::TwoFactor { code } => assert_eq!(code.expose(), "123456"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_request(r#"{"op":"cancel"}"#).unwrap(), Request::Cancel));
    }

    #[test]
    fn rejects_bad_two_factor_codes() {
        for code in ["12345", "1234567", "12a456", ""] {
            let err = parse_request(&format!(r#"{{"op":"two_factor","code":"{code}"}}"#)).unwrap_err();
            assert_eq!(err.code, ErrorCode::TwoFactorFailed);
        }
    }

    #[test]
    fn rejects_unknown_ops_and_garbage() {
        assert_eq!(parse_request(r#"{"op":"install"}"#).unwrap_err().code, ErrorCode::InvalidRequest);
        assert_eq!(parse_request("not json").unwrap_err().code, ErrorCode::InvalidRequest);
        assert_eq!(parse_request(r#"{"op":"sign"}"#).unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn validates_fields() {
        let cases = [
            sign_line("").replace(UDID, "../../etc"),
            sign_line("").replace("/tmp/r.ipa", "relative/r.ipa"),
            sign_line("").replace("me@example.com", "me @example.com"),
            sign_line(r#","anisette_url":"http://evil.example""#),
            sign_line(r#","anisette_url":"ftp://x""#),
            sign_line(r#","bundle_prefix":"com..bad""#),
        ];
        for line in cases {
            assert_eq!(parse_request(&line).unwrap_err().code, ErrorCode::InvalidRequest, "{line}");
        }
    }

    #[test]
    fn anisette_url_policy() {
        assert!(is_allowed_anisette_url("https://anisette.allternit.com"));
        assert!(is_allowed_anisette_url("http://127.0.0.1:6969"));
        assert!(is_allowed_anisette_url("http://localhost:6969/"));
        assert!(!is_allowed_anisette_url("http://127.0.0.1.evil.com"));
        assert!(!is_allowed_anisette_url("https://"));
    }

    #[test]
    fn errors_never_echo_the_password() {
        // Broken JSON that contains the password must not come back in the message.
        let line = r#"{"op":"sign","password":"hunter2-secret","#;
        let err = parse_request(line).unwrap_err();
        assert!(!err.message.contains("hunter2"), "{}", err.message);
        let line = sign_line("").replace(UDID, "bad");
        let err = parse_request(&line).unwrap_err();
        assert!(!err.message.contains("hunter2"));
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let Request::Sign(s) = parse_request(&sign_line("")).unwrap() else { panic!() };
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("hunter2"), "{dbg}");
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn events_serialize_to_the_desktop_shape() {
        let line = Event::progress(0.4, "Registering this iPhone").to_line();
        assert_eq!(line, r#"{"event":"progress","fraction":0.4,"step":"Registering this iPhone"}"#);
        let line = Event::TwoFactorRequired { sms: true, phone: mask_phone("12"), retry_reason: None }.to_line();
        assert_eq!(line, r#"{"event":"two_factor_required","sms":true,"phone":"•••• 12"}"#);
        let line = Event::from(Failure::new(ErrorCode::AppIdLimit, "x")).to_line();
        assert_eq!(line, r#"{"event":"error","code":"app_id_limit","message":"x"}"#);
        let done = Done {
            runner_ipa: "/o/runner-signed.ipa".into(),
            host_ipa: "/o/host-signed.ipa".into(),
            bundle_prefix: "com.allternit.phonelink.abcde12345".into(),
            runner_bundle_id: "com.allternit.phonelink.abcde12345.ictrl.uitests.xctrunner".into(),
            host_bundle_id: "com.allternit.phonelink.abcde12345.ictrl".into(),
            team_id: "ABCDE12345".into(),
            expires_at: Some("2026-10-23T12:00:00Z".into()),
            session_reused: false,
        };
        let v: serde_json::Value = serde_json::from_str(&Event::Done(done).to_line()).unwrap();
        assert_eq!(v["event"], "done");
        for key in ["runner_ipa", "host_ipa", "bundle_prefix", "runner_bundle_id", "host_bundle_id", "team_id", "expires_at"] {
            assert!(v[key].is_string(), "{key}");
        }
    }

    #[test]
    fn masks_phone_numbers() {
        assert_eq!(mask_phone("12").as_deref(), Some("•••• 12"));
        assert_eq!(mask_phone("").as_deref(), None);
    }
}
