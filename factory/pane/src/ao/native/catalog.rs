//! Port of `platform/packages/native-sessions/src/catalog.ts` (list half).
//!
//! One walker per non-sqlite harness, mirroring the TS layout logic exactly
//! (directory shapes, id derivation, updatedAt sources, fingerprint inputs).
//! Sqlite/cli-export harnesses are deferred by binding decision and return no
//! rows (see `mod.rs` module docs).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::fingerprint::{fingerprint_path, fingerprint_paths};
use super::{
    encode_claude_cwd, encode_grok_cwd, harness_home, NativeSession, ReaderKind, HARNESSES,
};

/// Per-harness listing metadata (`catalog.ts` listHarnesses).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessListing {
    pub id: String,
    pub label: String,
    pub reader: ReaderKind,
    pub projectable: bool,
    pub resume_hint: String,
    pub home: String,
    pub present: bool,
}

fn mtime_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|st| st.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Bounded tail budget for transcript scans (last-prompt extraction). The
/// visibility path runs under an ~800ms gateway timeout — seek from the end,
/// never whole-file.
const TAIL_BYTES: u64 = 64 * 1024;

/// Read the last `max_bytes` of a file as (lossy) UTF-8. The first line is
/// dropped when the read started mid-line. `None` on any I/O error.
fn tail_read(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        text = text.split_once('\n').map(|(_, rest)| rest).unwrap_or("").to_string();
    }
    Some(text)
}

/// Last `prompt.accepted` event in a kimi `wire.jsonl` tail → its `time`
/// (epoch ms) and prompt text (`content` text parts joined). `None` when no
/// such event is in the tail.
fn kimi_last_prompt(wire: &Path) -> Option<(u64, String)> {
    let text = tail_read(wire, TAIL_BYTES)?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("prompt.accepted") {
            continue;
        }
        let ts = v.get("time").and_then(|t| t.as_u64())?;
        let msg = content_text(v.get("content"))
            .or_else(|| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))?;
        if msg.trim().is_empty() {
            continue;
        }
        return Some((ts, msg));
    }
    None
}

/// Last claude-style `{"type":"user"}` line in a transcript tail →
/// (timestamp epoch ms, message text). `message.content` may be a string or
/// a block array (handled by `content_text`).
fn claude_like_last_prompt(transcript: &Path) -> Option<(u64, String)> {
    let text = tail_read(transcript, TAIL_BYTES)?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_rfc3339_ms)?;
        let msg = content_text(v.get("message").and_then(|m| m.get("content")))?;
        if msg.trim().is_empty() {
            continue;
        }
        return Some((ts, msg));
    }
    None
}

/// Last codex `payload.type == "user_message"` line in a rollout tail →
/// (timestamp epoch ms, message text).
fn codex_last_prompt(transcript: &Path) -> Option<(u64, String)> {
    let text = tail_read(transcript, TAIL_BYTES)?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.pointer("/payload/type").and_then(|t| t.as_str()) != Some("user_message") {
            continue;
        }
        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_rfc3339_ms)?;
        let msg = v
            .pointer("/payload/message")
            .and_then(|m| m.as_str())?
            .to_string();
        if msg.trim().is_empty() {
            continue;
        }
        return Some((ts, msg));
    }
    None
}

/// Normalize a timestamp value to epoch ms: numbers ≥1e12 are already ms,
/// ≥1e9 are epoch seconds; strings try RFC 3339 then a plain number.
/// Anything else → `None`.
fn epoch_ms_value(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f >= 1e12 {
                Some(f as u64)
            } else if f >= 1e9 {
                Some((f * 1000.0) as u64)
            } else {
                None
            }
        }
        serde_json::Value::String(s) => {
            let t = s.trim();
            parse_rfc3339_ms(t).or_else(|| {
                t.parse::<f64>().ok().and_then(|f| {
                    if f >= 1e12 {
                        Some(f as u64)
                    } else if f >= 1e9 {
                        Some((f * 1000.0) as u64)
                    } else {
                        None
                    }
                })
            })
        }
        _ => None,
    }
}

/// Timestamp from the first present candidate key, top-level or nested under
/// `message`.
fn record_timestamp(v: &serde_json::Value) -> Option<u64> {
    for key in ["timestamp", "time", "ts", "created_at", "createdAt"] {
        if let Some(ms) = v.get(key).and_then(epoch_ms_value) {
            return Some(ms);
        }
        if let Some(ms) = v.pointer("/message/timestamp").and_then(epoch_ms_value) {
            return Some(ms);
        }
    }
    None
}

/// Text from a gemini-style `parts` array (`[{"text": "…"}, …]`).
fn parts_text(v: Option<&serde_json::Value>) -> Option<String> {
    let parts = v?.as_array()?;
    let mut out = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Best-effort (text, timestamp) from one transcript record that marks a
/// user/human turn. Accepts the shapes the uncertain harnesses are known or
/// believed to write: top-level `role:"user"`, claude-style `type:"user"`,
/// OpenHands `source:"user"`, codex-style `payload.type:"user_message"`, and
/// nested `message.role:"user"`. Text comes from `content` (string or block
/// array via `content_text`), gemini-style `parts`, a string `message`, or a
/// string `text`.
fn generic_user_payload(v: &serde_json::Value) -> Option<(Option<u64>, String)> {
    let is_user = v.get("role").and_then(|r| r.as_str()) == Some("user")
        || v.get("type").and_then(|t| t.as_str()) == Some("user")
        || v.get("type").and_then(|t| t.as_str()) == Some("user_message")
        || v.get("source").and_then(|s| s.as_str()) == Some("user")
        || v.pointer("/message/role").and_then(|r| r.as_str()) == Some("user")
        || v.pointer("/payload/type").and_then(|t| t.as_str()) == Some("user_message");
    if !is_user {
        return None;
    }
    let msg = content_text(v.get("content"))
        .or_else(|| content_text(v.get("message").and_then(|m| m.get("content"))))
        .or_else(|| parts_text(v.get("parts")))
        .or_else(|| parts_text(v.get("message").and_then(|m| m.get("parts"))))
        .or_else(|| v.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()))
        .or_else(|| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
        .or_else(|| {
            v.pointer("/payload/message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })?;
    if msg.trim().is_empty() {
        return None;
    }
    Some((record_timestamp(v), msg))
}

/// Generic bounded tail-scan for harnesses whose record shape varies by
/// version (copilot, pi/omp, muse, vibe, gemini, droid, kimi-cli). Handles
/// both jsonl (one record per line) and single-line JSON arrays. Timestamp
/// may be `None` when the harness does not persist one.
fn generic_last_user_prompt(transcript: &Path) -> Option<(Option<u64>, String)> {
    let text = tail_read(transcript, TAIL_BYTES)?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        // Single-line JSON array (gemini/droid chat exports).
        if let Some(items) = v.as_array() {
            for item in items.iter().rev() {
                if let Some(found) = generic_user_payload(item) {
                    return Some(found);
                }
            }
            continue;
        }
        if let Some(found) = generic_user_payload(&v) {
            return Some(found);
        }
    }
    None
}

/// Grok `updates.jsonl` tails carry `session/update` records; the user's
/// prompt streams in as `user_message_chunk` updates. The last contiguous
/// run of such chunks (they may split one prompt across lines) is joined;
/// the timestamp (epoch **seconds**) of the run's first chunk is when the
/// prompt started arriving.
fn grok_last_prompt(updates: &Path) -> Option<(u64, String)> {
    let text = tail_read(updates, TAIL_BYTES)?;
    let mut run: Vec<(u64, String)> = Vec::new();
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            if run.is_empty() { continue } else { break };
        };
        let is_user_chunk = v
            .pointer("/params/update/sessionUpdate")
            .and_then(|s| s.as_str())
            == Some("user_message_chunk");
        if is_user_chunk {
            let (Some(ts), Some(chunk)) = (
                v.get("timestamp").and_then(|t| t.as_u64()),
                v.pointer("/params/update/content/text").and_then(|t| t.as_str()),
            ) else {
                break;
            };
            run.push((ts, chunk.to_string()));
        } else if !run.is_empty() {
            // Reached the end of the trailing user-chunk run (agent chunks
            // and other session updates stream after it).
            break;
        }
    }
    if run.is_empty() {
        return None;
    }
    let ts = run.last()?.0;
    let msg: String = run.iter().rev().map(|(_, c)| c.as_str()).collect();
    if msg.trim().is_empty() {
        return None;
    }
    Some((ts * 1000, msg))
}

/// OpenHands conversations persist one JSON event per file under
/// `events/`; a user turn is an event with `source:"user"` and a `message`.
/// Scan the newest event files (bounded) until one carries a user message.
fn openhands_last_prompt(dir: &Path) -> Option<(Option<u64>, String)> {
    let events = dir.join("events");
    let mut files = list_files(&events, |n| n.ends_with(".json"));
    files.sort();
    for file in files.iter().rev().take(8) {
        let text = tail_read(file, TAIL_BYTES)?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
            continue;
        };
        if let Some(found) = generic_user_payload(&v) {
            return Some(found);
        }
    }
    None
}

fn list_files(dir: &Path, predicate: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| predicate(&n.to_string_lossy()))
                .unwrap_or(false)
        })
        .collect()
}

fn read_dir_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect()
}

/// Port of `walkDirs`: depth-limited directory walk, skipping dot-directories.
fn walk_dirs(dir: &Path, max_depth: i32) -> Vec<PathBuf> {
    if max_depth < 0 || !dir.exists() {
        return Vec::new();
    }
    let mut out = vec![dir.to_path_buf()];
    for child in read_dir_dirs(dir) {
        if child
            .file_name()
            .map(|n| n.to_string_lossy().starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }
        out.extend(walk_dirs(&child, max_depth - 1));
    }
    out
}

fn basename(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn session(partial: NativeSession) -> NativeSession {
    NativeSession {
        installed: true,
        ..partial
    }
}

/// claude/gizzi/qwen layout (`catalog.ts` listClaudeLike):
/// `<root>/<subdir>/<project-dir>[/chats]/*.jsonl`.
fn list_claude_like(harness: &str, root: &Path, subdir: &str, reader: ReaderKind, nested_chats: bool) -> Vec<NativeSession> {
    let projects = root.join(subdir);
    if !projects.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for project in read_dir_dirs(&projects) {
        let project_name = basename(&project);
        let dir = if nested_chats {
            project.join("chats")
        } else {
            project.clone()
        };
        if !dir.exists() {
            continue;
        }
        for file in list_files(&dir, |n| n.ends_with(".jsonl") && !n.contains(".runtime.")) {
            let id = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let (last_prompt_at, last_prompt) = claude_like_last_prompt(&file)
                .map(|(ts, text)| (Some(ts), Some(text)))
                .unwrap_or((None, None));
            out.push(session(NativeSession {
                harness: harness.to_string(),
                session_id: id.clone(),
                path: file.to_string_lossy().to_string(),
                cwd: (!project_name.starts_with('-')).then_some(project_name.clone()),
                title: None,
                updated_at: mtime_ms(&file),
                created_at: None,
                fingerprint: fingerprint_path(&file),
                last_event_id: Some(id),
                last_prompt,
                last_prompt_at,
                installed: false,
                reader,
                projectable: true,
            }));
        }
    }
    out
}

/// codex layout (`catalog.ts` listCodex): rollout files under
/// `sessions/` and `archived_sessions/`, depth 4.
fn list_codex(root: &Path) -> Vec<NativeSession> {
    let rollout_id = |name: &str| -> Option<String> {
        // /^rollout-.*-([0-9a-fA-F-]{36})\.jsonl(?:\.zst)?$/ — the greedy
        // `.*-` backtracks so the capture is always the LAST 36 characters
        // of the stem (uuid dashes included).
        let stem = name.strip_suffix(".zst").unwrap_or(name);
        let stem = stem.strip_suffix(".jsonl")?;
        if stem.len() < "rollout-".len() + 36 {
            return None;
        }
        let (head, tail) = stem.split_at(stem.len() - 36);
        if !head.starts_with("rollout-") {
            return None;
        }
        if tail.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            Some(tail.to_string())
        } else {
            None
        }
    };
    let mut out = Vec::new();
    for base in ["sessions", "archived_sessions"] {
        let base_dir = root.join(base);
        if !base_dir.exists() {
            continue;
        }
        for dir in walk_dirs(&base_dir, 4) {
            for file in list_files(&dir, |n| rollout_id(n).is_some()) {
                let Some(id) = rollout_id(&basename(&file)) else { continue };
                let (last_prompt_at, last_prompt) = codex_last_prompt(&file)
                    .map(|(ts, text)| (Some(ts), Some(text)))
                    .unwrap_or((None, None));
                out.push(session(NativeSession {
                    harness: "codex".to_string(),
                    session_id: id.clone(),
                    path: file.to_string_lossy().to_string(),
                    cwd: None,
                    title: (base == "archived_sessions").then(|| "(archived)".to_string()),
                    updated_at: mtime_ms(&file),
                    created_at: None,
                    fingerprint: fingerprint_path(&file),
                    last_event_id: if base == "sessions" { Some(id) } else { None },
                    last_prompt,
                    last_prompt_at,
                    installed: false,
                    reader: ReaderKind::Jsonl,
                    projectable: true,
                }));
            }
        }
    }
    out
}

/// grok layout (`catalog.ts` listGrok):
/// `sessions/<cwdEnc>/<session>/updates.jsonl` + optional `summary.json`.
fn list_grok(root: &Path) -> Vec<NativeSession> {
    let sessions = root.join("sessions");
    if !sessions.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for cwd_dir in read_dir_dirs(&sessions) {
        for dir in read_dir_dirs(&cwd_dir) {
            let updates = dir.join("updates.jsonl");
            if !updates.exists() {
                continue;
            }
            let summary = dir.join("summary.json");
            let mut title = None;
            let mut cwd = None;
            if summary.exists() {
                if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(
                    &fs::read(&summary).unwrap_or_default(),
                ) {
                    cwd = parsed
                        .get("info")
                        .and_then(|i| i.get("cwd"))
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());
                    title = parsed
                        .get("session_summary")
                        .and_then(|s| s.as_str())
                        .map(|s| s.chars().take(120).collect());
                }
            }
            let id = basename(&dir);
            let cwd = cwd.or_else(|| {
                percent_decode(&basename(&cwd_dir))
            });
            let (last_prompt_at, last_prompt) = grok_last_prompt(&updates)
                .map(|(ts, text)| (Some(ts), Some(text)))
                .unwrap_or((None, None));
            let fp_targets: Vec<&Path> = [&summary, &updates]
                .iter()
                .filter(|p| p.exists())
                .map(|p| p.as_path())
                .collect();
            out.push(session(NativeSession {
                harness: "grok".to_string(),
                session_id: id.clone(),
                path: dir.to_string_lossy().to_string(),
                cwd,
                title,
                updated_at: mtime_ms(&updates),
                created_at: None,
                fingerprint: fingerprint_paths(&fp_targets),
                last_event_id: Some(id),
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Directory,
                projectable: true,
            }));
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Kimi Code layout (`catalog.ts` listKimiCode):
/// `sessions/wd_*/session_*/` with `state.json` + `agents/main/wire.jsonl`.
fn list_kimi_code(root: &Path) -> Vec<NativeSession> {
    let sessions = root.join("sessions");
    if !sessions.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for wd in read_dir_dirs(&sessions) {
        if !basename(&wd).starts_with("wd_") {
            continue;
        }
        for dir in read_dir_dirs(&wd) {
            if !basename(&dir).starts_with("session_") {
                continue;
            }
            let state_path = dir.join("state.json");
            let wire = dir.join("agents").join("main").join("wire.jsonl");
            let mut cwd = None;
            let mut updated_at = mtime_ms(&dir);
            let mut state_updated_at = None;
            let mut last_prompt = None;
            let mut id = basename(&dir).trim_start_matches("session_").to_string();
            if state_path.exists() {
                if let Ok(state) = serde_json::from_slice::<serde_json::Value>(
                    &fs::read(&state_path).unwrap_or_default(),
                ) {
                    cwd = state
                        .get("cwd")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());
                    if let Some(state_id) = state.get("id").and_then(|v| v.as_str()) {
                        id = state_id
                            .strip_prefix("session_")
                            .unwrap_or(state_id)
                            .to_string();
                    }
                    if let Some(ms) = state.get("updatedAt").and_then(|v| v.as_u64()) {
                        updated_at = ms;
                        state_updated_at = Some(ms);
                    }
                    last_prompt = state
                        .get("lastPrompt")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            // The wire tail is authoritative for arrival time; state.json
            // `lastPrompt` wins for text (it is the prompt the session
            // actually accepted), with the wire event text as fallback.
            let (last_prompt_at, last_prompt) = if wire.exists() {
                match kimi_last_prompt(&wire) {
                    Some((ts, wire_text)) => (
                        Some(ts),
                        last_prompt.or_else(|| {
                            let t = wire_text;
                            if t.trim().is_empty() { None } else { Some(t) }
                        }),
                    ),
                    None => (state_updated_at, last_prompt),
                }
            } else {
                (state_updated_at, last_prompt)
            };
            let fp_targets: Vec<&Path> = [&state_path, &wire]
                .iter()
                .filter(|p| p.exists())
                .map(|p| p.as_path())
                .collect();
            out.push(session(NativeSession {
                harness: "kimi".to_string(),
                session_id: id.clone(),
                path: dir.to_string_lossy().to_string(),
                cwd,
                title: None,
                updated_at,
                created_at: None,
                fingerprint: fingerprint_paths(&fp_targets),
                last_event_id: Some(id),
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Directory,
                projectable: true,
            }));
        }
    }
    out
}

/// Kimi CLI layout (`catalog.ts` listKimiCli):
/// `sessions/<hash>/<session>/context.jsonl`.
fn list_kimi_cli(root: &Path) -> Vec<NativeSession> {
    let sessions = root.join("sessions");
    if !sessions.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for hash_dir in read_dir_dirs(&sessions) {
        for dir in read_dir_dirs(&hash_dir) {
            let ctx = dir.join("context.jsonl");
            if !ctx.exists() {
                continue;
            }
            let id = basename(&dir);
            let (last_prompt_at, last_prompt) = generic_last_user_prompt(&ctx)
                .map(|(ts, text)| (ts, Some(text)))
                .unwrap_or((None, None));
            out.push(session(NativeSession {
                harness: "kimi-cli".to_string(),
                session_id: id.clone(),
                path: ctx.to_string_lossy().to_string(),
                cwd: None,
                title: None,
                updated_at: mtime_ms(&ctx),
                created_at: None,
                fingerprint: fingerprint_path(&ctx),
                last_event_id: None,
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Jsonl,
                projectable: true,
            }));
        }
    }
    out
}

/// copilot layout (`catalog.ts` listCopilot):
/// `session-state/<id>[/events.jsonl]` or flat `*.jsonl`.
fn list_copilot(root: &Path) -> Vec<NativeSession> {
    let dir = root.join("session-state");
    if !dir.exists() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let (events, id) = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let events = entry.path().join("events.jsonl");
            (events, entry.file_name().to_string_lossy().to_string())
        } else {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".jsonl") {
                continue;
            }
            (
                entry.path(),
                name.strip_suffix(".jsonl").unwrap_or(&name).to_string(),
            )
        };
        if !events.exists() {
            continue;
        }
        let (last_prompt_at, last_prompt) = generic_last_user_prompt(&events)
            .map(|(ts, text)| (ts, Some(text)))
            .unwrap_or((None, None));
        out.push(session(NativeSession {
            harness: "copilot".to_string(),
            session_id: id.clone(),
            path: events.to_string_lossy().to_string(),
            cwd: None,
            title: None,
            updated_at: mtime_ms(&events),
            created_at: None,
            fingerprint: fingerprint_path(&events),
            last_event_id: None,
            last_prompt,
            last_prompt_at,
            installed: false,
            reader: ReaderKind::Jsonl,
            projectable: true,
        }));
    }
    out
}

/// pi/omp layout (`catalog.ts` listPiLike): `sessions/**.jsonl`, depth 5,
/// id = final `_`-separated segment.
fn list_pi_like(harness: &str, root: &Path) -> Vec<NativeSession> {
    let sessions = root.join("sessions");
    if !sessions.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for dir in walk_dirs(&sessions, 5) {
        for file in list_files(&dir, |n| n.ends_with(".jsonl")) {
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let id = stem.rsplit('_').next().unwrap_or(&stem).to_string();
            let (last_prompt_at, last_prompt) = generic_last_user_prompt(&file)
                .map(|(ts, text)| (ts, Some(text)))
                .unwrap_or((None, None));
            out.push(session(NativeSession {
                harness: harness.to_string(),
                session_id: id.clone(),
                path: file.to_string_lossy().to_string(),
                cwd: None,
                title: None,
                updated_at: mtime_ms(&file),
                created_at: None,
                fingerprint: fingerprint_path(&file),
                last_event_id: None,
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Jsonl,
                projectable: true,
            }));
        }
    }
    out
}

/// cursor layout (`catalog.ts` listCursor):
/// `projects/<proj>/agent-transcripts/<id>[.jsonl]`.
fn list_cursor(root: &Path) -> Vec<NativeSession> {
    let projects = root.join("projects");
    if !projects.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for project in read_dir_dirs(&projects) {
        let transcripts = project.join("agent-transcripts");
        if !transcripts.exists() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&transcripts) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let (file, id) = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().to_string();
                (entry.path().join(format!("{}.jsonl", name)), name)
            } else {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".jsonl") {
                    continue;
                }
                (entry.path(), name.strip_suffix(".jsonl").unwrap_or(&name).to_string())
            };
            // `Path::ends_with` matches whole components, so a plain
            // `file.ends_with(".jsonl")` would always be false here.
            if !file.to_string_lossy().ends_with(".jsonl") || !file.exists() {
                continue;
            }
            // cursor-agent is a claude-code fork: same `type:"user"` transcript
            // records (`message.content`, ISO `timestamp`).
            let (last_prompt_at, last_prompt) = claude_like_last_prompt(&file)
                .map(|(ts, text)| (Some(ts), Some(text)))
                .unwrap_or((None, None));
            out.push(session(NativeSession {
                harness: "cursor".to_string(),
                session_id: id.clone(),
                path: file.to_string_lossy().to_string(),
                cwd: None,
                title: None,
                updated_at: mtime_ms(&file),
                created_at: None,
                fingerprint: fingerprint_path(&file),
                last_event_id: None,
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Jsonl,
                projectable: true,
            }));
        }
    }
    out
}

/// openhands layout (`catalog.ts` listOpenHands): one directory per session.
fn list_openhands(root: &Path) -> Vec<NativeSession> {
    if !root.exists() {
        return Vec::new();
    }
    read_dir_dirs(root)
        .into_iter()
        .map(|dir| {
            let id = basename(&dir);
            let (last_prompt_at, last_prompt) = openhands_last_prompt(&dir)
                .map(|(ts, text)| (ts, Some(text)))
                .unwrap_or((None, None));
            session(NativeSession {
                harness: "openhands".to_string(),
                session_id: id.clone(),
                path: dir.to_string_lossy().to_string(),
                cwd: None,
                title: None,
                updated_at: mtime_ms(&dir),
                created_at: None,
                fingerprint: fingerprint_path(&dir),
                last_event_id: None,
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Directory,
                projectable: true,
            })
        })
        .collect()
}

/// muse layout (`catalog.ts` listMuse): `sessions/**/session.jsonl`, depth 4,
/// id = directory name.
fn list_muse(root: &Path) -> Vec<NativeSession> {
    let sessions = root.join("sessions");
    if !sessions.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for dir in walk_dirs(&sessions, 4) {
        let file = dir.join("session.jsonl");
        if !file.exists() {
            continue;
        }
        let id = basename(&dir);
        let (last_prompt_at, last_prompt) = generic_last_user_prompt(&file)
            .map(|(ts, text)| (ts, Some(text)))
            .unwrap_or((None, None));
        out.push(session(NativeSession {
            harness: "muse".to_string(),
            session_id: id.clone(),
            path: file.to_string_lossy().to_string(),
            cwd: None,
            title: None,
            updated_at: mtime_ms(&file),
            created_at: None,
            fingerprint: fingerprint_path(&file),
            last_event_id: None,
            last_prompt,
            last_prompt_at,
            installed: false,
            reader: ReaderKind::Jsonl,
            projectable: true,
        }));
    }
    out
}

/// vibe layout (`catalog.ts` listVibe): `logs/session/<dir>/messages.jsonl`.
fn list_vibe(root: &Path) -> Vec<NativeSession> {
    let dir = root.join("logs").join("session");
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for ses in read_dir_dirs(&dir) {
        let messages = ses.join("messages.jsonl");
        if !messages.exists() {
            continue;
        }
        let name = basename(&ses);
        let id = name
            .trim_start_matches("session_")
            .rsplit('_')
            .next()
            .unwrap_or(&name)
            .to_string();
        let (last_prompt_at, last_prompt) = generic_last_user_prompt(&messages)
            .map(|(ts, text)| (ts, Some(text)))
            .unwrap_or((None, None));
        out.push(session(NativeSession {
            harness: "vibe".to_string(),
            session_id: id.clone(),
            path: ses.to_string_lossy().to_string(),
            cwd: None,
            title: None,
            updated_at: mtime_ms(&messages),
            created_at: None,
            fingerprint: fingerprint_path(&ses),
            last_event_id: None,
            last_prompt,
            last_prompt_at,
            installed: false,
            reader: ReaderKind::Directory,
            projectable: true,
        }));
    }
    out
}

/// gemini layout (`catalog.ts` listGemini): `tmp/**/chats/*.{json,jsonl}`, depth 3.
fn list_gemini(root: &Path) -> Vec<NativeSession> {
    let tmp = root.join("tmp");
    if !tmp.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for dir in walk_dirs(&tmp, 3) {
        let chats = dir.join("chats");
        if !chats.exists() {
            continue;
        }
        for file in list_files(&chats, |n| n.ends_with(".json") || n.ends_with(".jsonl")) {
            let name = basename(&file);
            let id = name
                .strip_suffix(".jsonl")
                .or_else(|| name.strip_suffix(".json"))
                .unwrap_or(&name)
                .to_string();
            let (last_prompt_at, last_prompt) = generic_last_user_prompt(&file)
                .map(|(ts, text)| (ts, Some(text)))
                .unwrap_or((None, None));
            out.push(session(NativeSession {
                harness: "gemini".to_string(),
                session_id: id.clone(),
                path: file.to_string_lossy().to_string(),
                cwd: None,
                title: None,
                updated_at: mtime_ms(&file),
                created_at: None,
                fingerprint: fingerprint_path(&file),
                last_event_id: None,
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Jsonl,
                projectable: true,
            }));
        }
    }
    out
}

/// factory droid layout (`catalog.ts` listFactory): flat `sessions/*.{jsonl,json}`.
fn list_factory(root: &Path) -> Vec<NativeSession> {
    let sessions = root.join("sessions");
    if !sessions.exists() {
        return Vec::new();
    }
    list_files(&sessions, |n| n.ends_with(".jsonl") || n.ends_with(".json"))
        .into_iter()
        .map(|file| {
            let name = basename(&file);
            let id = name
                .strip_suffix(".jsonl")
                .or_else(|| name.strip_suffix(".json"))
                .unwrap_or(&name)
                .to_string();
            let (last_prompt_at, last_prompt) = generic_last_user_prompt(&file)
                .map(|(ts, text)| (ts, Some(text)))
                .unwrap_or((None, None));
            session(NativeSession {
                harness: "droid".to_string(),
                session_id: id.clone(),
                path: file.to_string_lossy().to_string(),
                cwd: None,
                title: None,
                updated_at: mtime_ms(&file),
                created_at: None,
                fingerprint: fingerprint_path(&file),
                last_event_id: None,
                last_prompt,
                last_prompt_at,
                installed: false,
                reader: ReaderKind::Jsonl,
                projectable: true,
            })
        })
        .collect()
}

/// cline layout (`catalog.ts` listCline):
/// `tasks/<taskId>/api_conversation_history.json` + optional `ui_messages.json`.
fn list_cline(root: &Path) -> Vec<NativeSession> {
    let tasks = root.join("tasks");
    if !tasks.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for task in read_dir_dirs(&tasks) {
        let history = task.join("api_conversation_history.json");
        if !history.exists() {
            continue;
        }
        let ui = task.join("ui_messages.json");
        let mut title = None;
        let mut last_prompt = None;
        let mut last_prompt_at = None;
        if let Ok(parsed) =
            serde_json::from_slice::<serde_json::Value>(&fs::read(&history).unwrap_or_default())
        {
            if let Some(first) = parsed.as_array().and_then(|msgs| {
                msgs.iter().find(|m| {
                    m.get("role").and_then(|r| r.as_str()) == Some("user")
                        && m.get("content").and_then(|c| c.as_str()).is_some()
                })
            }) {
                title = first
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|s| s.chars().take(120).collect());
            }
            // Reuse the same parse for the last prompt (the walker already
            // reads this document for the title — no extra I/O).
            if let Some(last) = parsed.as_array().and_then(|msgs| {
                msgs.iter().rev().find(|m| {
                    m.get("role").and_then(|r| r.as_str()) == Some("user")
                        && m.get("content").and_then(|c| c.as_str()).is_some()
                })
            }) {
                last_prompt = last
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string());
                last_prompt_at = last.get("ts").and_then(epoch_ms_value);
            }
        }
        let id = basename(&task);
        let fp_targets: Vec<&Path> = [&history, &ui]
            .iter()
            .filter(|p| p.exists())
            .map(|p| p.as_path())
            .collect();
        out.push(session(NativeSession {
            harness: "cline".to_string(),
            session_id: id.clone(),
            path: task.to_string_lossy().to_string(),
            cwd: None,
            title,
            updated_at: mtime_ms(&history),
            created_at: None,
            fingerprint: fingerprint_paths(&fp_targets),
            last_event_id: Some(id),
            last_prompt,
            last_prompt_at,
            installed: false,
            reader: ReaderKind::Directory,
            projectable: true,
        }));
    }
    out
}

/// amp layout (`catalog.ts` listAmp): `threads/T-*.json` with title + createdAt
/// from the JSON document.
fn list_amp(root: &Path) -> Vec<NativeSession> {
    let threads = root.join("threads");
    if !threads.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for file in list_files(&threads, |n| n.starts_with("T-") && n.ends_with(".json")) {
        let id = basename(&file).strip_suffix(".json").unwrap_or(&basename(&file)).to_string();
        let mut title = None;
        let mut created_at = None;
        let mut last_prompt = None;
        let mut last_prompt_at = None;
        if let Ok(parsed) =
            serde_json::from_slice::<serde_json::Value>(&fs::read(&file).unwrap_or_default())
        {
            if let Some(created) = parsed.get("created_at").and_then(|c| c.as_str()) {
                if let Some(t) = parse_rfc3339_ms(created) {
                    created_at = Some(t);
                }
            }
            if let Some(first) = parsed.get("messages").and_then(|m| m.as_array()).and_then(
                |msgs| {
                    msgs.iter()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                },
            ) {
                title = content_text(first.get("content")).map(|s| s.chars().take(120).collect());
            }
            // Reuse the same parse for the last prompt (the walker already
            // reads this document for the title/createdAt — no extra I/O).
            // amp messages carry no per-message timestamp.
            if let Some(last) = parsed.get("messages").and_then(|m| m.as_array()).and_then(
                |msgs| {
                    msgs.iter()
                        .rev()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                },
            ) {
                last_prompt = content_text(last.get("content"));
            }
        }
        out.push(session(NativeSession {
            harness: "amp".to_string(),
            session_id: id.clone(),
            path: file.to_string_lossy().to_string(),
            cwd: None,
            title,
            updated_at: mtime_ms(&file),
            created_at,
            fingerprint: fingerprint_path(&file),
            last_event_id: Some(id),
            last_prompt,
            last_prompt_at,
            installed: false,
            reader: ReaderKind::Directory,
            projectable: true,
        }));
    }
    out
}

/// Extract text from an Anthropic-style `content` field (string or block array)
/// — the subset of `jsonl.ts` contentText the amp walker needs.
fn content_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            let mut out = String::new();
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        out.push_str(text);
                    }
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Parse an RFC 3339 timestamp into truncated milliseconds, mirroring
/// `Date.parse` (which returns NaN → `undefined` for garbage input).
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|d| {
            let millis = (d.unix_timestamp_nanos() / 1_000_000).max(0);
            millis as u64
        })
}

/// `catalog.ts` listHarnesses.
pub fn list_harnesses(home: &Path) -> Vec<HarnessListing> {
    HARNESSES
        .iter()
        .map(|h| {
            let home_path = harness_home(h.id, home).unwrap_or_else(|| PathBuf::from(""));
            HarnessListing {
                id: h.id.to_string(),
                label: h.label.to_string(),
                reader: h.reader,
                projectable: h.projectable,
                resume_hint: h.resume_hint.to_string(),
                home: home_path.to_string_lossy().to_string(),
                present: home_path.exists(),
            }
        })
        .collect()
}

/// `catalog.ts` listNativeSessions. `home` defaults are left to the caller
/// (the panel passes the real home dir; tests pass a fixture home).
///
/// Deferred readers (sqlite/cli-export: opencode, antigravity, hermes,
/// mastracode, devin, crush, kilo) return no rows by binding decision.
pub fn list_native_sessions(home: &Path, harnesses: Option<&[&str]>, cwd: Option<&str>) -> Vec<NativeSession> {
    let wanted: Vec<&str> = match harnesses {
        Some(ids) => ids.to_vec(),
        None => HARNESSES.iter().map(|h| h.id).collect(),
    };
    let mut out: Vec<NativeSession> = Vec::new();
    for id in wanted {
        let Some(root) = harness_home(id, home) else { continue };
        if !root.exists() {
            continue;
        }
        match id {
            "claude" => out.extend(list_claude_like("claude", &root, "projects", ReaderKind::Jsonl, false)),
            "gizzi" => out.extend(list_claude_like("gizzi", &root, "projects", ReaderKind::Jsonl, false)),
            "qwen" => out.extend(list_claude_like("qwen", &root, "projects", ReaderKind::Jsonl, true)),
            "codex" => out.extend(list_codex(&root)),
            "grok" => out.extend(list_grok(&root)),
            "kimi" => out.extend(list_kimi_code(&root)),
            "kimi-cli" => out.extend(list_kimi_cli(&root)),
            "copilot" => out.extend(list_copilot(&root)),
            "pi" => out.extend(list_pi_like("pi", &root)),
            "omp" => out.extend(list_pi_like("omp", &root)),
            "cursor" => out.extend(list_cursor(&root)),
            "openhands" => out.extend(list_openhands(&root)),
            "muse" => out.extend(list_muse(&root)),
            "vibe" => out.extend(list_vibe(&root)),
            "gemini" => out.extend(list_gemini(&root)),
            "droid" => out.extend(list_factory(&root)),
            "cline" => out.extend(list_cline(&root)),
            "amp" => out.extend(list_amp(&root)),
            // sqlite/cli-export readers deferred (binding decision 3).
            _ => {}
        }
    }
    if let Some(cwd) = cwd {
        let encoded = encode_claude_cwd(cwd);
        let grok_enc = encode_grok_cwd(cwd);
        return out
            .into_iter()
            .filter(|s| {
                if let Some(scwd) = &s.cwd {
                    if scwd == cwd || scwd.contains(cwd) {
                        return true;
                    }
                }
                s.path.contains(&encoded) || s.path.contains(&grok_enc)
            })
            .collect();
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn kimi_walker_reads_state_json() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi-code")
            .join("sessions")
            .join("wd_abc123")
            .join("session_def456");
        std::fs::create_dir_all(dir.join("agents").join("main")).unwrap();
        std::fs::write(
            dir.join("state.json"),
            r#"{"id":"session_aaa-bbb-ccc","cwd":"/tmp/demo","updatedAt":1725974400000}"#,
        )
        .unwrap();
        std::fs::write(dir.join("agents").join("main").join("wire.jsonl"), "{}\n").unwrap();

        let rows = list_native_sessions(
            home.path(),
            Some(&["kimi"]),
            None,
        );
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.harness, "kimi");
        // state.json id wins, "session_" prefix stripped (catalog.ts:184).
        assert_eq!(row.session_id, "aaa-bbb-ccc");
        assert_eq!(row.cwd.as_deref(), Some("/tmp/demo"));
        assert_eq!(row.updated_at, 1725974400000);
        assert_eq!(row.reader, ReaderKind::Directory);
        assert!(row.projectable);
        assert!(row.path.ends_with("session_def456"));
        assert_eq!(row.fingerprint.len(), 64);
    }

    #[test]
    fn kimi_walker_falls_back_to_dir_name() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi-code")
            .join("sessions")
            .join("wd_x")
            .join("session_fallback-id");
        std::fs::create_dir_all(&dir).unwrap();

        let rows = list_native_sessions(home.path(), Some(&["kimi"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "fallback-id");
        assert!(rows[0].cwd.is_none());
    }

    #[test]
    fn claude_walker_skips_runtime_files_and_dash_projects() {
        let home = fixture_home();
        let proj = home.path().join(".claude").join("projects").join("-Users-joe-demo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("11111111-2222-3333-4444-555555555555.jsonl"), "{}\n").unwrap();
        std::fs::write(proj.join("66666666-2222-3333-4444-555555555555.runtime.jsonl"), "{}\n").unwrap();

        let rows = list_native_sessions(home.path(), Some(&["claude"]), None);
        assert_eq!(rows.len(), 1, ".runtime. files must be excluded");
        assert_eq!(rows[0].session_id, "11111111-2222-3333-4444-555555555555");
        // Project dirs are dash-encoded cwd paths; the TS catalog leaves
        // cwd undefined for them (catalog.ts:62) — the cwd filter matches
        // via the encoded path component instead.
        assert!(rows[0].cwd.is_none());

        // Project dirs whose name starts with "-" have no cwd (catalog.ts:62).
        let dash = home.path().join(".claude").join("projects").join("-tmp-dash");
        std::fs::create_dir_all(&dash).unwrap();
        std::fs::write(dash.join("abc.jsonl"), "{}\n").unwrap();
        let rows = list_native_sessions(home.path(), Some(&["claude"]), None);
        assert_eq!(rows.len(), 2);
        let dash_row = rows.iter().find(|r| r.session_id == "abc").unwrap();
        assert!(dash_row.cwd.is_none());
    }

    #[test]
    fn codex_walker_extracts_uuid_and_marks_archived() {
        let home = fixture_home();
        let codex = home.path().join(".codex");
        let live = codex.join("sessions").join("2026").join("09").join("10");
        std::fs::create_dir_all(&live).unwrap();
        let uuid = "123e4567-e89b-42d3-a456-426614174000";
        std::fs::write(live.join(format!("rollout-2026-09-10T00-00-00-{}.jsonl", uuid)), "{}\n").unwrap();
        let archived = codex.join("archived_sessions");
        std::fs::create_dir_all(&archived).unwrap();
        std::fs::write(archived.join(format!("rollout-old-{}.jsonl.zst", uuid)), "zst").unwrap();

        let rows = list_native_sessions(home.path(), Some(&["codex"]), None);
        assert_eq!(rows.len(), 2);
        let live_row = rows.iter().find(|r| r.title.is_none()).unwrap();
        assert_eq!(live_row.session_id, uuid);
        assert_eq!(live_row.last_event_id.as_deref(), Some(uuid));
        let archived_row = rows.iter().find(|r| r.title.is_some()).unwrap();
        assert_eq!(archived_row.title.as_deref(), Some("(archived)"));
        assert!(archived_row.last_event_id.is_none());
    }

    #[test]
    fn sessions_sorted_by_updated_at_descending() {
        let home = fixture_home();
        let proj = home.path().join(".claude").join("projects").join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("a.jsonl"), "{}\n").unwrap();
        std::fs::write(proj.join("b.jsonl"), "{}\n{}\n{}\n").unwrap();

        let rows = list_native_sessions(home.path(), Some(&["claude"]), None);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].updated_at >= rows[1].updated_at);
    }

    #[test]
    fn cwd_filter_matches_claude_encoding_and_row_cwd() {
        let home = fixture_home();
        let proj = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-joe-demo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("s1.jsonl"), "{}\n").unwrap();
        let other = home.path().join(".claude").join("projects").join("-tmp-other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("s2.jsonl"), "{}\n").unwrap();

        let rows = list_native_sessions(home.path(), Some(&["claude"]), Some("~/demo"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "s1");
    }

    #[test]
    fn deferred_sqlite_harnesses_return_no_rows() {
        let home = fixture_home();
        // Even with on-disk state, deferred readers must not enumerate.
        std::fs::create_dir_all(home.path().join(".opencode")).unwrap();
        std::fs::write(home.path().join(".opencode").join("opencode.db"), "").unwrap();
        let rows = list_native_sessions(home.path(), Some(&["opencode"]), None);
        assert!(rows.is_empty(), "sqlite readers are deferred by binding decision");
    }

    #[test]
    fn unknown_or_absent_harness_returns_nothing() {
        let home = fixture_home();
        assert!(list_native_sessions(home.path(), Some(&["kimi"]), None).is_empty());
        assert!(list_native_sessions(home.path(), Some(&["nope"]), None).is_empty());
    }

    #[test]
    fn join_key_format_matches_spec() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi-code")
            .join("sessions")
            .join("wd_a")
            .join("session_x1");
        std::fs::create_dir_all(&dir).unwrap();
        let rows = list_native_sessions(home.path(), Some(&["kimi"]), None);
        assert_eq!(rows[0].join_key(), "kimi+x1");
    }

    #[test]
    fn kimi_walker_extracts_last_prompt_and_wire_time() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi-code")
            .join("sessions")
            .join("wd_p")
            .join("session_s1");
        std::fs::create_dir_all(dir.join("agents").join("main")).unwrap();
        std::fs::write(
            dir.join("state.json"),
            r#"{"id":"session_s1","cwd":"/tmp/demo","updatedAt":1725974400000,"lastPrompt":"ship it"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("agents").join("main").join("wire.jsonl"),
            concat!(
                r#"{"type":"prompt.accepted","agentId":"main","promptId":"a","content":[{"type":"text","text":"older"}],"time":1725974300000}"#,
                "\n",
                r#"{"type":"turn.ended","agentId":"main","time":1725974350000}"#,
                "\n",
                r#"{"type":"prompt.accepted","agentId":"main","promptId":"b","content":[{"type":"text","text":"ship it"}],"time":1725974401234}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["kimi"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("ship it"));
        // Last prompt.accepted wins over state.json updatedAt.
        assert_eq!(rows[0].last_prompt_at, Some(1725974401234));
    }

    #[test]
    fn kimi_walker_falls_back_to_state_updated_at() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi-code")
            .join("sessions")
            .join("wd_q")
            .join("session_s2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("state.json"),
            r#"{"id":"session_s2","cwd":"/tmp/demo","updatedAt":1725974400000,"lastPrompt":"no wire events"}"#,
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["kimi"]), None);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("no wire events"));
        assert_eq!(rows[0].last_prompt_at, Some(1725974400000));
    }

    #[test]
    fn kimi_walker_falls_back_to_wire_text_without_state_last_prompt() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi-code")
            .join("sessions")
            .join("wd_r")
            .join("session_s3");
        std::fs::create_dir_all(dir.join("agents").join("main")).unwrap();
        // state.json has no lastPrompt.
        std::fs::write(
            dir.join("state.json"),
            r#"{"id":"session_s3","cwd":"/tmp/demo","updatedAt":1725974400000}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("agents").join("main").join("wire.jsonl"),
            concat!(
                r#"{"type":"prompt.accepted","agentId":"main","promptId":"a","content":[{"type":"text","text":"wire only prompt"}],"time":1725974401234}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["kimi"]), None);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("wire only prompt"));
        assert_eq!(rows[0].last_prompt_at, Some(1725974401234));
    }

    #[test]
    fn claude_walker_extracts_last_user_message() {
        let home = fixture_home();
        let proj = home.path().join(".claude").join("projects").join("-tmp-demo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("u1.jsonl"),
            concat!(
                r#"{"type":"user","timestamp":"2026-09-09T00:58:35.293Z","message":{"role":"user","content":"first ask"}}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-09-09T00:59:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-09-09T01:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"part one "},{"type":"text","text":"part two"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["claude"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("part one part two"));
        assert_eq!(rows[0].last_prompt_at, Some(1788915600000));
    }

    #[test]
    fn codex_walker_extracts_last_user_message() {
        let home = fixture_home();
        let live = home.path().join(".codex").join("sessions").join("2026").join("09").join("10");
        std::fs::create_dir_all(&live).unwrap();
        let uuid = "123e4567-e89b-42d3-a456-426614174000";
        std::fs::write(
            live.join(format!("rollout-2026-09-10T00-00-00-{uuid}.jsonl")),
            concat!(
                r#"{"timestamp":"2026-05-07T13:42:00.571Z","type":"event_msg","payload":{"type":"user_message","message":"earlier"}}"#,
                "\n",
                r#"{"timestamp":"2026-05-07T13:43:00.571Z","type":"event_msg","payload":{"type":"agent_message","message":"working on it"}}"#,
                "\n",
                r#"{"timestamp":"2026-05-07T13:44:00.571Z","type":"event_msg","payload":{"type":"user_message","message":"its not renderignt in the platform"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["codex"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].last_prompt.as_deref(),
            Some("its not renderignt in the platform")
        );
        assert_eq!(rows[0].last_prompt_at, Some(1778161440571));
    }

    #[test]
    fn qwen_walker_extracts_last_user_message_from_nested_chats() {
        let home = fixture_home();
        let chats = home
            .path()
            .join(".qwen")
            .join("projects")
            .join("-tmp-demo")
            .join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            chats.join("q1.jsonl"),
            concat!(
                r#"{"type":"user","timestamp":"2026-09-09T00:58:35.293Z","message":{"role":"user","content":"older qwen ask"}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-09-09T01:00:00.000Z","message":{"role":"user","content":"newer qwen ask"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["qwen"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("newer qwen ask"));
        assert_eq!(rows[0].last_prompt_at, Some(1788915600000));
    }

    #[test]
    fn grok_walker_joins_user_message_chunks() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".grok")
            .join("sessions")
            .join("%2Ftmp%2Fdemo")
            .join("01a07286-0714-7790-adc2-9dc39adcf562");
        std::fs::create_dir_all(&dir).unwrap();
        let chunk = |ts: u64, text: &str, update: &str| {
            format!(
                r#"{{"timestamp":{ts},"method":"session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"{update}","content":{{"type":"text","text":"{text}"}}}}}}}}"#
            )
        };
        let updates = format!(
            "{}\n{}\n{}\n",
            chunk(1788627781, "Reply with ", "user_message_chunk"),
            chunk(1788627782, "exactly GROK_BRAIN_OK", "user_message_chunk"),
            chunk(1788627783, "GROK_BRAIN_OK", "agent_message_chunk"),
        );
        std::fs::write(dir.join("updates.jsonl"), updates).unwrap();

        let rows = list_native_sessions(home.path(), Some(&["grok"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].last_prompt.as_deref(),
            Some("Reply with exactly GROK_BRAIN_OK")
        );
        // epoch seconds → ms.
        assert_eq!(rows[0].last_prompt_at, Some(1788627781000));
    }

    #[test]
    fn kimi_cli_walker_extracts_last_user_line() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".kimi")
            .join("sessions")
            .join("6df21bdc83aa9a9b38cc257a95a02a1e")
            .join("2c59bb21-4024-4751-afc9-e21258492f3a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("context.jsonl"),
            concat!(
                r#"{"role":"_system_prompt","content":"You are Kimi CLI."}"#,
                "\n",
                r#"{"role":"user","content":"first ask"}"#,
                "\n",
                r#"{"role":"assistant","content":"answer"}"#,
                "\n",
                r#"{"role":"user","content":"last ask"}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["kimi-cli"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("last ask"));
        // kimi-cli context lines carry no timestamp.
        assert_eq!(rows[0].last_prompt_at, None);
    }

    #[test]
    fn copilot_walker_extracts_last_user_message() {
        let home = fixture_home();
        let dir = home.path().join(".copilot").join("session-state").join("cp1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("events.jsonl"),
            concat!(
                r#"{"type":"assistant_message","message":"working on it","timestamp":"2026-09-10T11:59:00.000Z"}"#,
                "\n",
                r#"{"type":"user_message","message":"ship the fix","timestamp":"2026-09-10T12:00:00.000Z"}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["copilot"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("ship the fix"));
        assert_eq!(rows[0].last_prompt_at, Some(1789041600000));
    }

    #[test]
    fn pi_walker_extracts_nested_user_message() {
        let home = fixture_home();
        let dir = home.path().join(".pi").join("agent").join("sessions").join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("sess_42.jsonl"),
            concat!(
                r#"{"type":"message","timestamp":1788627782000,"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
                "\n",
                r#"{"type":"message","timestamp":1788627790000,"message":{"role":"user","content":[{"type":"text","text":"now do the next thing"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["pi"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "42");
        assert_eq!(
            rows[0].last_prompt.as_deref(),
            Some("now do the next thing")
        );
        assert_eq!(rows[0].last_prompt_at, Some(1788627790000));
    }

    #[test]
    fn cursor_walker_reuses_claude_shape() {
        let home = fixture_home();
        let transcripts = home
            .path()
            .join(".cursor")
            .join("projects")
            .join("-tmp-demo")
            .join("agent-transcripts");
        std::fs::create_dir_all(&transcripts).unwrap();
        std::fs::write(
            transcripts.join("c1.jsonl"),
            concat!(
                r#"{"type":"user","timestamp":"2026-09-09T01:00:00.000Z","message":{"role":"user","content":"cursor ask"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["cursor"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("cursor ask"));
        assert_eq!(rows[0].last_prompt_at, Some(1788915600000));
    }

    #[test]
    fn openhands_walker_scans_event_files() {
        let home = fixture_home();
        let events = home
            .path()
            .join(".openhands")
            .join("conversations")
            .join("oh1")
            .join("events");
        std::fs::create_dir_all(&events).unwrap();
        std::fs::write(
            events.join("1.json"),
            r#"{"id":1,"source":"agent","message":"on it","timestamp":"2026-09-10T11:59:00.000Z"}"#,
        )
        .unwrap();
        std::fs::write(
            events.join("2.json"),
            r#"{"id":2,"source":"user","message":"run the tests","timestamp":"2026-09-10T12:00:00.000Z"}"#,
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["openhands"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("run the tests"));
        assert_eq!(rows[0].last_prompt_at, Some(1789041600000));
    }

    #[test]
    fn muse_walker_extracts_last_user_line() {
        let home = fixture_home();
        let dir = home
            .path()
            .join(".local")
            .join("share")
            .join("muse")
            .join("sessions")
            .join("x")
            .join("y");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.jsonl"),
            concat!(
                r#"{"role":"user","content":"compose the melody","timestamp":1788627782000}"#,
                "\n",
                r#"{"role":"assistant","content":"here it is","timestamp":1788627783000}"#,
                "\n",
                r#"{"role":"user","content":"now transpose it","timestamp":1788627790000}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["muse"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("now transpose it"));
        assert_eq!(rows[0].last_prompt_at, Some(1788627790000));
    }

    #[test]
    fn vibe_walker_extracts_last_user_line() {
        let home = fixture_home();
        let ses = home
            .path()
            .join(".vibe")
            .join("logs")
            .join("session")
            .join("session_1_2");
        std::fs::create_dir_all(&ses).unwrap();
        std::fs::write(
            ses.join("messages.jsonl"),
            concat!(
                r#"{"role":"user","content":"earlier vibe ask","created_at":1788627782}"#,
                "\n",
                r#"{"role":"user","content":"latest vibe ask","created_at":1788627790}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["vibe"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "2");
        assert_eq!(rows[0].last_prompt.as_deref(), Some("latest vibe ask"));
        // epoch seconds → ms.
        assert_eq!(rows[0].last_prompt_at, Some(1788627790000));
    }

    #[test]
    fn gemini_walker_handles_single_line_json_array() {
        let home = fixture_home();
        let chats = home.path().join(".gemini").join("tmp").join("proj").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            chats.join("g1.json"),
            concat!(
                r#"[{"role":"user","parts":[{"text":"first gemini ask"}],"timestamp":"2026-09-09T01:00:00.000Z"},"#,
                r#"{"role":"model","parts":[{"text":"ok"}],"timestamp":"2026-09-09T01:01:00.000Z"},"#,
                r#"{"role":"user","parts":[{"text":"second gemini ask"}],"timestamp":"2026-09-09T01:02:00.000Z"}]"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["gemini"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("second gemini ask"));
        assert_eq!(rows[0].last_prompt_at, Some(1788915720000));
    }

    #[test]
    fn droid_walker_extracts_last_user_line() {
        let home = fixture_home();
        let sessions = home.path().join(".factory").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("d1.jsonl"),
            concat!(
                r#"{"role":"user","content":"old droid ask","ts":1788627782000}"#,
                "\n",
                r#"{"role":"assistant","content":"building","ts":1788627783000}"#,
                "\n",
                r#"{"role":"user","content":"new droid ask","ts":1788627790000}"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["droid"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("new droid ask"));
        assert_eq!(rows[0].last_prompt_at, Some(1788627790000));
    }

    #[test]
    fn cline_walker_reuses_history_parse_for_last_prompt() {
        let home = fixture_home();
        let task = home
            .path()
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("globalStorage")
            .join("saoudrizwan.claude-dev")
            .join("tasks")
            .join("task-1");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(
            task.join("api_conversation_history.json"),
            concat!(
                r#"[{"role":"user","content":"cline first","ts":1725974400000},"#,
                r#"{"role":"assistant","content":"on it"},"#,
                r#"{"role":"user","content":"cline last","ts":1725974500000}]"#,
                "\n",
            ),
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["cline"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title.as_deref(), Some("cline first"));
        assert_eq!(rows[0].last_prompt.as_deref(), Some("cline last"));
        assert_eq!(rows[0].last_prompt_at, Some(1725974500000));
    }

    #[test]
    fn amp_walker_reuses_thread_parse_for_last_prompt() {
        let home = fixture_home();
        let threads = home.path().join(".local").join("share").join("amp").join("threads");
        std::fs::create_dir_all(&threads).unwrap();
        std::fs::write(
            threads.join("T-abc.json"),
            r#"{"created_at":"2026-09-01T00:00:00.000Z","messages":[{"role":"user","content":[{"type":"text","text":"amp first"}]},{"role":"assistant","content":[{"type":"text","text":"thinking"}]},{"role":"user","content":[{"type":"text","text":"amp last"}]}]}"#,
        )
        .unwrap();

        let rows = list_native_sessions(home.path(), Some(&["amp"]), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_prompt.as_deref(), Some("amp last"));
        // amp messages carry no per-message timestamp.
        assert_eq!(rows[0].last_prompt_at, None);
    }
}
