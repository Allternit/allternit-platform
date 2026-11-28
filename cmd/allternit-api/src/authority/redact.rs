//! The one secret redactor for tool results (CONTRACT.md 2.3, plan step 10).
//!
//! Every tool result in allternit-api passes through a [`Redactor`] before it can reach model
//! context: `tool_routes::execute_tool_internal`, `mcp_dispatcher::dispatch_call` and the computer
//! toolset results (`computer_toolset::execute`). ACI run frames use it through
//! `aci_credentials::scrub_frame`, which it generalises.
//!
//! It masks two things, in this order:
//!
//! 1. **The exact values the kernel attached during the call** (and their base64 form): vault
//!    secrets screened for this account, credentials bound by `secret.use`, header values sent to
//!    an MCP server. Longest first, so a value that contains another is masked whole.
//! 2. **Credential-shaped strings**, attached or not: provider key prefixes (`sk-`, `ghp_`,
//!    `gho_`, `xox?-`, `AKIA`, `AIza`, `sk_live_`), PEM private key blocks, JWTs, bearer/basic
//!    header values, passwords in URLs, and long or high-entropy values assigned to a key named
//!    like a credential (`password=`, `"api_key": `, `GITHUB_TOKEN=`).
//!
//! An approved read of a `.env` or credential file additionally masks every assignment value
//! ([`Redactor::credential_file`]).
//!
//! The gizzi port is `cmd/gizzi-code/src/shared/util/secret-redactor.ts`. Both are tested against
//! `spec/Contracts/os/v1/conformance/redaction-vectors.json`; change the rules in both, with a
//! vector.

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use serde_json::Value;

/// What a masked value becomes.
pub const MASK: &str = "[REDACTED]";

/// Attached values shorter than this are not masked (they would mangle ordinary text).
const MIN_VALUE_LEN: usize = 3;
/// Attached values at least this long are also masked in their base64 form.
const MIN_B64_LEN: usize = 8;

fn re(p: &str) -> Regex {
    Regex::new(p).expect("redaction rule compiles")
}

/// Whole-match rules: the match is replaced by [`MASK`].
static WHOLE: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // PEM private keys, with or without the END line (a truncated read still holds key bytes).
        re(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|$)"),
        // OpenAI / Anthropic (`sk-proj-`, `sk-ant-`, `sk-`).
        re(r"\bsk-[A-Za-z0-9][A-Za-z0-9_\-]{19,}"),
        // GitHub tokens and fine-grained PATs.
        re(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})"),
        // Slack.
        re(r"\bxox[abposr]-[A-Za-z0-9\-]{10,}"),
        // AWS access key ids.
        re(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        // Google API keys.
        re(r"\bAIza[0-9A-Za-z_\-]{35}"),
        // Stripe secret and restricted keys.
        re(r"\b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{16,}"),
        // JWTs.
        re(r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}"),
    ]
});

/// `Bearer <token>` / `Basic <token>`: group 2 is masked.
static BEARER: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b((?:bearer|basic)\s+)([A-Za-z0-9_\-.~+/]{16,}=*)"));

/// `scheme://user:password@host`: group 2 is masked.
static URL_CREDS: Lazy<Regex> = Lazy::new(|| re(r"\b([A-Za-z][A-Za-z0-9+.\-]*://[^/\s:@]+:)([^@\s/]{3,})@"));

/// `<credential-named key><sep><value>`: group 3 is masked when [`assignment_is_secret`] says so.
static ASSIGNMENT: Lazy<Regex> = Lazy::new(|| {
    re(r#"(?i)\b([A-Za-z0-9_.\-]*(?:api[_\-]?key|apikey|secret|token|passwd|password|pwd|credential|private[_\-]?key|access[_\-]?key|auth[_\-]?token|authorization)[A-Za-z0-9_.\-]*)(["']?[ \t]*[:=][ \t]*["']?)([^\s"'`,;&<>]+)"#)
});

/// One `KEY=value` / `key: value` line of a credential file: group 2 is masked.
static FILE_LINE: Lazy<Regex> =
    Lazy::new(|| re(r"(?m)^([ \t]*(?:export[ \t]+)?[A-Za-z_][A-Za-z0-9_.\-]*[ \t]*[=:][ \t]*)(.+)$"));

fn char_classes(v: &str) -> usize {
    let lower = v.chars().any(|c| c.is_ascii_lowercase());
    let upper = v.chars().any(|c| c.is_ascii_uppercase());
    let digit = v.chars().any(|c| c.is_ascii_digit());
    let other = v.chars().any(|c| !c.is_ascii_alphanumeric());
    [lower, upper, digit, other].iter().filter(|b| **b).count()
}

/// Does `value`, assigned to `key`, look like a secret? Password-like keys mask any value of six or
/// more characters; token/key-like keys mask long, mixed values (high entropy). Paths, URLs, plain
/// numbers and literals stay, so `PWD=/home/x`, `max_tokens=4096` and `password: true` read as is.
pub fn assignment_is_secret(key: &str, value: &str) -> bool {
    let v = value.trim();
    if v == MASK || v.contains("://") || v.starts_with('/') || v.starts_with('~') {
        return false;
    }
    if matches!(v.to_ascii_lowercase().as_str(), "true" | "false" | "null" | "none" | "undefined") {
        return false;
    }
    if v.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    let k = key.to_ascii_lowercase();
    let password_like = ["passw", "pwd", "secret", "credential", "private"].iter().any(|w| k.contains(w));
    if password_like {
        v.chars().count() >= 6
    } else {
        v.chars().count() >= 16 && char_classes(v) >= 2
    }
}

/// The credential-shape rules alone (no attached values).
pub fn mask_shapes(text: &str) -> String {
    let mut out = text.to_string();
    for rule in WHOLE.iter() {
        if rule.is_match(&out) {
            out = rule.replace_all(&out, MASK).into_owned();
        }
    }
    out = BEARER.replace_all(&out, |c: &Captures| format!("{}{MASK}", &c[1])).into_owned();
    out = URL_CREDS.replace_all(&out, |c: &Captures| format!("{}{MASK}@", &c[1])).into_owned();
    out = ASSIGNMENT
        .replace_all(&out, |c: &Captures| {
            if assignment_is_secret(&c[1], &c[3]) {
                format!("{}{}{MASK}", &c[1], &c[2])
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    out
}

/// True when `text` holds a credential-shaped string (used to refuse a value where a handle belongs).
pub fn looks_like_secret(text: &str) -> bool {
    mask_shapes(text) != text
}

/// Masks the attached values and credential shapes in tool results of one call.
#[derive(Clone, Default)]
pub struct Redactor {
    /// Longest first; plaintext and base64 forms. Never logged, never serialised.
    values: Vec<String>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redactor {{ values: {} }}", self.values.len())
    }
}

impl Redactor {
    /// A redactor for these attached values (plus the shape rules, always on).
    pub fn new<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out: Vec<String> = Vec::new();
        for v in values {
            let v = v.as_ref();
            if v.chars().count() < MIN_VALUE_LEN {
                continue;
            }
            out.push(v.to_string());
            if v.len() >= MIN_B64_LEN {
                out.push(B64.encode(v.as_bytes()));
            }
        }
        out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        out.dedup();
        Self { values: out }
    }

    /// Shape rules only.
    pub fn shapes_only() -> Self {
        Self::default()
    }

    /// Add more attached values (e.g. a credential `secret.use` bound mid-call).
    pub fn with<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut all = std::mem::take(&mut self.values);
        all.extend(values.into_iter().map(|v| v.as_ref().to_string()));
        Self::new(all)
    }

    /// Mask one string.
    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_string();
        for v in &self.values {
            if out.contains(v.as_str()) {
                out = out.replace(v.as_str(), MASK);
            }
        }
        mask_shapes(&out)
    }

    /// An approved read of a `.env` or credential file: [`Redactor::text`], then every
    /// assignment value on its own line.
    pub fn credential_file(&self, text: &str) -> String {
        let out = self.text(text);
        FILE_LINE
            .replace_all(&out, |c: &Captures| {
                let v = c[2].trim();
                if v.is_empty() || v == MASK {
                    c[0].to_string()
                } else {
                    format!("{}{MASK}", &c[1])
                }
            })
            .into_owned()
    }

    /// Mask every string in a JSON value, in place (object keys are left alone).
    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(s) => {
                let masked = self.text(s);
                if masked != *s {
                    *s = masked;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.value(v)),
            Value::Object(map) => map.values_mut().for_each(|v| self.value(v)),
            _ => {}
        }
    }

    /// A tool result (`Ok` value or `Err` message), masked.
    pub fn result(&self, r: Result<Value, String>) -> Result<Value, String> {
        match r {
            Ok(mut v) => {
                self.value(&mut v);
                Ok(v)
            }
            Err(e) => Err(self.text(&e)),
        }
    }
}

/// The credential paths gizzi asks about before a read (`credential-paths.ts`): `.env` files
/// (not `.env.example` / `.sample` / `.template`), SSH private keys (not `.pub`), and anything
/// named `credentials`.
pub fn is_credential_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    if [".env.example", ".env.sample", ".env.template"].iter().any(|s| name.ends_with(s)) {
        return false;
    }
    if name.ends_with(".pub") {
        return false;
    }
    name.ends_with(".env")
        || name.contains(".env.")
        || ["id_rsa", "id_ed25519", "id_ecdsa"].iter().any(|k| name.contains(k))
        || p.contains("credentials")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `spec/Contracts/os/v1/conformance/redaction-vectors.json`, shared with the gizzi tests.
    fn vectors() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../spec/Contracts/os/v1/conformance/redaction-vectors.json");
        serde_json::from_str(&std::fs::read_to_string(&path).expect("redaction vectors")).expect("vectors parse")
    }

    fn joined(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts.iter().filter_map(Value::as_str).collect(),
            _ => panic!("input must be a string or an array of strings"),
        }
    }

    #[test]
    fn shared_vectors_hold() {
        let doc = vectors();
        assert_eq!(doc["mask"], MASK);
        let list = doc["vectors"].as_array().expect("vectors");
        assert!(list.len() >= 20);
        for v in list {
            let name = v["name"].as_str().unwrap();
            let input = joined(&v["input"]);
            let secrets: Vec<String> = v["secrets"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
            let r = Redactor::new(&secrets);
            let out = match v["mode"].as_str().unwrap_or("tool_result") {
                "credential_file" => r.credential_file(&input),
                _ => r.text(&input),
            };
            for absent in v["absent"].as_array().unwrap() {
                let a = joined(absent);
                assert!(!out.contains(&a), "{name}: {a:?} still in {out:?}");
            }
            for present in v["present"].as_array().unwrap() {
                let p = joined(present);
                assert!(out.contains(&p), "{name}: {p:?} missing from {out:?}");
            }
            // Stable: a second pass changes nothing.
            assert_eq!(r.text(&out), out, "{name}: not stable");
        }
    }

    #[test]
    fn json_results_are_masked_in_every_string() {
        let secret = "vault-value-abcdef";
        let r = Redactor::new([secret]);
        let mut v = json!({ "stdout": format!("x {secret} y"), "nested": [{ "headers": format!("Bearer {secret}0123456789") }], "n": 3 });
        r.value(&mut v);
        assert!(!v.to_string().contains(secret), "{v}");
        assert_eq!(v["n"], 3);
        let err = r.result(Err(format!("failed with {secret}"))).unwrap_err();
        assert!(!err.contains(secret));
    }

    #[test]
    fn credential_paths_match_gizzi() {
        for p in ["/app/.env", "x/.env.local", "/home/u/.ssh/id_ed25519", "/home/u/.aws/credentials", "C:\\u\\.env"] {
            assert!(is_credential_path(p), "{p}");
        }
        for p in ["/app/.env.example", "/home/u/.ssh/id_rsa.pub", "/app/src/main.rs", "/app/environment.ts"] {
            assert!(!is_credential_path(p), "{p}");
        }
    }

    #[test]
    fn handle_shaped_refs_are_not_secrets() {
        assert!(!looks_like_secret("computer:vault:github-login"));
        assert!(looks_like_secret(&format!("{}{}", "ghp", "_TestOnlyNotARealToken000000000000abcd")));
    }
}
