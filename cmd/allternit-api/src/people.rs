//! One person across every channel (digital twin layer 1).
//!
//! Before this, a thread was tied to one external chat and the sender was a line of
//! text, so Dana on Telegram and Dana by SMS were strangers with separate memories.
//!
//! * **People + identities.** A person has identities: `(provider, external id)`
//!   pairs. The same phone number on SMS and WhatsApp, or the same email, links by
//!   exact match with no question asked; a likely match (same name and organisation)
//!   becomes a proposal the owner confirms. Merge and split are the owner's undo.
//! * **Inbound.** [`sender`] resolves an inbound message's sender; a direct message
//!   lands in the person's one thread per bot ([`person_thread`]). Chats that already
//!   had a thread of their own become sub-threads of it ([`backfill`]); group chats
//!   stay their own threads and only get the sender's name ([`Sender::name`]).
//! * **Replies go where the person is.** [`choose_reply`]: a channel the bot asks for
//!   (only one the person has already written to us on, so consent rules are
//!   unchanged), else the one the owner pinned, else the one they just used.
//! * **Memory.** [`remember`] / [`facts`] are keyed by person id, not by chat, so
//!   every channel sees the same notes.
//!
//! Routes (all under `/api/v1`, owner-scoped):
//! `GET/POST /people`, `GET/PATCH /people/:id`, `POST /people/:id/identities`,
//! `POST /people/:id/merge`, `POST /people/:id/split`, `POST /people/:id/facts`,
//! `POST /people/:id/message`, `GET /people-links`, `POST /people-links/:id/confirm|dismiss`.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_gateway_routes::{id, now};
use crate::auth::AuthUser;
use crate::channel_gateway::{BindingRow, Inbound, InboundKind};
use crate::db::DbHandle;
use crate::thread_routes::ThreadRuntime;
use crate::AppState;

// ---------------------------------------------------------------- identity keys

/// Digits of a phone-ish string with a leading `+`: `"+44 7700-900123"`, `"447700900123"`
/// and `"447700900123@s.whatsapp.net"` are all `+447700900123`.
pub fn normalize_phone(raw: &str) -> Option<String> {
    let head = raw.split('@').next().unwrap_or(raw);
    let digits: String = head.chars().filter(char::is_ascii_digit).collect();
    (digits.len() >= 7 && digits.len() <= 15).then(|| format!("+{digits}"))
}

/// The handle a provider's sender id is, normalised so the same human on two channels
/// compares equal: `(kind, external id)`.
pub fn identity_key(provider: &str, user: &str, workspace: Option<&str>) -> Option<(&'static str, String)> {
    let user = user.trim();
    if user.is_empty() {
        return None;
    }
    Some(match provider {
        "sms" | "whatsapp" | "whatsapp-personal" => ("phone", normalize_phone(user)?),
        "email" => ("email", user.to_lowercase()),
        "telegram" => ("telegram", user.to_string()),
        "slack" => ("slack", match workspace.filter(|w| !w.is_empty()) {
            Some(team) => format!("{team}:{user}"),
            None => user.to_string(),
        }),
        "discord" => ("discord", user.to_string()),
        "teams" => ("teams", user.to_string()),
        "allternit" => ("allternit", user.to_string()),
        _ => return None,
    })
}

/// Handles of these kinds are the same human wherever they appear.
fn exact_kind(kind: &str) -> bool {
    matches!(kind, "phone" | "email" | "allternit")
}

/// True for a one-to-one conversation (the person's own chat with the bot).
pub fn is_direct_conversation(provider: &str, channel: &str, conversation: &str, workspace: Option<&str>) -> bool {
    match provider {
        "sms" | "email" => true,
        "telegram" => !channel.starts_with('-'),
        "slack" => channel.starts_with('D'),
        "discord" => workspace.map_or(true, str::is_empty),
        "teams" => channel.starts_with("a:"),
        "whatsapp" => true,
        "whatsapp-personal" => !conversation.contains("@g.us"),
        _ => false,
    }
}

fn default_name(provider: &str, kind: &str, external: &str) -> String {
    let place = match provider {
        "sms" => "SMS",
        "whatsapp" | "whatsapp-personal" => "WhatsApp",
        "telegram" => "Telegram",
        "slack" => "Slack",
        "discord" => "Discord",
        "teams" => "Teams",
        "email" => "Email",
        _ => "Person",
    };
    match kind {
        "phone" | "email" => external.to_string(),
        _ => format!("{place} {}", external.rsplit(':').next().unwrap_or(external)),
    }
}

fn handle(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

// ---------------------------------------------------------------- storage

#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub id: String,
    pub display_name: String,
    pub name_source: String,
    pub avatar: Option<String>,
    pub notes: Option<String>,
    pub org: Option<String>,
    pub reply_pin: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: String,
    pub person_id: String,
    pub provider: String,
    pub kind: String,
    pub external_id: String,
    pub label: Option<String>,
    pub confidence: f64,
    pub source: String,
    pub last_seen_at: Option<String>,
}

const P_COLS: &str = "id, display_name, name_source, avatar, notes, org, reply_pin";
const I_COLS: &str = "id, person_id, provider, kind, external_id, label, confidence, source, last_seen_at";

fn person_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Person> {
    Ok(Person { id: r.get(0)?, display_name: r.get(1)?, name_source: r.get(2)?, avatar: r.get(3)?, notes: r.get(4)?, org: r.get(5)?, reply_pin: r.get(6)? })
}

fn identity_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Identity> {
    Ok(Identity { id: r.get(0)?, person_id: r.get(1)?, provider: r.get(2)?, kind: r.get(3)?, external_id: r.get(4)?, label: r.get(5)?, confidence: r.get(6)?, source: r.get(7)?, last_seen_at: r.get(8)? })
}

/// Follow `merged_into` to the person that is live now.
fn canonical(conn: &Connection, owner: &str, person: &str) -> String {
    let mut cur = person.to_string();
    for _ in 0..16 {
        match conn.query_row("SELECT merged_into FROM people WHERE id = ?1 AND owner = ?2", params![cur, owner], |r| r.get::<_, Option<String>>(0)) {
            Ok(Some(next)) if !next.is_empty() => cur = next,
            _ => break,
        }
    }
    cur
}

pub fn get_person(db: &DbHandle, owner: &str, person: &str) -> Option<Person> {
    let conn = db.connect().ok()?;
    let live = canonical(&conn, owner, person);
    conn.query_row(&format!("SELECT {P_COLS} FROM people WHERE id = ?1 AND owner = ?2"), params![live, owner], person_row).ok()
}

pub fn identities_of(db: &DbHandle, owner: &str, person: &str) -> Vec<Identity> {
    let Ok(conn) = db.connect() else { return vec![] };
    conn.prepare(&format!("SELECT {I_COLS} FROM person_identities WHERE owner = ?1 AND person_id = ?2 ORDER BY COALESCE(last_seen_at, '') DESC, created_at, id"))
        .and_then(|mut q| q.query_map(params![owner, person], identity_row)?.collect())
        .unwrap_or_default()
}

pub fn list_people(db: &DbHandle, owner: &str) -> Vec<Person> {
    let Ok(conn) = db.connect() else { return vec![] };
    conn.prepare(&format!("SELECT {P_COLS} FROM people WHERE owner = ?1 AND merged_into IS NULL ORDER BY display_name COLLATE NOCASE, id"))
        .and_then(|mut q| q.query_map(params![owner], person_row)?.collect())
        .unwrap_or_default()
}

fn new_person(conn: &Connection, owner: &str, name: &str, name_source: &str) -> rusqlite::Result<String> {
    let pid = id("per");
    let t = now();
    conn.execute(
        "INSERT INTO people (id, owner, display_name, name_source, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?5)",
        params![pid, owner, name, name_source, t],
    )?;
    Ok(pid)
}

fn add_identity(conn: &Connection, owner: &str, person: &str, provider: &str, kind: &str, external: &str, source: &str, confidence: f64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO person_identities (id, owner, person_id, provider, kind, external_id, confidence, source, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id("pid"), owner, person, provider, kind, external, confidence, source, now()],
    )?;
    Ok(())
}

/// The live person who already owns this handle, on any channel of an exact kind or on
/// this one.
fn find_owner_of(conn: &Connection, owner: &str, provider: &str, kind: &str, external: &str) -> Option<(String, bool)> {
    let own: Option<String> = conn
        .query_row("SELECT person_id FROM person_identities WHERE owner = ?1 AND provider = ?2 AND external_id = ?3", params![owner, provider, external], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    if let Some(p) = own {
        return Some((canonical(conn, owner, &p), true));
    }
    if !exact_kind(kind) {
        return None;
    }
    let other: Option<String> = conn
        .query_row("SELECT person_id FROM person_identities WHERE owner = ?1 AND kind = ?2 AND external_id = ?3 ORDER BY created_at LIMIT 1", params![owner, kind, external], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    other.map(|p| (canonical(conn, owner, &p), false))
}

// ---------------------------------------------------------------- inbound

/// Who sent an inbound message, as far as people know.
#[derive(Debug, Clone, PartialEq)]
pub struct Sender {
    pub person: String,
    pub name: String,
    pub direct: bool,
}

/// Resolve (and in a direct chat, create) the person behind an inbound message.
/// A group member we don't know yet stays unknown: a big channel must not fill the
/// owner's People list.
pub fn sender(db: &DbHandle, owner: &str, provider: &str, e: &Inbound) -> Option<Sender> {
    if e.own || e.kind != InboundKind::Message {
        return None;
    }
    let (kind, ext) = identity_key(provider, e.user.as_deref()?, e.workspace.as_deref().filter(|_| provider == "slack"))?;
    let direct = is_direct_conversation(provider, &e.channel, &e.conversation, e.workspace.as_deref());
    let conn = db.connect().ok()?;
    let known = find_owner_of(&conn, owner, provider, kind, &ext);
    let person = match known {
        Some((p, true)) => p,
        Some((p, false)) => {
            // Same phone/email seen on another channel: the same human, linked on the spot.
            add_identity(&conn, owner, &p, provider, kind, &ext, "exact_match", 1.0).ok()?;
            p
        }
        None if direct => {
            let p = new_person(&conn, owner, &default_name(provider, kind, &ext), "auto").ok()?;
            add_identity(&conn, owner, &p, provider, kind, &ext, "inbound", 1.0).ok()?;
            p
        }
        None => return None,
    };
    let _ = conn.execute("UPDATE person_identities SET last_seen_at = ?1 WHERE owner = ?2 AND provider = ?3 AND external_id = ?4", params![now(), owner, provider, ext]);
    let name: String = conn.query_row("SELECT display_name FROM people WHERE id = ?1", params![person], |r| r.get(0)).ok()?;
    Some(Sender { person, name, direct })
}

/// The person's one thread for `bot_id`: made on first use, found after. Returns its
/// session. Chat threads from before people existed move under it.
pub async fn person_thread<R: ThreadRuntime>(db: &DbHandle, rt: &R, owner: &str, person: &str, bot_id: &str, provider: &str, conversation: &str, name: &str, first_text: &str) -> Result<String, String> {
    let existing: Option<String> = db
        .connect()
        .map_err(|e| e.to_string())?
        .query_row("SELECT thread_id FROM person_threads WHERE owner = ?1 AND person_id = ?2 AND bot_id = ?3 AND role = 'main'", params![owner, person, bot_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if existing.is_none() {
        // A thread already open on this very conversation (a phone call came first and opened it)
        // becomes the person's main thread, so calls and texts keep sharing one thread.
        let conn = db.connect().map_err(|e| e.to_string())?;
        let open: Option<(String, String)> = conn
            .query_row(
                "SELECT id, current_session_id FROM bot_threads WHERE bot_id = ?1 AND user_id = ?2 AND json_extract(origin, '$.channelKey') = ?3
                   AND status NOT IN ('done', 'failed') AND current_session_id IS NOT NULL
                   AND id NOT IN (SELECT thread_id FROM person_threads) ORDER BY updated_at DESC LIMIT 1",
                params![bot_id, owner, conversation],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((thread, session)) = open {
            conn.execute("INSERT OR IGNORE INTO person_threads (thread_id, owner, person_id, bot_id, role, created_at) VALUES (?1,?2,?3,?4,'main',?5)", params![thread, owner, person, bot_id, now()])
                .map_err(|e| e.to_string())?;
            adopt_subthreads(&conn, owner, person, bot_id, &thread);
            return Ok(session);
        }
    }
    let title: String = name.chars().take(80).collect();
    let session = crate::thread_routes::channel_thread(db, rt, bot_id, provider, &format!("person:{person}"), &title, first_text).await?;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let thread: String = conn.query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![session], |r| r.get(0)).map_err(|e| e.to_string())?;
    if existing.is_none() {
        conn.execute("INSERT OR IGNORE INTO person_threads (thread_id, owner, person_id, bot_id, role, created_at) VALUES (?1,?2,?3,?4,'main',?5)", params![thread, owner, person, bot_id, now()])
            .map_err(|e| e.to_string())?;
        adopt_subthreads(&conn, owner, person, bot_id, &thread);
    }
    Ok(session)
}

/// Chat threads this person already had with this bot hang under the main thread.
fn adopt_subthreads(conn: &Connection, owner: &str, person: &str, bot_id: &str, main: &str) {
    let _ = conn.execute(
        "UPDATE bot_threads SET parent_thread_id = ?1 WHERE id IN
           (SELECT thread_id FROM person_threads WHERE owner = ?2 AND person_id = ?3 AND bot_id = ?4 AND role = 'sub') AND id <> ?1 AND parent_thread_id IS NULL",
        params![main, owner, person, bot_id],
    );
}

/// A conversation bound to a person thread that is no longer this sender's main thread
/// (the identity was split off, or the person was merged) follows the sender.
pub async fn rebind_if_moved<R: ThreadRuntime>(db: &DbHandle, rt: &R, owner: &str, binding: &BindingRow, s: &Sender, bot_id: &str, provider: &str, text: &str) -> Result<Option<BindingRow>, String> {
    if !s.direct {
        return Ok(None);
    }
    let conn = db.connect().map_err(|e| e.to_string())?;
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT t.person_id, t.role, json_extract(b.origin, '$.channelKey') FROM person_threads t JOIN bot_threads b ON b.id = t.thread_id WHERE t.thread_id = ?1 AND t.owner = ?2",
            params![binding.thread_id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((owner_person, role, key)) = row else { return Ok(None) };
    let was_main = key.as_deref().is_some_and(|k| k.starts_with("person:"));
    // A chat thread from before people existed keeps its history; only person threads move.
    if role == "sub" && !was_main {
        return Ok(None);
    }
    if role == "main" && canonical(&conn, owner, &owner_person) == s.person {
        return Ok(None);
    }
    drop(conn);
    let session = person_thread(db, rt, owner, &s.person, bot_id, provider, &binding.conversation, &s.name, text).await?;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let thread: String = conn.query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![session], |r| r.get(0)).map_err(|e| e.to_string())?;
    conn.execute("UPDATE channel_conversation_bindings SET thread_id = ?1, updated_at = ?2 WHERE id = ?3", params![thread, now(), binding.id]).map_err(|e| e.to_string())?;
    Ok(crate::channel_gateway::find_binding(db, &binding.provider, &binding.conversation))
}

// ---------------------------------------------------------------- existing threads

/// Turn the chats that existed before people into people. Each direct binding whose
/// inbound messages all came from one sender becomes that sender's person (made if
/// new, joined by exact match if known) and its thread becomes a sub-thread. Safe to
/// run again: bindings already claimed are skipped. Returns the bindings it adopted.
pub fn backfill(db: &DbHandle, owner: Option<&str>) -> usize {
    let Ok(conn) = db.connect() else { return 0 };
    type Row = (String, String, String, String, Option<String>, Option<String>, String, String);
    let rows: Vec<Row> = conn
        .prepare(
            "SELECT b.id, b.owner, b.thread_id, b.provider, b.external_channel_id, b.external_workspace_id, b.external_conversation_id, t.bot_id
             FROM channel_conversation_bindings b JOIN bot_threads t ON t.id = b.thread_id
             WHERE (?1 IS NULL OR b.owner = ?1) AND b.thread_id NOT IN (SELECT thread_id FROM person_threads)",
        )
        .and_then(|mut q| q.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?.collect())
        .unwrap_or_default();
    let mut adopted = 0;
    for (binding, own, thread, provider, channel, workspace, conversation, bot) in rows {
        if !is_direct_conversation(&provider, channel.as_deref().unwrap_or(""), &conversation, workspace.as_deref()) {
            continue;
        }
        let users: Vec<String> = conn
            .prepare("SELECT DISTINCT json_extract(detail_json, '$.user') FROM channel_message_log WHERE binding_id = ?1 AND direction = 'inbound' AND kind = 'message' AND json_extract(detail_json, '$.user') IS NOT NULL")
            .and_then(|mut q| q.query_map(params![binding], |r| r.get::<_, String>(0))?.collect())
            .unwrap_or_default();
        let [user] = users.as_slice() else { continue };
        let Some((kind, ext)) = identity_key(&provider, user, workspace.as_deref().filter(|_| provider == "slack")) else { continue };
        let person = match find_owner_of(&conn, &own, &provider, kind, &ext) {
            Some((p, true)) => p,
            Some((p, false)) => {
                let _ = add_identity(&conn, &own, &p, &provider, kind, &ext, "exact_match", 1.0);
                p
            }
            None => {
                let Ok(p) = new_person(&conn, &own, &default_name(&provider, kind, &ext), "auto") else { continue };
                let _ = add_identity(&conn, &own, &p, &provider, kind, &ext, "inbound", 1.0);
                p
            }
        };
        let seen: Option<String> = conn.query_row("SELECT MAX(created_at) FROM channel_message_log WHERE binding_id = ?1 AND direction = 'inbound'", params![binding], |r| r.get(0)).ok().flatten();
        let _ = conn.execute("UPDATE person_identities SET last_seen_at = COALESCE(?1, last_seen_at) WHERE owner = ?2 AND provider = ?3 AND external_id = ?4 AND (last_seen_at IS NULL OR last_seen_at < ?1)", params![seen, own, provider, ext]);
        let _ = conn.execute("INSERT OR IGNORE INTO person_threads (thread_id, owner, person_id, bot_id, role, created_at) VALUES (?1,?2,?3,?4,'sub',?5)", params![thread, own, person, bot, now()]);
        let main: Option<String> = conn
            .query_row("SELECT thread_id FROM person_threads WHERE owner = ?1 AND person_id = ?2 AND bot_id = ?3 AND role = 'main'", params![own, person, bot], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        if let Some(main) = main {
            adopt_subthreads(&conn, &own, &person, &bot, &main);
        }
        adopted += 1;
    }
    adopted
}

// ---------------------------------------------------------------- link, merge, split

#[derive(Debug, PartialEq)]
pub enum LinkOutcome {
    Linked,
    /// The handle already belongs to this person.
    Already,
    /// The handle belongs to someone else: the owner decides (a proposal was queued).
    Conflict(String),
}

/// Add a handle to a person (an invite, an imported contact, or typed by the owner).
pub fn link_identity(db: &DbHandle, owner: &str, person: &str, provider: &str, value: &str, source: &str) -> Result<LinkOutcome, String> {
    let (kind, ext) = identity_key(provider, value, None).ok_or("that isn't a valid handle for that channel")?;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let person = canonical(&conn, owner, person);
    match find_owner_of(&conn, owner, provider, kind, &ext) {
        Some((p, here)) if p == person => {
            if here {
                return Ok(LinkOutcome::Already);
            }
            add_identity(&conn, owner, &person, provider, kind, &ext, source, 1.0).map_err(|e| e.to_string())?;
            Ok(LinkOutcome::Linked)
        }
        Some((other, _)) => {
            propose(&conn, owner, &person, &other, &format!("{provider} {ext} is already linked to another person"), 0.95);
            Ok(LinkOutcome::Conflict(other))
        }
        None => {
            add_identity(&conn, owner, &person, provider, kind, &ext, source, 1.0).map_err(|e| e.to_string())?;
            Ok(LinkOutcome::Linked)
        }
    }
}

fn propose(conn: &Connection, owner: &str, a: &str, b: &str, reason: &str, score: f64) {
    if a == b {
        return;
    }
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    let _ = conn.execute(
        "INSERT OR IGNORE INTO person_link_proposals (id, owner, person_a, person_b, reason, score, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![id("plk"), owner, a, b, reason, score, now()],
    );
}

/// Queue proposals for people who share a name and an organisation but no handle.
/// Dismissed and accepted pairs are never asked again. Returns how many are open.
pub fn propose_links(db: &DbHandle, owner: &str) -> usize {
    let Ok(conn) = db.connect() else { return 0 };
    let people: Vec<(String, String, String)> = conn
        .prepare("SELECT id, display_name, COALESCE(org, '') FROM people WHERE owner = ?1 AND merged_into IS NULL AND name_source <> 'auto'")
        .and_then(|mut q| q.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
        .unwrap_or_default();
    for (i, a) in people.iter().enumerate() {
        for b in &people[i + 1..] {
            let (na, oa) = (handle(&a.1), handle(&a.2));
            if !na.is_empty() && !oa.is_empty() && na == handle(&b.1) && oa == handle(&b.2) {
                propose(&conn, owner, &a.0, &b.0, "same name and organisation", 0.6);
            }
        }
    }
    conn.query_row("SELECT COUNT(*) FROM person_link_proposals WHERE owner = ?1 AND status = 'open'", params![owner], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
}

/// Fold `from` into `into`: identities, facts and threads move; `from` stays as a
/// tombstone so references to it still resolve. The owner's undo is [`split`].
pub fn merge(db: &DbHandle, owner: &str, into: &str, from: &str) -> Result<(), String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let (into, from) = (canonical(&conn, owner, into), canonical(&conn, owner, from));
    if into == from {
        return Err("those are already the same person".into());
    }
    let (Some(a), Some(b)) = (
        conn.query_row(&format!("SELECT {P_COLS} FROM people WHERE id = ?1 AND owner = ?2"), params![into, owner], person_row).ok(),
        conn.query_row(&format!("SELECT {P_COLS} FROM people WHERE id = ?1 AND owner = ?2"), params![from, owner], person_row).ok(),
    ) else {
        return Err("person not found".into());
    };
    let moved: Vec<String> = identities_of(db, owner, &from).into_iter().map(|i| i.id).collect();
    conn.execute("UPDATE person_identities SET person_id = ?1 WHERE owner = ?2 AND person_id = ?3", params![into, owner, from]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE person_facts SET person_id = ?1 WHERE owner = ?2 AND person_id = ?3", params![into, owner, from]).map_err(|e| e.to_string())?;
    // Threads: when both people have a main thread for a bot, the old one becomes a sub-thread.
    let mains: Vec<(String, String)> = conn
        .prepare("SELECT thread_id, bot_id FROM person_threads WHERE owner = ?1 AND person_id = ?2 AND role = 'main'")
        .and_then(|mut q| q.query_map(params![owner, from], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(|e| e.to_string())?;
    for (thread, bot) in mains {
        let target: Option<String> = conn
            .query_row("SELECT thread_id FROM person_threads WHERE owner = ?1 AND person_id = ?2 AND bot_id = ?3 AND role = 'main'", params![owner, into, bot], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        match target {
            Some(main) => {
                conn.execute("UPDATE person_threads SET person_id = ?1, role = 'sub' WHERE thread_id = ?2", params![into, thread]).map_err(|e| e.to_string())?;
                conn.execute("UPDATE bot_threads SET parent_thread_id = ?1 WHERE id = ?2", params![main, thread]).map_err(|e| e.to_string())?;
            }
            None => {
                conn.execute("UPDATE person_threads SET person_id = ?1 WHERE thread_id = ?2", params![into, thread]).map_err(|e| e.to_string())?;
                conn.execute("UPDATE bot_threads SET origin = json_set(COALESCE(origin, '{}'), '$.channelKey', ?1) WHERE id = ?2", params![format!("person:{into}"), thread]).map_err(|e| e.to_string())?;
            }
        }
    }
    // The sub-threads of `from` re-parent under the surviving main threads.
    let subs: Vec<(String, String)> = conn
        .prepare("SELECT thread_id, bot_id FROM person_threads WHERE owner = ?1 AND person_id = ?2 AND role = 'sub'")
        .and_then(|mut q| q.query_map(params![owner, from], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(|e| e.to_string())?;
    for (thread, bot) in subs {
        conn.execute("UPDATE person_threads SET person_id = ?1 WHERE thread_id = ?2", params![into, thread]).map_err(|e| e.to_string())?;
        if let Some(main) = main_of(&conn, owner, &into, &bot) {
            adopt_subthreads(&conn, owner, &into, &bot, &main);
        }
    }
    let notes = match (a.notes.as_deref().filter(|n| !n.is_empty()), b.notes.as_deref().filter(|n| !n.is_empty())) {
        (Some(x), Some(y)) => Some(format!("{x}\n\n{y}")),
        (x, y) => x.or(y).map(str::to_string),
    };
    let take_name = a.name_source == "auto" && b.name_source != "auto";
    conn.execute(
        "UPDATE people SET notes = ?1, avatar = COALESCE(avatar, ?2), org = COALESCE(NULLIF(org, ''), ?3), display_name = CASE WHEN ?4 THEN ?5 ELSE display_name END,
           name_source = CASE WHEN ?4 THEN ?6 ELSE name_source END, updated_at = ?7 WHERE id = ?8",
        params![notes, b.avatar, b.org, take_name, b.display_name, b.name_source, now(), into],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("UPDATE people SET merged_into = ?1, updated_at = ?2 WHERE id = ?3", params![into, now(), from]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE person_link_proposals SET status = 'accepted' WHERE owner = ?1 AND status = 'open' AND ((person_a = ?2 AND person_b = ?3) OR (person_a = ?3 AND person_b = ?2))", params![owner, into, from])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM person_link_proposals WHERE owner = ?1 AND status = 'open' AND (person_a = ?2 OR person_b = ?2)", params![owner, from]).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO person_merges (id, owner, into_id, from_id, from_name, identity_ids, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![id("pmg"), owner, into, from, b.display_name, json!(moved).to_string(), now()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn main_of(conn: &Connection, owner: &str, person: &str, bot: &str) -> Option<String> {
    conn.query_row("SELECT thread_id FROM person_threads WHERE owner = ?1 AND person_id = ?2 AND bot_id = ?3 AND role = 'main'", params![owner, person, bot], |r| r.get(0)).ok()
}

/// Take one identity off a person and make it a person of its own. A person's last
/// identity can't be split off. The conversations of the identity follow it on the
/// next message ([`rebind_if_moved`]).
pub fn split(db: &DbHandle, owner: &str, identity_id: &str) -> Result<String, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let ident = conn
        .query_row(&format!("SELECT {I_COLS} FROM person_identities WHERE id = ?1 AND owner = ?2"), params![identity_id, owner], identity_row)
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("identity not found")?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM person_identities WHERE owner = ?1 AND person_id = ?2", params![owner, ident.person_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    if count < 2 {
        return Err("that is the person's only identity".into());
    }
    let pid = new_person(&conn, owner, &default_name(&ident.provider, &ident.kind, &ident.external_id), "auto").map_err(|e| e.to_string())?;
    conn.execute("UPDATE person_identities SET person_id = ?1, source = 'manual' WHERE id = ?2", params![pid, identity_id]).map_err(|e| e.to_string())?;
    // A pair that was merged and is now split must not come straight back as a proposal.
    conn.execute(
        "INSERT OR REPLACE INTO person_link_proposals (id, owner, person_a, person_b, reason, score, status, created_at) VALUES (?1,?2,?3,?4,'split by the owner',0,'dismissed',?5)",
        params![id("plk"), owner, ident.person_id.clone().min(pid.clone()), ident.person_id.clone().max(pid.clone()), now()],
    )
    .map_err(|e| e.to_string())?;
    // Backfilled chat threads of this handle's conversations follow it.
    conn.execute(
        "UPDATE person_threads SET person_id = ?1 WHERE owner = ?2 AND thread_id IN
           (SELECT b.thread_id FROM channel_conversation_bindings b JOIN channel_message_log l ON l.binding_id = b.id
            WHERE b.owner = ?2 AND b.provider = ?3 AND json_extract(l.detail_json, '$.user') = ?4) AND role = 'sub'",
        params![pid, owner, ident.provider, ident.external_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(pid)
}

pub fn update_person(db: &DbHandle, owner: &str, person: &str, patch: &Value) -> Result<Person, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let person = canonical(&conn, owner, person);
    let cur = conn.query_row(&format!("SELECT {P_COLS} FROM people WHERE id = ?1 AND owner = ?2"), params![person, owner], person_row).map_err(|_| "person not found".to_string())?;
    let text = |k: &str, old: Option<String>| -> Option<String> {
        match patch.get(k) {
            Some(Value::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            Some(Value::Null) => None,
            _ => old,
        }
    };
    let name = match patch.get("displayName").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) => (n.to_string(), "owner".to_string()),
        None => (cur.display_name.clone(), cur.name_source.clone()),
    };
    let pin = match patch.get("replyPin") {
        Some(Value::String(p)) if !p.is_empty() => {
            let linked: i64 = conn.query_row("SELECT COUNT(*) FROM person_identities WHERE owner = ?1 AND person_id = ?2 AND provider = ?3", params![owner, person, p], |r| r.get(0)).unwrap_or(0);
            if linked == 0 {
                return Err(format!("this person has no {p} identity to reply on"));
            }
            Some(p.clone())
        }
        Some(_) => None,
        None => cur.reply_pin.clone(),
    };
    conn.execute(
        "UPDATE people SET display_name = ?1, name_source = ?2, avatar = ?3, notes = ?4, org = ?5, reply_pin = ?6, updated_at = ?7 WHERE id = ?8",
        params![name.0, name.1, text("avatar", cur.avatar), text("notes", cur.notes), text("org", cur.org), pin, now(), person],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    propose_links(db, owner);
    get_person(db, owner, &person).ok_or_else(|| "person not found".into())
}

/// Create a person from a contact (a phone contact, an invite) and link every handle.
/// A handle that already belongs to someone links nothing new and queues a proposal.
pub fn import_contact(db: &DbHandle, owner: &str, name: &str, org: Option<&str>, handles: &[(String, String)], source: &str) -> Result<String, String> {
    let keys: Vec<(String, &'static str, String)> = handles.iter().filter_map(|(p, v)| identity_key(p, v, None).map(|(k, e)| (p.clone(), k, e))).collect();
    if keys.is_empty() {
        return Err("no valid handle in this contact".into());
    }
    let conn = db.connect().map_err(|e| e.to_string())?;
    let existing: Vec<String> = {
        let mut v: Vec<String> = keys.iter().filter_map(|(p, k, e)| find_owner_of(&conn, owner, p, k, e).map(|(pid, _)| pid)).collect();
        v.dedup();
        v.sort();
        v.dedup();
        v
    };
    let person = match existing.first() {
        Some(p) => p.clone(),
        None => new_person(&conn, owner, name, "contact").map_err(|e| e.to_string())?,
    };
    for other in existing.iter().skip(1) {
        propose(&conn, owner, &person, other, "one contact holds handles of two people", 0.95);
    }
    for (p, k, e) in &keys {
        if find_owner_of(&conn, owner, p, k, e).map_or(true, |(pid, _)| pid == person) {
            add_identity(&conn, owner, &person, p, k, e, source, 1.0).map_err(|e| e.to_string())?;
        }
    }
    let auto: bool = conn.query_row("SELECT name_source = 'auto' FROM people WHERE id = ?1", params![person], |r| r.get(0)).unwrap_or(false);
    if auto && !name.trim().is_empty() {
        let _ = conn.execute("UPDATE people SET display_name = ?1, name_source = 'contact', updated_at = ?2 WHERE id = ?3", params![name.trim(), now(), person]);
    }
    if let Some(org) = org.filter(|o| !o.trim().is_empty()) {
        let _ = conn.execute("UPDATE people SET org = COALESCE(NULLIF(org, ''), ?1) WHERE id = ?2", params![org.trim(), person]);
    }
    drop(conn);
    propose_links(db, owner);
    Ok(person)
}

// ---------------------------------------------------------------- replies

#[derive(Debug, Clone)]
pub struct ReplyChoice {
    pub binding: BindingRow,
    /// `requested` | `pinned` | `last_used` | `fallback`
    pub reason: &'static str,
}

/// Every conversation of this person that can be written to: one binding per channel,
/// the most recently active. A channel is only here when the person has written to us
/// on it, so reaching them there never needs a new consent.
pub fn reachable(db: &DbHandle, owner: &str, person: &str) -> Vec<BindingRow> {
    let Ok(conn) = db.connect() else { return vec![] };
    let ids: Vec<String> = conn
        .prepare(
            "SELECT b.id FROM channel_conversation_bindings b JOIN person_threads t ON t.thread_id = b.thread_id
             WHERE t.owner = ?1 AND t.person_id = ?2 AND b.read_only = 0 AND b.bidirectional = 1 ORDER BY b.updated_at DESC, b.created_at DESC",
        )
        .and_then(|mut q| q.query_map(params![owner, person], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .filter_map(|bid| {
            let (provider, conversation): (String, String) = conn.query_row("SELECT provider, external_conversation_id FROM channel_conversation_bindings WHERE id = ?1", params![bid], |r| Ok((r.get(0)?, r.get(1)?))).ok()?;
            seen.insert(provider.clone()).then(|| crate::channel_gateway::find_binding(db, &provider, &conversation)).flatten()
        })
        .collect()
}

/// Where a reply to `person` goes. `arrived` is the conversation the message came in
/// on (None for a bot-started message); `requested` is a channel the bot asks for.
pub fn choose_reply(db: &DbHandle, owner: &str, person: &str, arrived: Option<&BindingRow>, requested: Option<&str>) -> Option<ReplyChoice> {
    let options = reachable(db, owner, person);
    let on = |provider: &str| options.iter().find(|b| b.provider == provider).cloned();
    if let Some(b) = requested.filter(|p| !p.is_empty()).and_then(on) {
        return Some(ReplyChoice { binding: b, reason: "requested" });
    }
    if let Some(b) = get_person(db, owner, person).and_then(|p| p.reply_pin).and_then(|p| on(&p)) {
        return Some(ReplyChoice { binding: b, reason: "pinned" });
    }
    if let Some(a) = arrived {
        return Some(ReplyChoice { binding: a.clone(), reason: "last_used" });
    }
    let last = identities_of(db, owner, person).into_iter().find(|i| i.last_seen_at.is_some()).map(|i| i.provider);
    if let Some(b) = last.and_then(|p| on(&p)) {
        return Some(ReplyChoice { binding: b, reason: "last_used" });
    }
    options.into_iter().next().map(|binding| ReplyChoice { binding, reason: "fallback" })
}

// ---------------------------------------------------------------- memory

pub fn remember(db: &DbHandle, owner: &str, person: &str, bot_id: Option<&str>, fact: &str, thread: Option<&str>) -> Result<String, String> {
    let fact = fact.trim();
    if fact.is_empty() {
        return Err("fact is required".into());
    }
    let conn = db.connect().map_err(|e| e.to_string())?;
    let person = canonical(&conn, owner, person);
    conn.query_row("SELECT 1 FROM people WHERE id = ?1 AND owner = ?2", params![person, owner], |_| Ok(())).map_err(|_| "person not found".to_string())?;
    let dup: i64 = conn.query_row("SELECT COUNT(*) FROM person_facts WHERE owner = ?1 AND person_id = ?2 AND fact = ?3", params![owner, person, fact], |r| r.get(0)).unwrap_or(0);
    if dup > 0 {
        return Ok(person);
    }
    conn.execute(
        "INSERT INTO person_facts (id, owner, person_id, bot_id, fact, source_thread_id, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![id("pfa"), owner, person, bot_id, fact, thread, now()],
    )
    .map_err(|e| e.to_string())?;
    Ok(person)
}

pub fn facts(db: &DbHandle, owner: &str, person: &str, limit: usize) -> Vec<String> {
    let Ok(conn) = db.connect() else { return vec![] };
    let person = canonical(&conn, owner, person);
    conn.prepare("SELECT fact FROM person_facts WHERE owner = ?1 AND person_id = ?2 ORDER BY created_at DESC, rowid DESC LIMIT ?3")
        .and_then(|mut q| q.query_map(params![owner, person, limit as i64], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default()
}

/// Facts with their ids, for the owner to read and remove.
pub fn fact_items(db: &DbHandle, owner: &str, person: &str, limit: usize) -> Vec<Value> {
    let Ok(conn) = db.connect() else { return vec![] };
    let person = canonical(&conn, owner, person);
    conn.prepare("SELECT id, fact, bot_id, created_at FROM person_facts WHERE owner = ?1 AND person_id = ?2 ORDER BY created_at DESC, rowid DESC LIMIT ?3")
        .and_then(|mut q| {
            q.query_map(params![owner, person, limit as i64], |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "fact": r.get::<_, String>(1)?, "botId": r.get::<_, Option<String>>(2)?, "createdAt": r.get::<_, Option<String>>(3)? }))
            })?
            .collect()
        })
        .unwrap_or_default()
}

/// Remove one fact; false when it isn't the owner's.
pub fn forget(db: &DbHandle, owner: &str, fact_id: &str) -> bool {
    db.connect().ok().and_then(|c| c.execute("DELETE FROM person_facts WHERE id = ?1 AND owner = ?2", params![fact_id, owner]).ok()).unwrap_or(0) > 0
}

/// What `bot` may know about a person. On a Platform API project runtime (owner
/// `platform:…`) one runtime holds many end-customer accounts, so a bot sees only
/// facts its own account's bots learned; elsewhere every fact of the owner.
pub fn facts_for_bot(db: &DbHandle, owner: &str, person: &str, bot: Option<&str>, limit: usize) -> Vec<String> {
    if !owner.starts_with("platform:") {
        return facts(db, owner, person, limit);
    }
    let Some(bot) = bot else { return vec![] };
    let Ok(conn) = db.connect() else { return vec![] };
    let person = canonical(&conn, owner, person);
    conn.prepare(
        "SELECT fact FROM person_facts WHERE owner = ?1 AND person_id = ?2 AND bot_id IN (
             SELECT id FROM agents WHERE user_id = ?1 AND json_extract(config, '$.platformAgent.accountId') =
                 (SELECT json_extract(config, '$.platformAgent.accountId') FROM agents WHERE id = ?3 AND user_id = ?1))
         ORDER BY created_at DESC, rowid DESC LIMIT ?4",
    )
    .and_then(|mut q| q.query_map(params![owner, person, bot, limit as i64], |r| r.get::<_, String>(0))?.collect())
    .unwrap_or_default()
}

/// The line a bot turn starts with: who wrote, on which channel, and what is already
/// known about them from every other channel (the bot's account only, on a project runtime).
pub fn turn_prefix(db: &DbHandle, owner: &str, bot: Option<&str>, provider: &str, s: Option<&Sender>, raw_user: Option<&str>, text: &str) -> String {
    let Some(s) = s else { return format!("[{provider} from {}] {text}", raw_user.unwrap_or("someone")) };
    let mut out = format!("[{provider} from {}] {text}", s.name);
    let known = facts_for_bot(db, owner, &s.person, bot, 12);
    if !known.is_empty() {
        out.push_str(&format!("\n\n(Known about {}, from every channel: {})", s.name, known.join("; ")));
    }
    out
}

// ---------------------------------------------------------------- bot tools

/// Tools a bot calls while it works (served on the runtime's internal MCP endpoint beside
/// `allternit_mail.*` and the phone tools). The bot acts for its owner, so it reads and writes
/// the owner's people directly; vendor bots go through `twin_propose` instead.
pub fn mcp_tools() -> Vec<Value> {
    let person = json!({ "type": "string", "minLength": 1, "description": "The person's id (pe_…) or their name as shown in the conversation." });
    vec![
        json!({
            "name": "people_lookup",
            "title": "Look someone up",
            "description": "Find people the owner knows by name, email or phone number. Returns each match with what is already known about them (from every channel) and how to reach them.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "minLength": 1 } }, "required": ["query"], "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "people_remember",
            "title": "Remember something about a person",
            "description": "Save one short fact about a person the owner deals with (a preference, a role, a date that matters), so every channel and every bot knows it next time. Only facts the person shared or that are plainly useful; never passwords, payment details or health information.",
            "inputSchema": { "type": "object", "properties": { "person": person, "fact": { "type": "string", "minLength": 1, "maxLength": 500 }, "agent_id": { "type": "string" }, "thread_id": { "type": "string" } }, "required": ["person", "fact"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }
        }),
        json!({
            "name": "people_add_contact",
            "title": "Add a contact",
            "description": "Add a person with an email address and/or phone number. If one of them already belongs to someone the owner knows, that person is updated instead of creating a duplicate.",
            "inputSchema": { "type": "object", "properties": { "name": { "type": "string", "minLength": 1 }, "org": { "type": "string" }, "email": { "type": "string" }, "phone": { "type": "string" } }, "required": ["name"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }
        }),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, "people_lookup" | "people_remember" | "people_add_contact")
}

const SENSITIVE: [&str; 9] = ["password", "passcode", "pin code", "credit card", "card number", "cvv", "social security", "ssn", "bank account"];

/// A person by id, or by a name that matches exactly one person (case-insensitive).
fn resolve_person(db: &DbHandle, owner: &str, person: &str) -> Result<Person, String> {
    let person = person.trim();
    if let Some(p) = get_person(db, owner, person) {
        return Ok(p);
    }
    let matches: Vec<Person> = list_people(db, owner).into_iter().filter(|p| p.display_name.trim().eq_ignore_ascii_case(person)).collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => Err(format!("No one called \"{person}\" yet. Use people_lookup, or people_add_contact to add them.")),
        _ => Err(format!(
            "More than one person is called \"{person}\": {}. Pass the id.",
            matches.iter().map(|p| format!("{} ({}{})", p.id, p.display_name, p.org.as_deref().map(|o| format!(", {o}")).unwrap_or_default())).collect::<Vec<_>>().join("; ")
        )),
    }
}

fn person_summary(db: &DbHandle, owner: &str, p: &Person) -> Value {
    let reach: Vec<Value> = identities_of(db, owner, &p.id).into_iter().map(|i| json!({ "channel": i.provider, "handle": i.label.unwrap_or(i.external_id) })).collect();
    json!({ "id": p.id, "name": p.display_name, "org": p.org, "known": facts(db, owner, &p.id, 12), "reach": reach })
}

pub fn call_mcp_tool(db: &DbHandle, owner: &str, name: &str, args: &Value) -> Result<Value, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    match name {
        "people_lookup" => {
            let q = s("query").ok_or("query is required")?.to_lowercase();
            let digits: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
            let found: Vec<Value> = list_people(db, owner)
                .into_iter()
                .filter(|p| {
                    p.display_name.to_lowercase().contains(&q)
                        || p.org.as_deref().is_some_and(|o| o.to_lowercase().contains(&q))
                        || identities_of(db, owner, &p.id).iter().any(|i| {
                            i.external_id.to_lowercase().contains(&q) || (digits.len() >= 7 && i.external_id.chars().filter(|c| c.is_ascii_digit()).collect::<String>().contains(&digits))
                        })
                })
                .take(5)
                .map(|p| person_summary(db, owner, &p))
                .collect();
            Ok(json!({ "people": found }))
        }
        "people_remember" => {
            let fact = s("fact").ok_or("fact is required")?;
            if fact.chars().count() > 500 {
                return Err("Keep a fact under 500 characters.".into());
            }
            let lower = fact.to_lowercase();
            if SENSITIVE.iter().any(|w| lower.contains(w)) {
                return Err("That looks like a secret or payment detail; it isn't saved.".into());
            }
            let p = resolve_person(db, owner, s("person").ok_or("person is required")?)?;
            remember(db, owner, &p.id, s("agent_id"), fact, s("thread_id"))?;
            Ok(json!({ "saved": true, "person": { "id": p.id, "name": p.display_name } }))
        }
        "people_add_contact" => {
            let name = s("name").ok_or("name is required")?;
            let mut handles = Vec::new();
            if let Some(e) = s("email") {
                handles.push(("email".to_string(), e.to_string()));
            }
            if let Some(ph) = s("phone") {
                handles.push(("sms".to_string(), ph.to_string()));
            }
            if handles.is_empty() {
                return Err("Give an email address or a phone number.".into());
            }
            let id = import_contact(db, owner, name, s("org"), &handles, "bot")?;
            let p = get_person(db, owner, &id).ok_or("person not found")?;
            Ok(person_summary(db, owner, &p))
        }
        other => Err(format!("unknown people tool: {other}")),
    }
}

// ---------------------------------------------------------------- routes

pub fn people_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/people", get(list_h).post(create_h))
        .route("/people/:id", get(get_h).patch(patch_h))
        .route("/people/:id/identities", post(identity_h))
        .route("/people/:id/merge", post(merge_h))
        .route("/people/:id/split", post(split_h))
        .route("/people/:id/facts", post(fact_h))
        .route("/people/:id/facts/:fact_id", axum::routing::delete(forget_h))
        .route("/people/:id/message", post(message_h))
        .route("/people-links", get(links_h))
        .route("/people-links/:id/confirm", post(confirm_h))
        .route("/people-links/:id/dismiss", post(dismiss_h))
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

fn identity_json(i: &Identity) -> Value {
    json!({ "id": i.id, "provider": i.provider, "kind": i.kind, "externalId": i.external_id, "label": i.label, "confidence": i.confidence, "source": i.source, "lastSeenAt": i.last_seen_at })
}

pub fn person_json(db: &DbHandle, owner: &str, p: &Person) -> Value {
    let identities = identities_of(db, owner, &p.id);
    let last = identities.iter().find(|i| i.last_seen_at.is_some());
    let threads: Vec<Value> = db
        .connect()
        .ok()
        .and_then(|c| {
            c.prepare("SELECT thread_id, bot_id, role FROM person_threads WHERE owner = ?1 AND person_id = ?2 ORDER BY role, created_at")
                .and_then(|mut q| q.query_map(params![owner, p.id], |r| Ok(json!({ "threadId": r.get::<_, String>(0)?, "botId": r.get::<_, String>(1)?, "role": r.get::<_, String>(2)? })))?.collect())
                .ok()
        })
        .unwrap_or_default();
    let mut channels: Vec<&str> = identities.iter().map(|i| i.provider.as_str()).collect();
    channels.sort();
    channels.dedup();
    json!({
        "id": p.id, "displayName": p.display_name, "nameSource": p.name_source, "avatar": p.avatar, "notes": p.notes, "org": p.org, "replyPin": p.reply_pin,
        "identities": identities.iter().map(identity_json).collect::<Vec<_>>(),
        "channels": channels,
        "lastChannel": last.map(|i| i.provider.clone()), "lastSeenAt": last.and_then(|i| i.last_seen_at.clone()),
        "threads": threads,
        "facts": fact_items(db, owner, &p.id, 50),
    })
}

async fn list_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    backfill(&st.db, Some(&user.user_id));
    propose_links(&st.db, &user.user_id);
    let people: Vec<Value> = list_people(&st.db, &user.user_id).iter().map(|p| person_json(&st.db, &user.user_id, p)).collect();
    Json(json!({ "people": people })).into_response()
}

async fn get_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>) -> Response {
    let Some(p) = get_person(&st.db, &user.user_id, &pid) else { return err(StatusCode::NOT_FOUND, "not_found", "person not found") };
    let mut v = person_json(&st.db, &user.user_id, &p);
    v["facts"] = json!(facts(&st.db, &user.user_id, &p.id, 50));
    Json(v).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandleBody {
    provider: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    display_name: String,
    org: Option<String>,
    #[serde(default)]
    identities: Vec<HandleBody>,
    source: Option<String>,
}

async fn create_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<CreateBody>) -> Response {
    if b.display_name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "name_required", "displayName is required");
    }
    let source = match b.source.as_deref() {
        Some(s @ ("contact" | "invite")) => s,
        _ => "manual",
    };
    let handles: Vec<(String, String)> = b.identities.into_iter().map(|h| (h.provider, h.value)).collect();
    let person = if handles.is_empty() {
        let Ok(conn) = st.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "db", "database unavailable") };
        match new_person(&conn, &user.user_id, b.display_name.trim(), "owner") {
            Ok(p) => {
                if let Some(org) = b.org.as_deref().filter(|o| !o.trim().is_empty()) {
                    let _ = conn.execute("UPDATE people SET org = ?1 WHERE id = ?2", params![org.trim(), p]);
                }
                p
            }
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db", &e.to_string()),
        }
    } else {
        match import_contact(&st.db, &user.user_id, &b.display_name, b.org.as_deref(), &handles, source) {
            Ok(p) => p,
            Err(e) => return err(StatusCode::BAD_REQUEST, "bad_contact", &e),
        }
    };
    propose_links(&st.db, &user.user_id);
    match get_person(&st.db, &user.user_id, &person) {
        Some(p) => (StatusCode::CREATED, Json(person_json(&st.db, &user.user_id, &p))).into_response(),
        None => err(StatusCode::INTERNAL_SERVER_ERROR, "db", "person vanished"),
    }
}

async fn patch_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>, Json(patch): Json<Value>) -> Response {
    match update_person(&st.db, &user.user_id, &pid, &patch) {
        Ok(p) => Json(person_json(&st.db, &user.user_id, &p)).into_response(),
        Err(e) if e == "person not found" => err(StatusCode::NOT_FOUND, "not_found", &e),
        Err(e) => err(StatusCode::BAD_REQUEST, "bad_request", &e),
    }
}

async fn identity_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>, Json(b): Json<HandleBody>) -> Response {
    if get_person(&st.db, &user.user_id, &pid).is_none() {
        return err(StatusCode::NOT_FOUND, "not_found", "person not found");
    }
    match link_identity(&st.db, &user.user_id, &pid, &b.provider, &b.value, "manual") {
        Ok(LinkOutcome::Conflict(other)) => (StatusCode::CONFLICT, Json(json!({ "error": "belongs_to_another_person", "personId": other, "message": "that handle is already linked to someone else; merge the two people if they are the same" }))).into_response(),
        Ok(o) => {
            let p = get_person(&st.db, &user.user_id, &pid).expect("checked above");
            Json(json!({ "linked": o == LinkOutcome::Linked, "person": person_json(&st.db, &user.user_id, &p) })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, "bad_handle", &e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeBody {
    from_id: String,
}

async fn merge_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>, Json(b): Json<MergeBody>) -> Response {
    match merge(&st.db, &user.user_id, &pid, &b.from_id) {
        Ok(()) => match get_person(&st.db, &user.user_id, &pid) {
            Some(p) => Json(person_json(&st.db, &user.user_id, &p)).into_response(),
            None => err(StatusCode::NOT_FOUND, "not_found", "person not found"),
        },
        Err(e) if e == "person not found" => err(StatusCode::NOT_FOUND, "not_found", &e),
        Err(e) => err(StatusCode::BAD_REQUEST, "bad_request", &e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SplitBody {
    identity_id: String,
}

async fn split_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>, Json(b): Json<SplitBody>) -> Response {
    let belongs = identities_of(&st.db, &user.user_id, &pid).iter().any(|i| i.id == b.identity_id);
    if !belongs {
        return err(StatusCode::NOT_FOUND, "not_found", "that identity isn't this person's");
    }
    match split(&st.db, &user.user_id, &b.identity_id) {
        Ok(new_id) => {
            let (Some(a), Some(n)) = (get_person(&st.db, &user.user_id, &pid), get_person(&st.db, &user.user_id, &new_id)) else { return err(StatusCode::INTERNAL_SERVER_ERROR, "db", "person vanished") };
            Json(json!({ "person": person_json(&st.db, &user.user_id, &a), "split": person_json(&st.db, &user.user_id, &n) })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, "bad_request", &e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactBody {
    fact: String,
    bot_id: Option<String>,
    thread_id: Option<String>,
}

async fn fact_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>, Json(b): Json<FactBody>) -> Response {
    match remember(&st.db, &user.user_id, &pid, b.bot_id.as_deref(), &b.fact, b.thread_id.as_deref()) {
        Ok(p) => Json(json!({ "personId": p, "facts": fact_items(&st.db, &user.user_id, &p, 50) })).into_response(),
        Err(e) if e == "person not found" => err(StatusCode::NOT_FOUND, "not_found", &e),
        Err(e) => err(StatusCode::BAD_REQUEST, "bad_request", &e),
    }
}

async fn forget_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((_pid, fact_id)): Path<(String, String)>) -> Response {
    if forget(&st.db, &user.user_id, &fact_id) {
        Json(json!({ "removed": true })).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "not_found", "no such note")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageBody {
    text: String,
    /// A channel the bot asks for; honoured only when the person has written to us there.
    channel: Option<String>,
}

/// Send a message to a person on the channel [`choose_reply`] picks.
async fn message_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>, Json(b): Json<MessageBody>) -> Response {
    let Some(p) = get_person(&st.db, &user.user_id, &pid) else { return err(StatusCode::NOT_FOUND, "not_found", "person not found") };
    if b.text.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "text_required", "text is required");
    }
    let Some(choice) = choose_reply(&st.db, &user.user_id, &p.id, None, b.channel.as_deref()) else {
        return err(StatusCode::CONFLICT, "no_channel", "this person hasn't written to you on any channel yet, so there's nowhere to reply");
    };
    let Some(tx) = crate::channel_transports::transport_for(&st, &choice.binding) else { return err(StatusCode::SERVICE_UNAVAILABLE, "not_configured", "that channel isn't connected") };
    let thread = choice.binding.external_thread.clone().unwrap_or_default();
    match crate::channel_gateway::post_reply(&st.db, tx.as_ref(), &choice.binding, &thread, b.text.trim()).await {
        Ok(()) => Json(json!({ "sent": true, "provider": choice.binding.provider, "reason": choice.reason })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, "send_failed", &e),
    }
}

async fn links_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    propose_links(&st.db, &user.user_id);
    let Ok(conn) = st.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "db", "database unavailable") };
    let rows: Vec<(String, String, String, String, f64)> = conn
        .prepare("SELECT id, person_a, person_b, reason, score FROM person_link_proposals WHERE owner = ?1 AND status = 'open' ORDER BY score DESC, created_at")
        .and_then(|mut q| q.query_map(params![user.user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect())
        .unwrap_or_default();
    let links: Vec<Value> = rows
        .into_iter()
        .filter_map(|(pid, a, b, reason, score)| {
            let (pa, pb) = (get_person(&st.db, &user.user_id, &a)?, get_person(&st.db, &user.user_id, &b)?);
            (pa.id != pb.id).then(|| json!({ "id": pid, "reason": reason, "score": score, "a": person_json(&st.db, &user.user_id, &pa), "b": person_json(&st.db, &user.user_id, &pb) }))
        })
        .collect();
    Json(json!({ "links": links })).into_response()
}

fn proposal(db: &DbHandle, owner: &str, pid: &str) -> Option<(String, String)> {
    db.connect().ok()?.query_row("SELECT person_a, person_b FROM person_link_proposals WHERE id = ?1 AND owner = ?2 AND status = 'open'", params![pid, owner], |r| Ok((r.get(0)?, r.get(1)?))).ok()
}

async fn confirm_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>) -> Response {
    let Some((a, b)) = proposal(&st.db, &user.user_id, &pid) else { return err(StatusCode::NOT_FOUND, "not_found", "no such open proposal") };
    // Keep the person who has been around longer.
    let first_seen = |p: &str| st.db.connect().ok().and_then(|c| c.query_row("SELECT created_at FROM people WHERE id = ?1", params![p], |r| r.get::<_, String>(0)).ok()).unwrap_or_default();
    let (into, from) = if first_seen(&a) <= first_seen(&b) { (a, b) } else { (b, a) };
    match merge(&st.db, &user.user_id, &into, &from) {
        Ok(()) => match get_person(&st.db, &user.user_id, &into) {
            Some(p) => Json(person_json(&st.db, &user.user_id, &p)).into_response(),
            None => err(StatusCode::NOT_FOUND, "not_found", "person not found"),
        },
        Err(e) => err(StatusCode::BAD_REQUEST, "bad_request", &e),
    }
}

async fn dismiss_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(pid): Path<String>) -> Response {
    let n = st.db.connect().ok().and_then(|c| c.execute("UPDATE person_link_proposals SET status = 'dismissed' WHERE id = ?1 AND owner = ?2 AND status = 'open'", params![pid, user.user_id]).ok()).unwrap_or(0);
    if n == 0 {
        return err(StatusCode::NOT_FOUND, "not_found", "no such open proposal");
    }
    Json(json!({ "dismissed": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::{accounts, route_inbound, telegram_normalize, whatsapp_normalize, Account};
    use crate::channel_gateway::Recorded;

    struct Rt;
    impl ThreadRuntime for Rt {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, id: &str) -> Result<String, String> {
            Ok(format!("sess-{id}"))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, _s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Err("n/a".into())
        }
    }

    async fn setup(provider: &str) -> (Arc<AppState>, Account) {
        let dir = std::env::temp_dir().join(format!("allternit-people-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, restricted_bot_id, state, created_at, updated_at) VALUES (?1,'user-a',?2,'api_key',?3,'bot-1','CONNECTED','2026-01-01','2026-01-01')",
            params![format!("acct-{provider}"), provider, crate::token_crypto::seal("{\"k\":\"v\"}")],
        )
        .unwrap();
        let a = accounts(&st.db, provider, None).remove(0);
        (st, a)
    }

    fn add_account(st: &Arc<AppState>, provider: &str) -> Account {
        st.db.connect().unwrap().execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, restricted_bot_id, state, created_at, updated_at) VALUES (?1,'user-a',?2,'api_key',?3,'bot-1','CONNECTED','2026-01-01','2026-01-01')",
            params![format!("acct-{provider}"), provider, crate::token_crypto::seal("{\"k\":\"v\"}")],
        )
        .unwrap();
        accounts(&st.db, provider, None).remove(0)
    }

    fn msg(conv: &str, channel: &str, user: &str, mid: &str, text: &str) -> Inbound {
        Inbound {
            kind: InboundKind::Message, workspace: None, channel: channel.into(), conversation: conv.into(), thread: None, remote_id: mid.into(), message_id: mid.into(),
            text: Some(text.into()), user: Some(user.into()), reaction: None, added: None, cursor: Some(mid.into()), own: false,
        }
    }

    fn count(st: &Arc<AppState>, sql: &str) -> i64 {
        st.db.connect().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn handles_normalise_so_the_same_number_matches_across_channels() {
        assert_eq!(identity_key("sms", "+44 7700-900123", None), Some(("phone", "+447700900123".into())));
        assert_eq!(identity_key("whatsapp", "447700900123", None), Some(("phone", "+447700900123".into())));
        assert_eq!(identity_key("whatsapp-personal", "447700900123@s.whatsapp.net", None), Some(("phone", "+447700900123".into())));
        assert_eq!(identity_key("email", " Dana@Example.COM ", None), Some(("email", "dana@example.com".into())));
        assert_eq!(identity_key("slack", "U1", Some("T9")), Some(("slack", "T9:U1".into())));
        assert_eq!(identity_key("sms", "12", None), None);
        assert_eq!(identity_key("carrier-pigeon", "x", None), None);
    }

    #[test]
    fn direct_chats_are_told_from_groups() {
        assert!(is_direct_conversation("telegram", "55", "telegram:55", None));
        assert!(!is_direct_conversation("telegram", "-1001", "telegram:-1001", None));
        assert!(is_direct_conversation("slack", "D1", "slack:D1:1.2", Some("T1")));
        assert!(!is_direct_conversation("slack", "C1", "slack:C1:1.2", Some("T1")));
        assert!(!is_direct_conversation("discord", "9", "discord:9", Some("guild")));
        assert!(is_direct_conversation("discord", "9", "discord:9", None));
        assert!(!is_direct_conversation("whatsapp-personal", "x@g.us", "wp:x@g.us", None));
    }

    #[tokio::test]
    async fn the_same_phone_on_sms_and_whatsapp_is_one_person_and_one_thread() {
        let (st, wa) = setup("whatsapp").await;
        let sms = add_account(&st, "sms");
        let payload = json!({ "entry": [{ "id": "waba", "changes": [{ "value": { "metadata": { "phone_number_id": "PN1" },
            "messages": [{ "from": "15551234567", "id": "wamid.1", "timestamp": "1", "type": "text", "text": { "body": "hello" } }] } }] }] });
        let from_wa = whatsapp_normalize(&payload).remove(0);
        let wa_user = from_wa.user.clone().unwrap();
        let first = route_inbound(&st.db, &Rt, &wa, "whatsapp", &from_wa).await.unwrap();
        assert_eq!(first.recorded, Recorded::New);
        let person = first.person.clone().expect("a direct message resolves a person");
        let sms_event = msg(&format!("phone:+15550000:+{wa_user}"), "+15550000", &format!("+{wa_user}"), "s1", "hi again by text");
        let second = route_inbound(&st.db, &Rt, &sms, "sms", &sms_event).await.unwrap();
        assert_eq!(second.person.as_deref(), Some(person.as_str()), "same number, same person");
        assert_eq!(first.binding.unwrap().thread_id, second.binding.unwrap().thread_id, "one thread per person per bot");
        assert_eq!(count(&st, "SELECT COUNT(*) FROM people"), 1);
        assert_eq!(count(&st, "SELECT COUNT(*) FROM person_identities"), 2);
        assert_eq!(count(&st, "SELECT COUNT(*) FROM bot_threads"), 1);
        let src: String = st.db.connect().unwrap().query_row("SELECT source FROM person_identities WHERE provider = 'sms'", [], |r| r.get(0)).unwrap();
        assert_eq!(src, "exact_match");
    }

    #[tokio::test]
    async fn a_telegram_user_and_an_sms_number_stay_apart_until_the_owner_links_them() {
        let (st, tg_acct) = setup("telegram").await;
        let e = telegram_normalize(&json!({ "update_id": 1, "message": { "message_id": 1, "chat": { "id": 55 }, "from": { "id": 55 }, "text": "hi" } })).remove(0);
        let r = route_inbound(&st.db, &Rt, &tg_acct, "telegram", &e).await.unwrap();
        let dana = r.person.unwrap();
        let other = import_contact(&st.db, "user-a", "Dana", Some("Acme"), &[("sms".into(), "+15551234567".into())], "contact").unwrap();
        assert_ne!(dana, other);
        assert_eq!(link_identity(&st.db, "user-a", &dana, "telegram", "55", "manual").unwrap(), LinkOutcome::Already);
        update_person(&st.db, "user-a", &dana, &json!({ "displayName": "Dana", "org": "Acme" })).unwrap();
        // Same name + organisation: proposed, never merged on its own.
        assert_eq!(propose_links(&st.db, "user-a"), 1);
        assert_eq!(count(&st, "SELECT COUNT(*) FROM people WHERE merged_into IS NOT NULL"), 0);
        let (pid, reason): (String, String) = st.db.connect().unwrap().query_row("SELECT id, reason FROM person_link_proposals", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(reason, "same name and organisation");
        // Asking again doesn't duplicate; dismissing never asks again.
        assert_eq!(propose_links(&st.db, "user-a"), 1);
        st.db.connect().unwrap().execute("UPDATE person_link_proposals SET status = 'dismissed' WHERE id = ?1", params![pid]).unwrap();
        assert_eq!(propose_links(&st.db, "user-a"), 0);
    }

    #[test]
    fn a_contact_with_a_known_handle_joins_that_person_and_a_contact_spanning_two_people_is_proposed() {
        let dir = std::env::temp_dir().join(format!("allternit-people-c-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let st = rt.block_on(crate::test_helpers::app_state(&dir));
        let a = import_contact(&st.db, "o", "Sam", None, &[("email".into(), "sam@x.io".into())], "contact").unwrap();
        let again = import_contact(&st.db, "o", "Sam Lee", None, &[("email".into(), "SAM@x.io".into()), ("sms".into(), "+15557654321".into())], "invite").unwrap();
        assert_eq!(a, again, "same email links by exact match");
        assert_eq!(identities_of(&st.db, "o", &a).len(), 2);
        let b = import_contact(&st.db, "o", "Pat", None, &[("sms".into(), "+15550001111".into())], "contact").unwrap();
        import_contact(&st.db, "o", "Sam Lee", None, &[("sms".into(), "+15557654321".into()), ("sms".into(), "+15550001111".into())], "contact").unwrap();
        let proposals: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM person_link_proposals WHERE status = 'open'", [], |r| r.get(0)).unwrap();
        assert_eq!(proposals, 1);
        assert_ne!(a, b);
        // Another owner never sees or matches these.
        assert!(get_person(&st.db, "someone-else", &a).is_none());
        assert_ne!(import_contact(&st.db, "someone-else", "Sam", None, &[("email".into(), "sam@x.io".into())], "contact").unwrap(), a);
    }

    #[tokio::test]
    async fn group_chats_keep_their_own_thread_and_only_name_known_senders() {
        let (st, acct) = setup("telegram").await;
        let dm = telegram_normalize(&json!({ "update_id": 1, "message": { "message_id": 1, "chat": { "id": 55 }, "from": { "id": 55 }, "text": "dm" } })).remove(0);
        let r = route_inbound(&st.db, &Rt, &acct, "telegram", &dm).await.unwrap();
        let dana = r.person.unwrap();
        update_person(&st.db, "user-a", &dana, &json!({ "displayName": "Dana" })).unwrap();
        let group = |from: i64, mid: i64| telegram_normalize(&json!({ "update_id": mid, "message": { "message_id": mid, "chat": { "id": -100 }, "from": { "id": from }, "text": "hello team" } })).remove(0);
        let g1 = route_inbound(&st.db, &Rt, &acct, "telegram", &group(55, 2)).await.unwrap();
        let (_, _, text) = g1.turn.unwrap();
        assert!(text.starts_with("[telegram from Dana]"), "{text}");
        assert_ne!(g1.binding.as_ref().unwrap().thread_id, r.binding.as_ref().unwrap().thread_id, "the group is its own thread");
        let stranger = route_inbound(&st.db, &Rt, &acct, "telegram", &group(777, 3)).await.unwrap();
        assert!(stranger.person.is_none());
        assert!(stranger.turn.unwrap().2.starts_with("[telegram from 777]"));
        assert_eq!(count(&st, "SELECT COUNT(*) FROM people"), 1, "a group member we don't know isn't added to People");
    }

    fn bind(st: &Arc<AppState>, thread: &str, provider: &str, conv: &str, channel: &str, updated: &str) -> BindingRow {
        let c = st.db.connect().unwrap();
        c.execute(
            "INSERT INTO channel_conversation_bindings (id, owner, thread_id, provider, external_channel_id, external_conversation_id, bidirectional, read_only, sync_state, created_at, updated_at)
             VALUES (?1,'user-a',?2,?3,?4,?5,1,0,'LIVE',?6,?6)",
            params![format!("b-{provider}"), thread, provider, channel, conv, updated],
        )
        .unwrap();
        crate::channel_gateway::find_binding(&st.db, provider, conv).unwrap()
    }

    #[tokio::test]
    async fn replies_go_where_the_person_is_unless_pinned_or_the_bot_picks_another_allowed_channel() {
        let (st, acct) = setup("telegram").await;
        let e = telegram_normalize(&json!({ "update_id": 1, "message": { "message_id": 1, "chat": { "id": 55 }, "from": { "id": 55 }, "text": "hi" } })).remove(0);
        let r = route_inbound(&st.db, &Rt, &acct, "telegram", &e).await.unwrap();
        let (person, tg) = (r.person.unwrap(), r.binding.unwrap());
        link_identity(&st.db, "user-a", &person, "sms", "+15551234567", "manual").unwrap();
        let sms = bind(&st, &tg.thread_id, "sms", "phone:+1555:+15551234567", "+1555", "2999-01-01");
        // Arrived on Telegram: reply on Telegram.
        let c = choose_reply(&st.db, "user-a", &person, Some(&tg), None).unwrap();
        assert_eq!((c.binding.provider.as_str(), c.reason), ("telegram", "last_used"));
        // The owner pins SMS: it wins over the arrival channel.
        update_person(&st.db, "user-a", &person, &json!({ "replyPin": "sms" })).unwrap();
        let c = choose_reply(&st.db, "user-a", &person, Some(&tg), None).unwrap();
        assert_eq!((c.binding.provider.as_str(), c.reason, c.binding.id.as_str()), ("sms", "pinned", sms.id.as_str()));
        // The bot asks for Telegram: allowed, the person has written there.
        assert_eq!(choose_reply(&st.db, "user-a", &person, Some(&tg), Some("telegram")).unwrap().reason, "requested");
        // The bot asks for a channel the person never wrote on: ignored, consent unchanged.
        assert_eq!(choose_reply(&st.db, "user-a", &person, Some(&tg), Some("slack")).unwrap().reason, "pinned");
        // Unpin; a bot-started message goes to the channel they used last.
        update_person(&st.db, "user-a", &person, &json!({ "replyPin": null })).unwrap();
        st.db.connect().unwrap().execute("UPDATE person_identities SET last_seen_at = '2026-02-01' WHERE provider = 'telegram'", []).unwrap();
        st.db.connect().unwrap().execute("UPDATE person_identities SET last_seen_at = '2026-03-01' WHERE provider = 'sms'", []).unwrap();
        let c = choose_reply(&st.db, "user-a", &person, None, None).unwrap();
        assert_eq!((c.binding.provider.as_str(), c.reason), ("sms", "last_used"));
        // A pin on a channel the person doesn't have is refused.
        assert!(update_person(&st.db, "user-a", &person, &json!({ "replyPin": "discord" })).is_err());
    }

    #[tokio::test]
    async fn merge_folds_identities_facts_and_threads_and_split_takes_one_back() {
        let (st, acct) = setup("telegram").await;
        let tg = |id: i64, mid: i64| telegram_normalize(&json!({ "update_id": mid, "message": { "message_id": mid, "chat": { "id": id }, "from": { "id": id }, "text": "hi" } })).remove(0);
        let a = route_inbound(&st.db, &Rt, &acct, "telegram", &tg(55, 1)).await.unwrap();
        let b = route_inbound(&st.db, &Rt, &acct, "telegram", &tg(66, 2)).await.unwrap();
        let (pa, pb) = (a.person.unwrap(), b.person.unwrap());
        let (ta, tb) = (a.binding.unwrap(), b.binding.unwrap());
        assert_ne!(ta.thread_id, tb.thread_id);
        remember(&st.db, "user-a", &pb, Some("bot-1"), "prefers mornings", None).unwrap();
        merge(&st.db, "user-a", &pa, &pb).unwrap();
        assert_eq!(identities_of(&st.db, "user-a", &pa).len(), 2);
        assert_eq!(get_person(&st.db, "user-a", &pb).unwrap().id, pa, "the merged id resolves to the survivor");
        assert_eq!(facts(&st.db, "user-a", &pa, 10), vec!["prefers mornings".to_string()]);
        let parent: Option<String> = st.db.connect().unwrap().query_row("SELECT parent_thread_id FROM bot_threads WHERE id = ?1", params![tb.thread_id], |r| r.get(0)).unwrap();
        assert_eq!(parent.as_deref(), Some(ta.thread_id.as_str()), "the other person's thread is now a sub-thread");
        // New message on 66's chat follows the merged person into the one thread.
        let after = route_inbound(&st.db, &Rt, &acct, "telegram", &tg(66, 3)).await.unwrap();
        assert_eq!(after.person.as_deref(), Some(pa.as_str()));
        assert_eq!(after.binding.unwrap().thread_id, ta.thread_id);
        // Merging a person into themselves is refused.
        assert!(merge(&st.db, "user-a", &pa, &pb).is_err());
        // Split 66 back out; its chat follows it on the next message.
        let ident = identities_of(&st.db, "user-a", &pa).into_iter().find(|i| i.external_id == "66").unwrap();
        let new_id = split(&st.db, "user-a", &ident.id).unwrap();
        assert_ne!(new_id, pa);
        assert_eq!(identities_of(&st.db, "user-a", &new_id).len(), 1);
        assert!(split(&st.db, "user-a", &identities_of(&st.db, "user-a", &new_id)[0].id).is_err(), "can't split off a person's last identity");
        let again = route_inbound(&st.db, &Rt, &acct, "telegram", &tg(66, 4)).await.unwrap();
        assert_eq!(again.person.as_deref(), Some(new_id.as_str()));
        assert_ne!(again.binding.unwrap().thread_id, ta.thread_id, "the split person has their own thread again");
        // The split pair isn't proposed straight back.
        assert_eq!(propose_links(&st.db, "user-a"), 0);
    }

    #[tokio::test]
    async fn existing_chat_threads_become_people_and_sub_threads_of_the_person_thread() {
        let (st, acct) = setup("telegram").await;
        // A chat from before people existed: its own thread, a binding, one sender in the log.
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, last_activity_at, created_at, updated_at) VALUES ('old-1','user-a','bot-1','old chat','t','t','t')", []).unwrap();
        c.execute("INSERT INTO bot_thread_sessions (thread_id, session_id, generation, started_at) VALUES ('old-1','old-sess',1,'t')", []).unwrap();
        c.execute(
            "INSERT INTO channel_conversation_bindings (id, owner, thread_id, provider, external_channel_id, external_conversation_id, bidirectional, read_only, sync_state, created_at, updated_at) VALUES ('b-old','user-a','old-1','telegram','55','telegram:55',1,0,'LIVE','t','t')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, state, detail_json, created_at, updated_at) VALUES ('m1','user-a','b-old','old-1','inbound','message','55:1','confirmed','{\"user\":\"55\"}','2026-01-01','2026-01-01')",
            [],
        )
        .unwrap();
        // A group chat with several senders is left alone.
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, last_activity_at, created_at, updated_at) VALUES ('old-g','user-a','bot-1','group','t','t','t')", []).unwrap();
        c.execute("INSERT INTO bot_thread_sessions (thread_id, session_id, generation, started_at) VALUES ('old-g','old-gs',1,'t')", []).unwrap();
        c.execute("INSERT INTO channel_conversation_bindings (id, owner, thread_id, provider, external_channel_id, external_conversation_id, bidirectional, read_only, sync_state, created_at, updated_at) VALUES ('b-g','user-a','old-g','telegram','-100','telegram:-100',1,0,'LIVE','t','t')", []).unwrap();
        drop(c);
        assert_eq!(backfill(&st.db, Some("user-a")), 1);
        assert_eq!(backfill(&st.db, Some("user-a")), 0, "running it twice adopts nothing new");
        assert_eq!(count(&st, "SELECT COUNT(*) FROM people"), 1);
        let person: String = st.db.connect().unwrap().query_row("SELECT person_id FROM person_threads WHERE thread_id = 'old-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(identities_of(&st.db, "user-a", &person)[0].external_id, "55");
        // The person's next message on that same chat keeps the old thread; a message on a new channel makes the person thread and adopts the old one.
        let e = telegram_normalize(&json!({ "update_id": 9, "message": { "message_id": 9, "chat": { "id": 55 }, "from": { "id": 55 }, "text": "again" } })).remove(0);
        let r = route_inbound(&st.db, &Rt, &acct, "telegram", &e).await.unwrap();
        assert_eq!(r.binding.as_ref().unwrap().thread_id, "old-1");
        assert_eq!(r.person.as_deref(), Some(person.as_str()));
        let sms = add_account(&st, "sms");
        let s = msg("phone:+1555:+15551112222", "+1555", "+15551112222", "s1", "text me");
        link_identity(&st.db, "user-a", &person, "sms", "+15551112222", "manual").unwrap();
        let r2 = route_inbound(&st.db, &Rt, &sms, "sms", &s).await.unwrap();
        let main = r2.binding.unwrap().thread_id;
        assert_ne!(main, "old-1");
        let parent: Option<String> = st.db.connect().unwrap().query_row("SELECT parent_thread_id FROM bot_threads WHERE id = 'old-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(parent.as_deref(), Some(main.as_str()), "the old chat thread hangs under the person thread");
        let group_parent: Option<String> = st.db.connect().unwrap().query_row("SELECT parent_thread_id FROM bot_threads WHERE id = 'old-g'", [], |r| r.get(0)).unwrap();
        assert!(group_parent.is_none());
    }

    #[tokio::test]
    async fn the_bots_memory_of_a_person_is_shared_by_every_channel_turn() {
        let (st, acct) = setup("telegram").await;
        let e = telegram_normalize(&json!({ "update_id": 1, "message": { "message_id": 1, "chat": { "id": 55 }, "from": { "id": 55 }, "text": "hi" } })).remove(0);
        let person = route_inbound(&st.db, &Rt, &acct, "telegram", &e).await.unwrap().person.unwrap();
        remember(&st.db, "user-a", &person, Some("bot-1"), "allergic to nuts", None).unwrap();
        remember(&st.db, "user-a", &person, Some("bot-1"), "allergic to nuts", None).unwrap();
        let e2 = telegram_normalize(&json!({ "update_id": 2, "message": { "message_id": 2, "chat": { "id": 55 }, "from": { "id": 55 }, "text": "what can I eat" } })).remove(0);
        let r = route_inbound(&st.db, &Rt, &acct, "telegram", &e2).await.unwrap();
        let text = r.turn.unwrap().2;
        assert!(text.contains("allergic to nuts") && text.matches("allergic").count() == 1, "{text}");
        assert!(remember(&st.db, "other-owner", &person, None, "x", None).is_err());
    }

    #[test]
    fn bots_add_people_remember_facts_and_look_them_up() {
        let dir = std::env::temp_dir().join(format!("allternit-people-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let st = rt.block_on(crate::test_helpers::app_state(&dir));
        let call = |name: &str, args: Value| call_mcp_tool(&st.db, "o", name, &args);

        assert!(call("people_add_contact", json!({ "name": "Dana" })).unwrap_err().contains("email address or a phone"));
        let dana = call("people_add_contact", json!({ "name": "Dana Lee", "org": "Acme", "email": "dana@acme.io", "phone": "+1 (555) 123-4567" })).unwrap();
        let id = dana["id"].as_str().unwrap().to_string();
        // The same email again updates Dana instead of adding a second person.
        assert_eq!(call("people_add_contact", json!({ "name": "D. Lee", "email": "DANA@acme.io" })).unwrap()["id"], id.as_str());

        assert_eq!(call("people_remember", json!({ "person": "dana lee", "fact": "Prefers email over calls", "agent_id": "bot-1" })).unwrap()["person"]["id"], id.as_str());
        assert!(call("people_remember", json!({ "person": id, "fact": "Her card number is 4111" })).unwrap_err().contains("isn't saved"));
        assert!(call("people_remember", json!({ "person": "Nobody", "fact": "x" })).unwrap_err().contains("No one called"));

        // Lookup by name, org, email or phone digits; facts come back from every channel.
        for q in ["dana", "acme", "dana@acme", "555-123-4567"] {
            let found = call("people_lookup", json!({ "query": q })).unwrap();
            assert_eq!(found["people"][0]["id"], id.as_str(), "{q}");
            assert_eq!(found["people"][0]["known"], json!(["Prefers email over calls"]), "{q}");
        }
        // Another owner sees none of it.
        assert_eq!(call_mcp_tool(&st.db, "someone-else", "people_lookup", &json!({ "query": "dana" })).unwrap()["people"], json!([]));

        // Two people with one name: the bot is asked for the id.
        call("people_add_contact", json!({ "name": "Dana Lee", "email": "other@x.io" })).unwrap();
        assert!(call("people_remember", json!({ "person": "Dana Lee", "fact": "y" })).unwrap_err().contains("More than one"));
        assert!(mcp_tools().iter().all(|t| is_tool(t["name"].as_str().unwrap())));

        // The owner sees each note with who wrote it, and can remove it; nobody else can.
        let items = fact_items(&st.db, "o", &id, 50);
        assert_eq!((items.len(), items[0]["botId"].as_str()), (1, Some("bot-1")));
        let fid = items[0]["id"].as_str().unwrap();
        assert!(!forget(&st.db, "someone-else", fid));
        assert!(forget(&st.db, "o", fid));
        assert!(fact_items(&st.db, "o", &id, 50).is_empty());
    }
}
