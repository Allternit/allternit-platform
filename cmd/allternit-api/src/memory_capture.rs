//! Memory Drive grows from where the person already talks to AI, not only
//! from Allternit's own chat (ledger program phase 3):
//!
//! - **channels**: the bot owner's own messages on Telegram, WhatsApp,
//!   Discord, Teams, Slack, SMS and bot email. Messages from anyone else are
//!   never captured. Each one is recorded with its channel as the source and
//!   goes through the same model extraction a web chat turn does.
//! - **gizzi**: Gizzi CLI turns, recorded so the nightly Dream sees them
//!   (Gizzi already saves memories itself, so no second extraction here).
//! - **agent_folders**: off until turned on. Re-imports Claude Code, Codex,
//!   Hermes and OpenClaw memory files on this computer every few hours;
//!   memories already in the drive are skipped.
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
const FOLDER_SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub channels: bool,
    pub gizzi: bool,
    pub agent_folders: bool,
}

impl Default for Settings {
    /// On for places the person connected inside Allternit; reading other
    /// agents' files stays off until they turn it on.
    fn default() -> Self {
        Settings { channels: true, gizzi: true, agent_folders: false }
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
    let t = text.trim();
    // Commands ("/start", "/help") aren't about the person.
    if t.is_empty() || t.starts_with('/') {
        return None;
    }
    Some(t.chars().take(MAX_TURN_CHARS).collect())
}

/// Record the owner's own message from `source` with provenance `label`
/// (e.g. `telegram:chat/123`). Returns the observation id when recorded,
/// `None` when that source is switched off or there is nothing to keep.
pub fn record_turn(db: &DbHandle, owner: &str, source: Source, label: &str, text: &str) -> Result<Option<String>> {
    if owner.is_empty() {
        return Ok(None);
    }
    crate::memory_drive::validate_source(label).map_err(|e| ServiceError::Provenance(e.to_string()))?;
    let Some(text) = clean_turn(text) else { return Ok(None) };
    if !settings(&db.connect()?, owner).allows(source) {
        return Ok(None);
    }
    crate::memory_kernel_service::record_observation(db, owner, None, None, "turn_user", &text, Some(label))
        .map(Some)
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
        .map(|a| match crate::memory_drive_agent_import::apply(db, owner, home, a.id, None) {
            Ok((_, n)) => n,
            Err(ServiceError::Provenance(_)) => 0, // nothing new
            Err(e) => {
                tracing::warn!(agent = a.id, "agent folder sync failed: {e}");
                0
            }
        })
        .sum()
}

fn folder_sync_owners(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT user_id, value FROM runtime_settings WHERE key = ?1")?;
    let rows = stmt.query_map(params![KEY], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows
        .filter_map(|r| r.ok())
        .filter(|(_, v)| serde_json::from_str::<Settings>(v).map(|s| s.agent_folders).unwrap_or(false))
        .map(|(u, _)| u)
        .collect())
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
                let owners = db.connect().ok().and_then(|c| folder_sync_owners(&c).ok()).unwrap_or_default();
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

    #[test]
    fn settings_default_and_partial_json() {
        assert_eq!(Settings::default(), Settings { channels: true, gizzi: true, agent_folders: false });
        let s: Settings = serde_json::from_str(r#"{"agent_folders":true}"#).unwrap();
        assert_eq!(s, Settings { channels: true, gizzi: true, agent_folders: true });
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
