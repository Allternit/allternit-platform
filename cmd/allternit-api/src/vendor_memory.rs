//! Keep Memory Drive current from the person's own assistant logins
//! (ChatGPT, Claude, Gemini, Kimi, Copilot on their Sessions computer).
//!
//! For each signed-in login, through the Subscriptions gateway's read-only
//! routes (`GET v1/accounts/:id/memory`, `/threads`, `/threads/:id`):
//!
//! 1. **Saved memories and instructions**, word for word, into
//!    `imports/<assistant>.md` with `<scheme>:memory` as the source.
//! 2. **Past chats** changed since the last read (up to [`CHATS_PER_RUN`]):
//!    the person's own messages go through the same model as a chat-export
//!    import, and what it finds lands in `imports/<assistant>-chats.md`.
//!
//! - **Read-only.** The gateway never types, clicks or sends for these, and
//!   refuses a login that is mid-task (`busy`) or not signed in.
//! - **The person's say.** Runs only with the `vendors` capture switch on
//!   (on by default) and only for providers whose Subscriptions disclosure
//!   the person acknowledged. A forget sticks: lines the person removed are
//!   never brought back (`check_auto`).
//! - **Visible.** Every login read is a `vendor` row in Memory Drive →
//!   Activity, and the commits themselves log as `vendor` too.
//! - **When.** Ten minutes after start, then every six hours, plus
//!   `POST /memory/drive/vendors/run` ("Read now").
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::memory_drive_chat_import as chats;
use crate::AppState;

/// Gateway provider id → Memory Drive assistant id.
pub const PROVIDERS: &[(&str, &str)] = &[("chatgpt", "chatgpt"), ("claude", "claude"), ("gemini", "gemini"), ("kimi", "kimi"), ("copilot", "copilot")];
/// Most past chats read per login per run.
pub const CHATS_PER_RUN: usize = chats::MAX_CONVERSATIONS;
/// How far back in the history list a run looks.
const LIST_LIMIT: usize = 50;
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST_AFTER: Duration = Duration::from_secs(10 * 60);
const LAST_KEY: &str = "vendor_memory_last";

fn cursor_key(account: &str) -> String {
    format!("vendor_memory_cursor:{account}")
}

/// What one login read did.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoginRead {
    pub account_id: String,
    pub provider: String,
    pub name: String,
    /// `read`, `busy`, `needs_you`, `not_supported`, `no_model` or `failed`.
    pub status: String,
    pub memories_added: usize,
    pub chats_read: usize,
    pub chat_memories_added: usize,
    pub detail: Option<String>,
    pub at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursor {
    /// Thread id → the updated_at last read.
    seen: BTreeMap<String, String>,
}

fn user_for(owner: &str) -> AuthUser {
    AuthUser {
        user_id: owner.to_string(),
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    }
}

/// One gateway GET: (status, body).
async fn gateway(state: &Arc<AppState>, owner: &str, path: &str, query: Option<&str>) -> Result<(u16, Value), String> {
    let resp = crate::subscription_routes::forward(state, &user_for(owner), path, Method::GET, &HeaderMap::new(), query, Bytes::new()).await;
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024).await.map_err(|e| e.to_string())?;
    Ok((status, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// A gateway refusal as a status word.
fn refusal(status: u16, body: &Value) -> (&'static str, Option<String>) {
    match (status, body["error"].as_str()) {
        (409, Some("busy")) => ("busy", Some("It was busy with a chat; trying again next time.".into())),
        (409, Some("not_ready")) | (409, Some("account_disabled")) => (
            "needs_you",
            Some(match body["session_health"].as_str() {
                Some("challenge_presented") => "It's showing a verification check. Clear it on your Sessions computer.".into(),
                Some("auth_required") | Some("logged_out") => "It's signed out. Sign in again in Settings → Subscriptions.".into(),
                _ => "It isn't ready. Open Settings → Subscriptions.".into(),
            }),
        ),
        (501, _) | (404, Some("not_found")) => ("not_supported", None),
        _ => ("failed", Some(format!("The Sessions computer answered {status}."))),
    }
}

fn get_setting(conn: &rusqlite::Connection, owner: &str, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM runtime_settings WHERE user_id = ?1 AND key = ?2", params![owner, key], |r| r.get(0)).optional().ok().flatten()
}

fn put_setting(conn: &rusqlite::Connection, owner: &str, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO runtime_settings (user_id, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value",
        params![owner, key, value],
    );
}

/// The last run's results, for Memory Drive → Sources.
pub fn last_reads(conn: &rusqlite::Connection, owner: &str) -> Vec<LoginRead> {
    get_setting(conn, owner, LAST_KEY).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default()
}

/// Saved memories and instructions as lines to import.
pub fn memory_lines(body: &Value) -> Vec<String> {
    let mut out: Vec<String> = body["memories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["text"].as_str())
        .map(str::to_string)
        .collect();
    for i in body["instructions"].as_array().into_iter().flatten() {
        if let (Some(label), Some(text)) = (i["label"].as_str(), i["text"].as_str()) {
            out.push(format!("{label}: {text}"));
        }
    }
    out.retain(|l| !l.trim().is_empty());
    out
}

/// Threads changed since the cursor, newest first, at most `max`.
pub fn changed_threads(list: &Value, cursor: &BTreeMap<String, String>, max: usize) -> Vec<(String, String, String)> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let id = t["id"].as_str()?.to_string();
            let updated = t["updated_at"].as_str().unwrap_or("").to_string();
            let title = t["title"].as_str().unwrap_or("Untitled").to_string();
            (cursor.get(&id) != Some(&updated)).then_some((id, title, updated))
        })
        .take(max)
        .collect()
}

/// A past chat as the distiller's input: the person's own messages only.
pub fn conversation(id: &str, title: &str, updated: &str, body: &Value) -> Option<chats::Conversation> {
    let mine: Vec<&str> = body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["text"].as_str())
        .collect();
    if mine.is_empty() {
        return None;
    }
    let mut text = mine.join("\n\n");
    if text.chars().count() > chats::MAX_CONVERSATION_CHARS {
        // Keep the latest part: it says most about the person today.
        let skip = text.chars().count() - chats::MAX_CONVERSATION_CHARS;
        text = text.chars().skip(skip).collect();
    }
    Some(chats::Conversation { id: id.to_string(), title: Some(title.to_string()), date: updated.get(..10).map(str::to_string), text })
}

/// Read one login. `complete` runs the distiller's model call.
async fn read_login<F, Fut>(state: &Arc<AppState>, owner: &str, account: &Value, complete: &F) -> LoginRead
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    let account_id = account["account_id"].as_str().unwrap_or_default().to_string();
    let provider = account["provider"].as_str().unwrap_or_default().to_string();
    let from = PROVIDERS.iter().find(|(p, _)| *p == provider).map(|(_, f)| *f).unwrap_or("other");
    let name = crate::memory_drive_text_import::display_name(from).unwrap_or("assistant").to_string();
    let mut out = LoginRead {
        account_id: account_id.clone(),
        provider: provider.clone(),
        name: name.clone(),
        status: "read".into(),
        memories_added: 0,
        chats_read: 0,
        chat_memories_added: 0,
        detail: None,
        at: chrono::Utc::now().to_rfc3339(),
    };
    let fail = |mut o: LoginRead, s: &str, d: Option<String>| {
        o.status = s.into();
        o.detail = d;
        o
    };

    // 1. Saved memories and instructions.
    let mut memory_read = false;
    match gateway(state, owner, &format!("v1/accounts/{account_id}/memory"), None).await {
        Ok((200, body)) => {
            memory_read = true;
            let lines = memory_lines(&body);
            if !lines.is_empty() {
                let (db, o, f) = (state.db.clone(), owner.to_string(), from.to_string());
                match tokio::task::spawn_blocking(move || crate::memory_drive_text_import::sync_from_login(&db, &o, &f, &lines)).await {
                    Ok(Ok((_, n))) => out.memories_added = n,
                    Ok(Err(e)) => tracing::warn!(provider = %provider, "vendor memory import failed: {e}"),
                    Err(e) => tracing::warn!("vendor memory import task failed: {e}"),
                }
            }
        }
        Ok((status, body)) => {
            let (s, d) = refusal(status, &body);
            if s != "not_supported" {
                return fail(out, s, d);
            }
        }
        Err(e) => return fail(out, "failed", Some(e)),
    }

    // 2. Past chats changed since the last read.
    let mut cursor: Cursor = db_get(state, owner, &cursor_key(&account_id)).await.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    let list = match gateway(state, owner, &format!("v1/accounts/{account_id}/threads"), Some(&format!("limit={LIST_LIMIT}"))).await {
        Ok((200, body)) => body,
        Ok((status, body)) => {
            let (s, d) = refusal(status, &body);
            return match (s, memory_read) {
                ("not_supported", true) => out,
                ("not_supported", false) => fail(out, "not_supported", Some(format!("Allternit can't read {name} yet."))),
                _ => fail(out, s, d),
            };
        }
        Err(e) => return fail(out, "failed", Some(e)),
    };
    let changed = changed_threads(&list, &cursor.seen, CHATS_PER_RUN);
    let mut convs = Vec::new();
    let mut read = Vec::new();
    let mut chars = 0;
    for (id, title, updated) in changed {
        let Ok((200, body)) = gateway(state, owner, &format!("v1/accounts/{account_id}/threads/{id}"), None).await else { continue };
        read.push((id.clone(), updated.clone()));
        if let Some(c) = conversation(&id, &title, &updated, &body) {
            if chars + c.text.len() > chats::MAX_BATCH_CHARS {
                read.pop();
                break;
            }
            chars += c.text.len();
            convs.push(c);
        }
    }
    out.chats_read = read.len();
    if !convs.is_empty() {
        let found = complete(chats::build_prompt(&convs)).await.and_then(|raw| chats::parse_reply(&raw, &convs));
        let Some(found) = found else {
            // Not marked as read: the next run tries these chats again.
            return fail(out, "no_model", Some("The memory model didn't answer; your chats will be read next time.".into()));
        };
        let (db, o, f) = (state.db.clone(), owner.to_string(), from.to_string());
        match tokio::task::spawn_blocking(move || chats::sync_from_login(&db, &o, &f, found)).await {
            Ok(Ok((_, n))) => out.chat_memories_added = n,
            Ok(Err(e)) => tracing::warn!(provider = %provider, "vendor chat import failed: {e}"),
            Err(e) => tracing::warn!("vendor chat import task failed: {e}"),
        }
    }
    for (id, updated) in read {
        cursor.seen.insert(id, updated);
    }
    // Bounded: only threads still in the recent list are remembered.
    let recent: std::collections::BTreeSet<&str> = list.as_array().into_iter().flatten().filter_map(|t| t["id"].as_str()).collect();
    cursor.seen.retain(|id, _| recent.contains(id.as_str()));
    db_put(state, owner, &cursor_key(&account_id), serde_json::to_string(&cursor).unwrap_or_default()).await;
    out
}

async fn db_get(state: &Arc<AppState>, owner: &str, key: &str) -> Option<String> {
    let (db, o, k) = (state.db.clone(), owner.to_string(), key.to_string());
    tokio::task::spawn_blocking(move || db.connect().ok().and_then(|c| get_setting(&c, &o, &k))).await.ok().flatten()
}

async fn db_put(state: &Arc<AppState>, owner: &str, key: &str, value: String) {
    let (db, o, k) = (state.db.clone(), owner.to_string(), key.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(c) = db.connect() {
            put_setting(&c, &o, &k, &value);
        }
    })
    .await;
}

fn summary(r: &LoginRead) -> String {
    match r.status.as_str() {
        "read" => {
            let mut parts = vec![];
            parts.push(match r.memories_added {
                0 => "no new saved memories".to_string(),
                1 => "1 new saved memory".to_string(),
                n => format!("{n} new saved memories"),
            });
            if r.chats_read > 0 {
                parts.push(format!(
                    "{} {} read, {} new {}",
                    r.chats_read,
                    if r.chats_read == 1 { "chat" } else { "chats" },
                    r.chat_memories_added,
                    if r.chat_memories_added == 1 { "memory" } else { "memories" }
                ));
            }
            format!("Read your {} login: {}", r.name, parts.join("; "))
        }
        _ => format!("Couldn't read your {} login: {}", r.name, r.detail.clone().unwrap_or_else(|| r.status.clone())),
    }
}

/// Read every eligible login of one owner. Empty when the switch is off or
/// no Sessions computer answers.
pub async fn run_owner(state: &Arc<AppState>, owner: &str) -> Vec<LoginRead> {
    let model = crate::memory_extraction::extraction_model();
    let o = owner.to_string();
    let complete = move |prompt: String| {
        let (model, o) = (model.clone(), o.clone());
        async move {
            crate::usage_ledger::scope(
                crate::usage_ledger::LedgerCtx::surface("memory").tenant(None, Some(&o)),
                crate::gizzi_completion::complete_ephemeral_structured(&prompt, Some(chats::SYSTEM_PROMPT), Some(&model), &chats::reply_schema()),
            )
            .await
        }
    };
    run_owner_with(state, owner, &complete).await
}

pub async fn run_owner_with<F, Fut>(state: &Arc<AppState>, owner: &str, complete: &F) -> Vec<LoginRead>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    let on = {
        let (db, o) = (state.db.clone(), owner.to_string());
        tokio::task::spawn_blocking(move || db.connect().map(|c| crate::memory_capture::settings(&c, &o).vendors).unwrap_or(false)).await.unwrap_or(false)
    };
    if !on {
        return vec![];
    }
    let accounts = match gateway(state, owner, "v1/accounts", None).await {
        Ok((200, Value::Array(a))) => a,
        _ => return vec![],
    };
    let mut results = vec![];
    for account in accounts.iter().filter(|a| a["enabled"].as_bool() != Some(false)) {
        let provider = account["provider"].as_str().unwrap_or_default();
        if !PROVIDERS.iter().any(|(p, _)| *p == provider) {
            continue;
        }
        let acked = crate::subscription_routes::acknowledged_version(&state.db, owner, provider).ok().flatten();
        if acked != Some(crate::subscription_routes::DISCLOSURE_VERSION) {
            continue;
        }
        let r = read_login(state, owner, account, complete).await;
        if r.status == "not_supported" {
            // Shown under the switch, not logged every six hours.
            results.push(r);
            continue;
        }
        crate::memory_activity::record_db(
            &state.db,
            owner,
            crate::memory_activity::Activity {
                kind: "vendor",
                title: summary(&r),
                extra: Some(json!({ "provider": r.provider, "status": r.status, "memories_added": r.memories_added, "chats_read": r.chats_read, "chat_memories_added": r.chat_memories_added })),
                source: Some(format!("{}:login", crate::memory_drive_text_import::scheme(PROVIDERS.iter().find(|(p, _)| *p == provider).map(|(_, f)| *f).unwrap_or("other")).unwrap_or("import"))),
                ..Default::default()
            },
        );
        results.push(r);
    }
    db_put(state, owner, LAST_KEY, serde_json::to_string(&results).unwrap_or_default()).await;
    results
}

/// Ten minutes after start, then every six hours: every owner with a
/// Sessions computer.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + FIRST_AFTER, EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let db = state.db.clone();
            let local = crate::subscription_routes::local_gateway().and_then(|_| crate::relay_auth::process_secret().paired_owner());
            let owners = tokio::task::spawn_blocking(move || db.connect().and_then(|c| crate::subscription_sync::sync_owners(&c, local)))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            for owner in owners {
                let results = run_owner(&state, &owner).await;
                let added: usize = results.iter().map(|r| r.memories_added + r.chat_memories_added).sum();
                if added > 0 {
                    tracing::info!(added, "memories read from assistant logins");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_lines_keep_memories_and_label_instructions() {
        let body = json!({ "available": true, "memories": [{ "text": "Lives in Saint Paul." }, { "text": " " }], "instructions": [{ "label": "About you", "text": "Runs Allternit." }] });
        assert_eq!(memory_lines(&body), vec!["Lives in Saint Paul.".to_string(), "About you: Runs Allternit.".to_string()]);
        assert!(memory_lines(&Value::Null).is_empty());
    }

    #[test]
    fn only_changed_threads_are_read_newest_first() {
        let list = json!([{ "id": "a", "title": "A", "updated_at": "2026-10-17T10:00:00Z" }, { "id": "b", "title": "B", "updated_at": "2026-10-16T10:00:00Z" }, { "id": "c", "title": "C", "updated_at": "2026-10-15T10:00:00Z" }]);
        let mut seen = BTreeMap::new();
        seen.insert("b".to_string(), "2026-10-16T10:00:00Z".to_string());
        seen.insert("a".to_string(), "2026-10-01T00:00:00Z".to_string());
        let ids: Vec<String> = changed_threads(&list, &seen, 20).into_iter().map(|t| t.0).collect();
        assert_eq!(ids, vec!["a", "c"]);
        assert_eq!(changed_threads(&list, &BTreeMap::new(), 1).len(), 1);
    }

    #[test]
    fn a_chat_becomes_the_persons_own_messages_clipped_to_the_latest() {
        let body = json!({ "messages": [{ "role": "user", "text": "I moved to Saint Paul" }, { "role": "assistant", "text": "Welcome!" }, { "role": "user", "text": "Plan my week" }] });
        let c = conversation("t1", "Move", "2026-10-17T10:00:00Z", &body).unwrap();
        assert_eq!(c.text, "I moved to Saint Paul\n\nPlan my week");
        assert_eq!(c.date.as_deref(), Some("2026-10-17"));
        assert!(conversation("t2", "x", "", &json!({ "messages": [{ "role": "assistant", "text": "hi" }] })).is_none());
        let long = json!({ "messages": [{ "role": "user", "text": format!("{}END", "x".repeat(chats::MAX_CONVERSATION_CHARS)) }] });
        let c = conversation("t3", "x", "", &long).unwrap();
        assert!(c.text.ends_with("END") && c.text.chars().count() == chats::MAX_CONVERSATION_CHARS);
    }

    #[test]
    fn refusals_say_what_to_do() {
        assert_eq!(refusal(409, &json!({ "error": "busy" })).0, "busy");
        let (s, d) = refusal(409, &json!({ "error": "not_ready", "session_health": "challenge_presented" }));
        assert_eq!(s, "needs_you");
        assert!(d.unwrap().contains("verification check"));
        assert_eq!(refusal(501, &json!({ "error": "not_supported" })).0, "not_supported");
        assert_eq!(refusal(502, &Value::Null).0, "failed");
    }

    /// One run against a fake Sessions computer: saved memories and new chats
    /// land in the drive with their sources, unchanged chats aren't read
    /// twice, a forgotten line never comes back, and Activity says so.
    #[tokio::test]
    async fn one_run_reads_memories_and_new_chats_from_a_login() {
        use axum::{extract::Path, routing::get, Json as J, Router};
        const OWNER: &str = "user-1";
        let app = Router::new()
            .route("/v1/accounts", get(|| async { J(json!([{ "account_id": "acc-1", "provider": "chatgpt", "enabled": true }, { "account_id": "acc-2", "provider": "claude", "enabled": true }])) }))
            .route("/v1/accounts/:id/memory", get(|Path(id): Path<String>| async move {
                J(json!({ "available": true, "memories": [{ "text": format!("Lives in Saint Paul ({id}).") }], "instructions": [{ "label": "About you", "text": "Runs Allternit." }] }))
            }))
            .route("/v1/accounts/:id/threads", get(|| async { J(json!([{ "id": "t1", "title": "Move", "updated_at": "2026-10-17T10:00:00Z" }])) }))
            .route("/v1/accounts/:id/threads/:tid", get(|| async { J(json!({ "id": "t1", "messages": [{ "role": "user", "text": "I just adopted a dog named Rex" }, { "role": "assistant", "text": "Congrats!" }] })) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let temp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn allternit_driver_interface::ExecutionDriver> = Arc::new(crate::computer_ws::tests::StubDriver(Some(url)));
        let state = crate::test_helpers::app_state_with_driver(temp.path(), Some(driver)).await;
        crate::memory_drive_writer::configure_root(&state.db, &temp.path().join("brains")).unwrap();
        {
            let c = state.db.connect().unwrap();
            c.execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', ?1, 'sessions', 'ubuntu-24.04', 'sandbox-1', 'credits')",
                params![OWNER],
            )
            .unwrap();
            c.execute(
                "INSERT INTO subs_gateway_bindings (user_id, computer_id, guest_port, token_sealed) VALUES (?1, 'computer-1', 7788, ?2)",
                params![OWNER, crate::token_crypto::seal("gw-token")],
            )
            .unwrap();
            // Only ChatGPT's disclosure is acknowledged: Claude is skipped.
            c.execute("INSERT INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, 'chatgpt', ?2)", params![OWNER, crate::subscription_routes::DISCLOSURE_VERSION]).unwrap();
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model = {
            let calls = calls.clone();
            move |prompt: String| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert!(prompt.contains("adopted a dog") && !prompt.contains("Congrats"), "only the person's own messages go to the model");
                    Some(r#"{"memories":[{"fact":"User has a dog named Rex.","conversation":1}]}"#.to_string())
                }
            }
        };

        let reads = run_owner_with(&state, OWNER, &model).await;
        assert_eq!(reads.len(), 1, "claude's disclosure isn't acknowledged");
        let r = &reads[0];
        assert_eq!((r.status.as_str(), r.memories_added, r.chats_read, r.chat_memories_added), ("read", 2, 1, 1), "{r:?}");
        let files = crate::memory_drive_service::resolve(&state.db, OWNER).unwrap().storage().unwrap().snapshot(None).unwrap().files;
        assert!(files["imports/chatgpt.md"].contains("Lives in Saint Paul (acc-1). [source: chatgpt:memory;"));
        assert!(files["imports/chatgpt.md"].contains("About you: Runs Allternit."));
        assert!(files["imports/chatgpt-chats.md"].contains("User has a dog named Rex. [source: chatgpt:conversation/t1;"));
        let conn = state.db.connect().unwrap();
        let vendor = crate::memory_activity::list(&conn, OWNER, &crate::memory_activity::Filter { kind: Some("vendor".into()), ..Default::default() }).unwrap();
        assert!(vendor.iter().any(|a| a.title == "Read your ChatGPT login: 2 new saved memories; 1 chat read, 1 new memory"), "{:?}", vendor.iter().map(|a| &a.title).collect::<Vec<_>>());
        assert_eq!(last_reads(&conn, OWNER), reads);

        // Again: the chat is unchanged, so neither it nor the model is touched;
        // after a forget, the saved memories stay out.
        crate::memory_capture::forget_source(&state.db, OWNER, "chatgpt").unwrap();
        let again = run_owner_with(&state, OWNER, &model).await;
        assert_eq!((again[0].memories_added, again[0].chats_read), (0, 0));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Switch off: nothing runs.
        let mut s = crate::memory_capture::settings(&conn, OWNER);
        s.vendors = false;
        crate::memory_capture::save(&conn, OWNER, &s).unwrap();
        assert!(run_owner_with(&state, OWNER, &model).await.is_empty());
        std::mem::forget(temp);
    }

    #[test]
    fn summaries_read_plainly() {
        let mut r = LoginRead { account_id: "a".into(), provider: "chatgpt".into(), name: "ChatGPT".into(), status: "read".into(), memories_added: 3, chats_read: 2, chat_memories_added: 1, detail: None, at: String::new() };
        assert_eq!(summary(&r), "Read your ChatGPT login: 3 new saved memories; 2 chats read, 1 new memory");
        r.status = "needs_you".into();
        r.detail = Some("It's signed out.".into());
        assert_eq!(summary(&r), "Couldn't read your ChatGPT login: It's signed out.");
    }
}
