//! Freeweights manifest v0: parse from exact bytes, check canonical form, verify
//! the minisign signature. Spec: freeweights `spec/manifest-v0.md`.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// The `freeweights-v0` minisign public key (key id [`FREEWEIGHTS_KEY_ID`]).
pub const FREEWEIGHTS_PUBLIC_KEY: &str = include_str!("../freeweights-v0.pub");
/// Key id of [`FREEWEIGHTS_PUBLIC_KEY`], as `minisign` prints it.
pub const FREEWEIGHTS_KEY_ID: &str = "2DC28CC7B26A283B";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub section: String,
    pub upstream: Upstream,
    pub license: License,
    pub files: Vec<ManifestFile>,
    pub sources: Vec<ManifestSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent: Option<Torrent>,
    pub created_at: String,
    pub generator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    pub host: String,
    pub repo: String,
    /// 40-hex upstream commit the manifest is pinned to.
    pub commit: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct License {
    pub id: String,
    pub name: String,
    pub file_path: Option<String>,
    pub file_sha256: Option<String>,
    /// `yes`, `no` or `click-through`.
    pub redistribution: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub lfs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSource {
    /// `hosted`, `seeded` or `source`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnet: Option<String>,
    pub priority: i64,
    pub last_checked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Torrent {
    pub infohash_v1: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infohash_v2: Option<String>,
    pub magnet: String,
    #[serde(default)]
    pub web_seeds: Vec<String>,
}

impl Manifest {
    pub fn file(&self, path: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// Verify `bytes` against a detached `.minisig` with the embedded Freeweights key,
/// then parse and validate the manifest. Any failure is an error; never fall back.
pub fn verify_manifest(bytes: &[u8], minisig: &str) -> Result<Manifest, Error> {
    verify_manifest_with_key(bytes, minisig, FREEWEIGHTS_PUBLIC_KEY)
}

/// Same as [`verify_manifest`] with an explicit minisign public key (the
/// `.pub` file text, or just its base64 line). For key rotation.
pub fn verify_manifest_with_key(
    bytes: &[u8],
    minisig: &str,
    public_key: &str,
) -> Result<Manifest, Error> {
    let key_line = key_line(public_key);
    let pk = minisign_verify::PublicKey::from_base64(key_line)
        .map_err(|e| Error::Signature(format!("bad public key: {e}")))?;
    let sig = minisign_verify::Signature::decode(minisig)
        .map_err(|e| Error::Signature(format!("bad .minisig: {e}")))?;
    // allow_legacy = false: only `ED` (prehashed BLAKE2b-512) signatures pass.
    pk.verify(bytes, &sig, false).map_err(|e| match e {
        minisign_verify::Error::UnexpectedKeyId => Error::KeyId {
            expected: key_id_of(key_line).unwrap_or_else(|| "?".into()),
            found: sig_key_id(minisig).unwrap_or_else(|| "?".into()),
        },
        other => Error::Signature(other.to_string()),
    })?;
    parse_manifest(bytes)
}

/// Parse and validate manifest bytes (canonical form + v0 field rules).
/// Does NOT check the signature; use [`verify_manifest`] for anything you trust.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::NotCanonical("not UTF-8".into()))?;
    let value: Value =
        serde_json::from_str(text).map_err(|e| Error::Manifest(format!("not JSON: {e}")))?;
    let canonical = canonical_json(&value)?;
    if canonical != text {
        return Err(Error::NotCanonical(
            "bytes differ from canonical JSON (sorted keys, 2-space indent, trailing newline)"
                .into(),
        ));
    }
    let m: Manifest =
        serde_json::from_value(value).map_err(|e| Error::Manifest(format!("v0 shape: {e}")))?;
    validate(&m)?;
    Ok(m)
}

fn validate(m: &Manifest) -> Result<(), Error> {
    let bad = |msg: String| Err(Error::Manifest(msg));
    if m.schema_version != 0 {
        return bad(format!("schema_version {} (want 0)", m.schema_version));
    }
    if m.sample == Some(true) {
        return bad("sample manifest (placeholder hashes), not a real one".into());
    }
    crate::check_repo(&m.id)?;
    if !is_hex(&m.upstream.commit, 40) {
        return bad(format!(
            "upstream.commit {:?} is not 40 lowercase hex",
            m.upstream.commit
        ));
    }
    if m.files.is_empty() || m.sources.is_empty() {
        return bad("files and sources must be non-empty".into());
    }
    for f in &m.files {
        if !is_hex(&f.sha256, 64) {
            return bad(format!("{}: sha256 is not 64 lowercase hex", f.path));
        }
        crate::check_path(&f.path)?;
    }
    if m.files.windows(2).any(|w| w[0].path >= w[1].path) {
        return bad("files not sorted by path (or duplicated)".into());
    }
    let key = |s: &ManifestSource| (s.priority, s.url.clone().or_else(|| s.magnet.clone()));
    if m.sources.windows(2).any(|w| key(&w[0]) > key(&w[1])) {
        return bad("sources not sorted by priority then url/magnet".into());
    }
    Ok(())
}

pub(crate) fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Canonical JSON as the spec defines it: Python
/// `json.dumps(obj, sort_keys=True, indent=2, ensure_ascii=False) + "\n"`.
/// Keys are sorted here explicitly, so serde_json's `preserve_order` feature
/// (which another crate may switch on) cannot change the result.
pub fn canonical_json(v: &Value) -> Result<String, Error> {
    let mut out = String::new();
    write_value(v, 0, &mut out)?;
    out.push('\n');
    Ok(out)
}

fn write_value(v: &Value, depth: usize, out: &mut String) -> Result<(), Error> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_f64() {
                return Err(Error::NotCanonical(format!(
                    "float {n} (only integers allowed)"
                )));
            }
            out.push_str(&n.to_string());
        }
        Value::String(s) => out.push_str(&json_string(s)),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                indent(depth + 1, out);
                write_value(item, depth + 1, out)?;
            }
            out.push('\n');
            indent(depth, out);
            out.push(']');
        }
        Value::Object(o) => {
            if o.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                indent(depth + 1, out);
                out.push_str(&json_string(k));
                out.push_str(": ");
                write_value(&o[k.as_str()], depth + 1, out)?;
            }
            out.push('\n');
            indent(depth, out);
            out.push('}');
        }
    }
    Ok(())
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// serde_json string escaping matches Python's with `ensure_ascii=False`
/// (short escapes for \b \t \n \f \r, `\u00xx` for other control bytes, raw UTF-8 otherwise).
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Accept either the 2-line `.pub` file or the bare base64 key line.
fn key_line(public_key: &str) -> &str {
    public_key
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .unwrap_or("")
}

/// Key id as minisign prints it: bytes 2..10 of the decoded blob, reversed, hex upper.
fn key_id_from_blob(b64: &str) -> Option<String> {
    let bin = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    if bin.len() < 10 {
        return None;
    }
    Some(
        bin[2..10]
            .iter()
            .rev()
            .map(|b| format!("{b:02X}"))
            .collect(),
    )
}

/// Key id of a public key (`.pub` text or base64 line).
pub fn key_id_of(public_key: &str) -> Option<String> {
    key_id_from_blob(key_line(public_key))
}

/// Key id a `.minisig` claims to be signed with.
pub fn sig_key_id(minisig: &str) -> Option<String> {
    let line = minisig
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))?;
    key_id_from_blob(line)
}
