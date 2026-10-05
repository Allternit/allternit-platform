//! `ao` native-session catalog — Rust port of the gizzi native-session
//! catalog **list half** (P5 binding decision 3, spec
//! `Research/specs/ao-visibility-peers.md`).
//!
//! Port source: `platform/packages/native-sessions/src/{types,harness,
//! catalog,fingerprint}.ts`. Scope: the `NativeSession` schema, the 27-entry
//! harness table, the directory/jsonl catalog walkers, and the sha256
//! fingerprint scheme. **Deferred** (not ported, by binding decision):
//! sqlite/protobuf/cli-export readers (opencode, antigravity, hermes,
//! mastracode, devin, crush, kilo return empty rows), transcript projection,
//! and the gizzi SQLite session DB.
//!
//! Semantics parity notes:
//! - `updated_at` is truncated milliseconds since epoch (JS `Math.trunc(mtimeMs)`).
//! - The final list is sorted by `updated_at` descending (unless a cwd filter
//!   is applied), matching `listNativeSessions`.
//! - Fingerprints are content-free (path/size/mtime/ino + directory children),
//!   matching `fingerprint.ts` — they detect change, they do not hash content.

mod catalog;
mod fingerprint;

pub use catalog::{list_harnesses, list_native_sessions, HarnessListing};

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Transcript reader kind (`types.ts` ReaderKind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderKind {
    Jsonl,
    Sqlite,
    Directory,
    Protobuf,
    #[serde(rename = "cli-export")]
    CliExport,
}

/// One on-disk native CLI session (`types.ts` NativeSession).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSession {
    pub harness: String,
    pub session_id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    pub fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_id: Option<String>,
    /// Text of the last user prompt the session is (or was last) working on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    /// Epoch ms when that prompt arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt_at: Option<u64>,
    pub installed: bool,
    pub reader: ReaderKind,
    pub projectable: bool,
}

impl NativeSession {
    /// Stable join key: `harness+sessionId` (spec binding decision 6).
    pub fn join_key(&self) -> String {
        format!("{}+{}", self.harness, self.session_id)
    }
}

/// Static harness metadata (`harness.ts` HarnessMeta). `default_home` is
/// resolved against the user's home directory.
#[derive(Debug, Clone, Copy)]
pub struct HarnessMeta {
    pub id: &'static str,
    pub label: &'static str,
    pub reader: ReaderKind,
    pub projectable: bool,
    pub resume_hint: &'static str,
    pub env_home: Option<&'static str>,
    pub default_home: fn(&Path) -> PathBuf,
}

/// Verbatim port of the 27-entry table in `harness.ts` (order preserved).
pub const HARNESSES: &[HarnessMeta] = &[
    meta("claude", "Claude Code", ReaderKind::Jsonl, true, "claude --resume <id>", Some("CLAUDE_CONFIG_DIR"), |h| h.join(".claude")),
    meta("gizzi", "Gizzi Code", ReaderKind::Jsonl, true, "gizzi /resume <id>", None, |h| h.join(".gizzi")),
    meta("codex", "Codex", ReaderKind::Jsonl, true, "codex resume <id>", Some("CODEX_HOME"), |h| h.join(".codex")),
    meta("grok", "Grok", ReaderKind::Directory, true, "grok --resume <id>", Some("GROK_HOME"), |h| h.join(".grok")),
    meta("kimi", "Kimi Code", ReaderKind::Directory, true, "kimi --session <id>", Some("KIMI_CODE_HOME"), |h| h.join(".kimi-code")),
    meta("kimi-cli", "Kimi CLI", ReaderKind::Jsonl, true, "kimi", None, |h| h.join(".kimi")),
    meta("qwen", "Qwen Code", ReaderKind::Jsonl, true, "qwen --resume <id>", Some("QWEN_HOME"), |h| h.join(".qwen")),
    meta("opencode", "OpenCode", ReaderKind::Sqlite, true, "opencode --session <id>", Some("OPENCODE_DATA"), |h| h.join(".local").join("share").join("opencode")),
    meta("copilot", "GitHub Copilot CLI", ReaderKind::Jsonl, true, "copilot --resume=<id>", None, |h| h.join(".copilot")),
    meta("pi", "Pi", ReaderKind::Jsonl, true, "pi --session <id>", Some("PI_CODING_AGENT_DIR"), |h| h.join(".pi").join("agent")),
    meta("omp", "Oh My Pi", ReaderKind::Jsonl, true, "omp --resume=<id>", None, |h| h.join(".omp").join("agent")),
    meta("cursor", "Cursor Agent", ReaderKind::Jsonl, true, "cursor-agent --resume <id>", None, |h| h.join(".cursor")),
    meta("antigravity", "Antigravity", ReaderKind::Sqlite, false, "agy --conversation <id>", None, |h| h.join(".gemini").join("antigravity-cli")),
    meta("vibe", "Mistral Vibe", ReaderKind::Directory, true, "vibe", Some("VIBE_HOME"), |h| h.join(".vibe")),
    meta("muse", "Muse Code", ReaderKind::Jsonl, true, "muse", None, |h| h.join(".local").join("share").join("muse")),
    meta("kilo", "Kilo Code", ReaderKind::CliExport, false, "kilo --session <id>", None, |h| h.join(".local").join("share").join("kilo")),
    meta("openhands", "OpenHands", ReaderKind::Directory, true, "openhands", Some("OPENHANDS_CONVERSATIONS_DIR"), |h| h.join(".openhands").join("conversations")),
    meta("hermes", "Hermes Agent", ReaderKind::Sqlite, false, "hermes --resume <id>", Some("HERMES_HOME"), |h| h.join(".hermes")),
    meta("mastracode", "MastraCode", ReaderKind::Sqlite, false, "mastracode --thread <id>", Some("MASTRA_DB_PATH"), |h| h.join(".local").join("share").join("mastra")),
    meta("devin", "Devin CLI", ReaderKind::Sqlite, false, "devin --resume <id>", None, |h| h.join(".devin")),
    meta("gemini", "Gemini CLI", ReaderKind::Jsonl, true, "gemini", None, |h| h.join(".gemini")),
    meta("aider", "Aider", ReaderKind::Jsonl, true, "aider", None, |h| h.join(".aider")),
    meta("cline", "Cline", ReaderKind::Directory, true, "cline (reopen the task in VS Code)", None, |h| h.join("Library").join("Application Support").join("Code").join("User").join("globalStorage").join("saoudrizwan.claude-dev")),
    meta("amp", "Amp", ReaderKind::Directory, true, "amp threads continue <id>", None, |h| h.join(".local").join("share").join("amp")),
    meta("kiro", "Kiro", ReaderKind::Directory, false, "kiro --yolo", None, |h| h.join("Library").join("Application Support").join("Kiro")),
    meta("droid", "Factory Droid", ReaderKind::Jsonl, true, "droid --resume <id>", None, |h| h.join(".factory")),
    meta("crush", "Crush", ReaderKind::Sqlite, false, "crush", None, |h| h.join(".crush")),
];

const fn meta(
    id: &'static str,
    label: &'static str,
    reader: ReaderKind,
    projectable: bool,
    resume_hint: &'static str,
    env_home: Option<&'static str>,
    default_home: fn(&Path) -> PathBuf,
) -> HarnessMeta {
    HarnessMeta {
        id,
        label,
        reader,
        projectable,
        resume_hint,
        env_home,
        default_home,
    }
}

/// Harness table lookup by id (`HARNESS_BY_ID`).
pub fn harness_meta(id: &str) -> Option<&'static HarnessMeta> {
    HARNESSES.iter().find(|h| h.id == id)
}

/// Resolve a harness home directory: `env_home` env override, else the
/// default under `home` (`harness.ts` harnessHome).
pub fn harness_home(id: &str, home: &Path) -> Option<PathBuf> {
    let meta = harness_meta(id)?;
    if let Some(env_name) = meta.env_home {
        if let Ok(value) = std::env::var(env_name) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    Some((meta.default_home)(home))
}

/// Resume hint for a harness id (used by the panel's native rows).
pub fn resume_hint(id: &str) -> Option<&'static str> {
    harness_meta(id).map(|m| m.resume_hint)
}

/// claude-style cwd encoding (`harness.ts` encodeClaudeCwd): every
/// non-alphanumeric character becomes `-`.
pub fn encode_claude_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// grok-style cwd encoding (`harness.ts` encodeGrokCwd`): JS
/// `encodeURIComponent` — unreserved set
/// `A-Za-z0-9 - _ . ! ~ * ' ( )` passes through, everything else is
/// percent-encoded as UTF-8.
pub fn encode_grok_cwd(cwd: &str) -> String {
    let mut out = String::new();
    for &b in cwd.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~'
            | b'*' | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_table_matches_ts_count_and_ids() {
        // The TS table (harness.ts HARNESSES) has exactly 27 entries; keep the
        // port in lockstep.
        assert_eq!(HARNESSES.len(), 27);
        let ids: Vec<&str> = HARNESSES.iter().map(|h| h.id).collect();
        for expected in [
            "claude", "gizzi", "codex", "grok", "kimi", "kimi-cli", "qwen", "opencode",
            "copilot", "pi", "omp", "cursor", "antigravity", "vibe", "muse", "kilo",
            "openhands", "hermes", "mastracode", "devin", "gemini", "aider", "cline",
            "amp", "kiro", "droid", "crush",
        ] {
            assert!(ids.contains(&expected), "missing harness {expected}");
        }
    }

    #[test]
    fn claude_cwd_encoding_matches_ts() {
        assert_eq!(encode_claude_cwd("~/work space"), "-Users-joe-work-space");
        assert_eq!(encode_claude_cwd("/tmp/demo_1"), "-tmp-demo-1");
    }

    #[test]
    fn grok_cwd_encoding_matches_encode_uri_component() {
        assert_eq!(encode_grok_cwd("/tmp/demo dir"), "%2Ftmp%2Fdemo%20dir");
        assert_eq!(encode_grok_cwd("/tmp/demo_1"), "%2Ftmp%2Fdemo_1");
    }
}

/// The current user's home directory (catalog root). `HOME` on unix,
/// `USERPROFILE` on Windows — no new dependency.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}
