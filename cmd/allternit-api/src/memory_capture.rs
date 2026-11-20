//! Memory Drive grows from where the person already talks to AI, not only
//! from Allternit's own chat (ledger program phase 3):
//!
//! - **channels**: the bot owner's own messages on Telegram, WhatsApp,
//!   Discord, Teams, Slack, SMS and bot email. Messages from anyone else are
//!   never captured. Each one is recorded with its channel as the source and
//!   goes through the same model extraction a web chat turn does.
//! - **gizzi**: Gizzi CLI turns, recorded so the nightly Dream sees them
//!   (Gizzi already saves memories itself, so no second extraction here).
//! - **vendors**: on unless turned off. Saved memories and new chats from
//!   the person's own assistant logins (see `vendor_memory`).
//! - **agent_folders**: on unless turned off. Re-imports Claude Code, Codex,
//!   Hermes and OpenClaw memory files on this computer every few hours;
//!   memories already in the drive are skipped. Only the person this computer
//!   is paired as gets them (or someone who turned it on here), so a shared
//!   server's home folder never lands in anyone's drive.
//!
//! Switches live in `runtime_settings` under `memory_capture`. Every captured
//! memory keeps its source, so [`forget_source`] can take a source back out.
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::DbHandle;
use crate::memory_drive_service::{Result, ServiceError};

const KEY: &str = "memory_capture";
/// Longest captured message; longer ones are clipped.
const MAX_TURN_CHARS: usize = 4000;
/// Longest raw user turn kept for a person who turned Allternit Sleep on
/// (Sleep's source of truth; extraction input stays at `MAX_TURN_CHARS`).
pub const MAX_RAW_TURN_CHARS: usize = 64_000;
/// Tool rows: input and output are clipped separately, with a marker.
const MAX_TOOL_INPUT_CHARS: usize = 4_000;
const MAX_TOOL_OUTPUT_CHARS: usize = 16_000;
const FOLDER_SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub channels: bool,
    pub gizzi: bool,
    pub agent_folders: bool,
    /// Read saved memories and new chats from the person's own assistant
    /// logins (`vendor_memory`).
    pub vendors: bool,
}

impl Default for Settings {
    /// Everything on. Each one can be turned off, and its memories pruned
    /// with [`forget_source`].
    fn default() -> Self {
        Settings { channels: true, gizzi: true, agent_folders: true, vendors: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Channels,
    Gizzi,
    AgentFolders,
}

impl Settings {
    pub fn allows(&self, s: Source) -> bool {
        match s {
            Source::Channels => self.channels,
            Source::Gizzi => self.gizzi,
            Source::AgentFolders => self.agent_folders,
        }
    }
}

pub fn settings(conn: &Connection, owner: &str) -> Settings {
    conn.query_row("SELECT value FROM runtime_settings WHERE user_id=?1 AND key=?2", params![owner, KEY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn save(conn: &Connection, owner: &str, s: &Settings) -> Result<()> {
    conn.execute(
        "INSERT INTO runtime_settings (user_id, key, value, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP) \
         ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
        params![owner, KEY, serde_json::to_string(s).unwrap_or_default()],
    )?;
    Ok(())
}

fn clean_turn(text: &str) -> Option<String> {
    clean_turn_cap(text, MAX_TURN_CHARS)
}

fn clean_turn_cap(text: &str, cap: usize) -> Option<String> {
    let t = text.trim();
    // Commands ("/start", "/help") aren't about the person.
    if t.is_empty() || t.starts_with('/') {
        return None;
    }
    Some(t.chars().take(cap).collect())
}

/// `s` cut to `cap` chars; a cut ends with `[truncated N chars]`.
fn clip_marked(s: &str, cap: usize) -> String {
    let n = s.chars().count();
    if n <= cap {
        return s.to_string();
    }
    let head: String = s.chars().take(cap).collect();
    format!("{head}\n[truncated {} chars]", n - cap)
}

/// What a captured row is: the person's message, the assistant's reply, or a
/// tool call with its result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

impl Role {
    /// `None` role means an older client: a user turn.
    pub fn parse(s: Option<&str>) -> Option<Role> {
        match s.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("user") => Some(Role::User),
            Some("assistant") => Some(Role::Assistant),
            Some("tool") => Some(Role::Tool),
            Some(_) => None,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Role::User => "turn_user",
            Role::Assistant => "turn_assistant",
            Role::Tool => "turn_tool",
        }
    }
}

/// Result of a capture attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Capture {
    Recorded(String),
    /// Nothing stored, and why (`source_off`, `sleep_off`, `incognito`,
    /// `empty`, `repeat`, `no_owner`).
    Skipped(&'static str),
}

impl Capture {
    pub fn id(self) -> Option<String> {
        match self {
            Capture::Recorded(id) => Some(id),
            Capture::Skipped(_) => None,
        }
    }
}

/// The row text for a tool call: name, input (4k), then output (16k).
fn tool_content(name: &str, input: &str, output: &str) -> String {
    format!("{}\n{}\n---\n{}", name.trim(), clip_marked(input.trim(), MAX_TOOL_INPUT_CHARS), clip_marked(output.trim(), MAX_TOOL_OUTPUT_CHARS))
}

/// The session a `gizzi:session/<id>` label names.
fn gizzi_session(label: &str) -> Option<&str> {
    label.strip_prefix("gizzi:session/").filter(|s| !s.is_empty())
}

/// Record the owner's own message from `source` with provenance `label`
/// (e.g. `telegram:chat/123`). Returns the observation id when recorded,
/// `None` when that source is switched off or there is nothing to keep.
pub fn record_turn(db: &DbHandle, owner: &str, source: Source, label: &str, text: &str) -> Result<Option<String>> {
    record_row(db, owner, source, label, Role::User, text, None, "", "").map(Capture::id)
}

/// Record one row of a turn. User rows are as before, except that with Sleep
/// on the full text (up to `MAX_RAW_TURN_CHARS`) is kept. Assistant and tool
/// rows are recorded only with Sleep on. Nothing is recorded for an
/// incognito Gizzi session. Every role respects the per-source switches.
/// For tool rows `text` is unused: `tool_name`, `tool_input`, `tool_output`
/// make the content.
#[allow(clippy::too_many_arguments)]
pub fn record_row(
    db: &DbHandle,
    owner: &str,
    source: Source,
    label: &str,
    role: Role,
    text: &str,
    tool_name: Option<&str>,
    tool_input: &str,
    tool_output: &str,
) -> Result<Capture> {
    if owner.is_empty() {
        return Ok(Capture::Skipped("no_owner"));
    }
    crate::memory_drive::validate_source(label).map_err(|e| ServiceError::Provenance(e.to_string()))?;
    let conn = db.connect()?;
    if !settings(&conn, owner).allows(source) {
        return Ok(Capture::Skipped("source_off"));
    }
    if source == Source::Gizzi {
        if let Some(sid) = gizzi_session(label) {
            if db.is_session_ephemeral(sid).unwrap_or(false) {
                return Ok(Capture::Skipped("incognito"));
            }
        }
    }
    let sleep_on = crate::sleep::config::load(&conn, owner).map(|r| r.enabled()).unwrap_or(false);
    if role != Role::User && !sleep_on {
        return Ok(Capture::Skipped("sleep_off"));
    }
    let content = match role {
        Role::User => clean_turn_cap(text, if sleep_on { MAX_RAW_TURN_CHARS } else { MAX_TURN_CHARS }),
        Role::Assistant => {
            let t = text.trim();
            (!t.is_empty()).then(|| clip_marked(t, MAX_RAW_TURN_CHARS))
        }
        Role::Tool => {
            let name = tool_name.map(str::trim).unwrap_or("");
            (!name.is_empty()).then(|| tool_content(name, tool_input, tool_output))
        }
    };
    let Some(content) = content else { return Ok(Capture::Skipped("empty")) };
    let kind = role.kind();
    // A redelivered message (webhook retry, replay, client resend) is
    // captured once.
    let repeat: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memory_observations WHERE user_id = ?1 AND source = ?2 AND kind = ?3 AND content = ?4 AND timestamp >= datetime('now', '-10 minutes'))",
        params![owner, label, kind, content],
        |r| r.get(0),
    )?;
    if repeat {
        return Ok(Capture::Skipped("repeat"));
    }
    crate::memory_kernel_service::record_observation(db, owner, None, None, kind, &content, Some(label))
        .map(Capture::Recorded)
        .map_err(|e| ServiceError::Provenance(e.to_string()))
}

/// A channel message from the bot's owner: record it and extract memories
/// from it in the background, as a web chat turn would. Never blocks the
/// channel reply.
pub fn owner_channel_turn(db: &DbHandle, owner: &str, label: &str, text: &str) {
    match record_turn(db, owner, Source::Channels, label, text) {
        Ok(Some(obs)) => {
            tokio::spawn(crate::memory_extraction::extract_and_reconcile(db.clone(), owner.to_string(), None, obs, text.to_string()));
        }
        Ok(None) => {}
        Err(e) => tracing::debug!("memory capture skipped: {e}"),
    }
}

/// A channel label from provider and chat id, kept within the provenance
/// alphabet.
pub fn channel_label(provider: &str, chat: &str) -> String {
    let scheme: String = provider.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(32).collect();
    let scheme = if scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) { scheme } else { format!("channel-{scheme}") };
    crate::memory_drive_text_import::provenance_label(&scheme, &format!("chat/{}", if chat.is_empty() { "direct" } else { chat }))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceCount {
    /// The part of `source:` before the colon (`chatgpt`, `telegram`, `claude-code`, …).
    pub scheme: String,
    pub memories: i64,
}

/// How many memories in the personal drive came from each labelled source.
/// Session links (web chats) and the drive's own entries aren't listed.
pub fn counts(conn: &Connection, owner: &str) -> Result<Vec<SourceCount>> {
    let mut stmt = conn.prepare(
        "SELECT substr(e.source, 1, instr(e.source, ':') - 1) AS scheme, COUNT(*) FROM memory_drive_entries e \
         JOIN memory_drives d ON d.id = e.drive_id \
         WHERE d.kind = 'personal' AND d.scope_id = ?1 AND instr(e.source, ':') > 1 \
           AND e.source NOT LIKE 'http%' AND e.source NOT LIKE '/%' \
         GROUP BY scheme ORDER BY COUNT(*) DESC",
    )?;
    let rows = stmt
        .query_map(params![owner], |r| Ok(SourceCount { scheme: r.get(0)?, memories: r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().filter(|c| c.scheme != "allternit" && !c.scheme.is_empty()).collect())
}

/// Most memories one forget removes; run again for the rest.
pub const MAX_FORGET: usize = 2000;

/// Remove every memory whose source starts with `<scheme>:` from the personal
/// drive, as one commit. Like any delete, the lines stay in the drive's
/// history. Returns (revision, removed).
pub fn forget_source(db: &DbHandle, owner: &str, scheme: &str) -> Result<(Option<String>, usize)> {
    if scheme.is_empty() || scheme.len() > 32 || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) || scheme == "allternit" {
        return Err(ServiceError::Provenance("Choose a source to forget.".into()));
    }
    let facts: Vec<String> = {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT e.fact_id FROM memory_drive_entries e JOIN memory_drives d ON d.id = e.drive_id \
             WHERE d.kind = 'personal' AND d.scope_id = ?1 AND substr(e.source, 1, length(?2) + 1) = ?2 || ':' LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![owner, scheme, MAX_FORGET as i64], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        rows
    };
    if facts.is_empty() {
        return Ok((None, 0));
    }
    let out = crate::memory_drive_writer::commit_facts(db, owner, &[], &facts, &format!("Forget memories from {scheme}"))?
        .ok_or(ServiceError::NotFound)?;
    Ok((out.revision, facts.len()))
}

/// Import new memories from every agent folder on this computer. Already
/// remembered lines are skipped, so this is safe to repeat. Returns how many
/// were added.
pub fn sync_agent_folders(db: &DbHandle, owner: &str, home: &std::path::Path) -> usize {
    crate::memory_drive_agent_import::detect(home)
        .into_iter()
        .map(|a| match crate::memory_drive_agent_import::sync(db, owner, home, a.id) {
            Ok((_, n)) => n,
            Err(ServiceError::Provenance(_)) => 0, // nothing new
            Err(e) => {
                tracing::warn!(agent = a.id, "agent folder sync failed: {e}");
                0
            }
        })
        .sum()
}

/// Who gets this computer's agent folders: the owner it's paired as (on by
/// default, unless they turned it off) plus anyone who turned it on here.
fn folder_sync_owners(conn: &Connection, local_owner: Option<String>) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT user_id, value FROM runtime_settings WHERE key = ?1")?;
    let rows = stmt.query_map(params![KEY], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut owners: Vec<String> = rows
        .filter_map(|r| r.ok())
        .filter(|(_, v)| serde_json::from_str::<Settings>(v).map(|s| s.agent_folders).unwrap_or(false))
        .map(|(u, _)| u)
        .collect();
    if let Some(local) = local_owner {
        if settings(conn, &local).agent_folders {
            owners.push(local);
        }
    }
    owners.sort();
    owners.dedup();
    Ok(owners)
}

/// Every few hours, re-import agent folders for owners who turned that on.
/// Only meaningful where this API runs on the person's own computer.
pub fn spawn(db: DbHandle) {
    let db = Arc::new(db);
    tokio::spawn(async move {
        // Not at boot: start a few minutes after the app is up.
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + std::time::Duration::from_secs(5 * 60), FOLDER_SYNC_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(home) = crate::memory_drive_agent_import::home() else { continue };
            let db = db.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let local = crate::relay_auth::process_secret().paired_owner();
                let owners = db.connect().ok().and_then(|c| folder_sync_owners(&c, local).ok()).unwrap_or_default();
                for owner in owners {
                    let n = sync_agent_folders(&db, &owner, &home);
                    if n > 0 {
                        tracing::info!(added = n, "agent folder memories imported");
                    }
                }
            })
            .await;
        }
    });
}

/// Channels where the owner has a verified identity (the `verify <code>`
/// pairing, or a Telegram bot paired to its owner). Capture needs one.
pub fn verified_channels(conn: &Connection, owner: &str) -> Vec<String> {
    let mut out: Vec<String> = conn
        .prepare("SELECT DISTINCT channel FROM factory_owner_identities WHERE owner = ?1")
        .and_then(|mut s| s.query_map([owner], |r| r.get::<_, String>(0)).map(|it| it.filter_map(|r| r.ok()).collect()))
        .unwrap_or_default();
    let telegram: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_account_bindings WHERE owner = ?1 AND vendor = 'telegram' AND COALESCE(tg_owner_user_id, '') <> '')",
            [owner],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if telegram && !out.iter().any(|c| c == "telegram") {
        out.push("telegram".into());
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_valid_provenance() {
        for (p, c) in [("telegram", "12345"), ("whatsapp", "+1 (651) 555-0100"), ("slack", "C01/thread 9"), ("9bad", ""), ("email", "a@b.co")] {
            let l = channel_label(p, c);
            crate::memory_drive::validate_source(&l).unwrap_or_else(|e| panic!("{l}: {e:?}"));
        }
        assert_eq!(channel_label("telegram", "12345"), "telegram:chat/12345");
        assert_eq!(channel_label("email", ""), "email:chat/direct");
    }

    #[test]
    fn commands_and_blank_turns_are_not_captured() {
        assert_eq!(clean_turn("/start abc"), None);
        assert_eq!(clean_turn("   "), None);
        assert_eq!(clean_turn("I moved to Saint Paul").as_deref(), Some("I moved to Saint Paul"));
        assert_eq!(clean_turn(&"x".repeat(MAX_TURN_CHARS + 10)).map(|t| t.len()), Some(MAX_TURN_CHARS));
    }

    fn sleep_on(db: &DbHandle, owner: &str) {
        let conn = db.connect().unwrap();
        crate::sleep::config::set_owner(&conn, owner, crate::sleep::config::Group::Setting, &[("enabled".to_string(), serde_json::json!(true))]).unwrap();
    }

    fn rows(db: &DbHandle, owner: &str, kind: &str) -> Vec<String> {
        let conn = db.connect().unwrap();
        let mut st = conn.prepare("SELECT content FROM memory_observations WHERE user_id = ?1 AND kind = ?2").unwrap();
        let v = st.query_map(params![owner, kind], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect();
        v
    }

    const LABEL: &str = "gizzi:session/s1";

    #[test]
    fn sleep_off_keeps_todays_capture() {
        let db = DbHandle::new_memory().unwrap();
        let big = "a".repeat(10_000);
        let r = record_row(&db, "u1", Source::Gizzi, LABEL, Role::Assistant, "hello", None, "", "").unwrap();
        assert_eq!(r, Capture::Skipped("sleep_off"));
        let r = record_row(&db, "u1", Source::Gizzi, LABEL, Role::Tool, "", Some("bash"), "{}", "out").unwrap();
        assert_eq!(r, Capture::Skipped("sleep_off"));
        assert!(record_turn(&db, "u1", Source::Gizzi, LABEL, &big).unwrap().is_some());
        assert!(rows(&db, "u1", "turn_assistant").is_empty() && rows(&db, "u1", "turn_tool").is_empty());
        assert_eq!(rows(&db, "u1", "turn_user")[0].len(), MAX_TURN_CHARS);
    }

    #[test]
    fn sleep_on_records_assistant_tool_and_full_user_text() {
        let db = DbHandle::new_memory().unwrap();
        sleep_on(&db, "u1");
        let big = "b".repeat(10_000);
        assert!(record_turn(&db, "u1", Source::Gizzi, LABEL, &big).unwrap().is_some());
        assert_eq!(rows(&db, "u1", "turn_user")[0].len(), 10_000);
        assert!(matches!(record_row(&db, "u1", Source::Gizzi, LABEL, Role::Assistant, "Sure, done.", None, "", "").unwrap(), Capture::Recorded(_)));
        assert_eq!(rows(&db, "u1", "turn_assistant"), vec!["Sure, done.".to_string()]);
        let out = "o".repeat(40_000);
        assert!(matches!(record_row(&db, "u1", Source::Gizzi, LABEL, Role::Tool, "", Some("bash"), "{\"cmd\":\"ls\"}", &out).unwrap(), Capture::Recorded(_)));
        let t = &rows(&db, "u1", "turn_tool")[0];
        assert!(t.starts_with("bash\n{\"cmd\":\"ls\"}\n---\n"), "{t}");
        assert!(t.ends_with("[truncated 24000 chars]"), "{}", &t[t.len() - 40..]);
        // A resend of the same row inside ten minutes is dropped.
        assert_eq!(record_row(&db, "u1", Source::Gizzi, LABEL, Role::Assistant, "Sure, done.", None, "", "").unwrap(), Capture::Skipped("repeat"));
        // Another person with Sleep off gets nothing.
        assert_eq!(record_row(&db, "u2", Source::Gizzi, LABEL, Role::Assistant, "hi", None, "", "").unwrap(), Capture::Skipped("sleep_off"));
    }

    #[test]
    fn incognito_sessions_and_switched_off_sources_record_nothing() {
        let db = DbHandle::new_memory().unwrap();
        sleep_on(&db, "u1");
        db.connect().unwrap().execute("INSERT INTO ephemeral_sessions (session_id) VALUES ('s-inc')", []).unwrap();
        for role in [Role::User, Role::Assistant] {
            let r = record_row(&db, "u1", Source::Gizzi, "gizzi:session/s-inc", role, "secret", None, "", "").unwrap();
            assert_eq!(r, Capture::Skipped("incognito"));
        }
        save(&db.connect().unwrap(), "u1", &Settings { gizzi: false, ..Settings::default() }).unwrap();
        assert_eq!(record_row(&db, "u1", Source::Gizzi, LABEL, Role::Assistant, "x", None, "", "").unwrap(), Capture::Skipped("source_off"));
    }

    #[test]
    fn settings_default_and_partial_json() {
        assert_eq!(Settings::default(), Settings { channels: true, gizzi: true, agent_folders: true, vendors: true });
        let s: Settings = serde_json::from_str(r#"{"agent_folders":false}"#).unwrap();
        assert_eq!(s, Settings { channels: true, gizzi: true, agent_folders: false, vendors: true });
    }

    #[test]
    fn forget_refuses_odd_schemes() {
        let db = DbHandle::new_memory().unwrap();
        for bad in ["", "allternit", "a b", "x:y", &"a".repeat(40)] {
            assert!(forget_source(&db, "u1", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn verified_channels_come_from_pairing_or_a_telegram_owner() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE factory_owner_identities (account_id TEXT, channel TEXT, owner TEXT, identity TEXT, verified_at TEXT);
             CREATE TABLE provider_account_bindings (id TEXT, owner TEXT, vendor TEXT, tg_owner_user_id TEXT);
             INSERT INTO provider_account_bindings VALUES ('b1','u1','telegram',NULL), ('b2','u1','sms',NULL);",
        )
        .unwrap();
        assert!(verified_channels(&conn, "u1").is_empty());
        conn.execute("INSERT INTO factory_owner_identities VALUES ('b2','sms','u1','+16515550100','2026-10-17')", []).unwrap();
        conn.execute("UPDATE provider_account_bindings SET tg_owner_user_id='42' WHERE id='b1'", []).unwrap();
        assert_eq!(verified_channels(&conn, "u1"), vec!["sms", "telegram"]);
        assert!(verified_channels(&conn, "u2").is_empty());
    }
}
