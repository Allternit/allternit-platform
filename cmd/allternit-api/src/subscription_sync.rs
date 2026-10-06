//! Keeps Agent Gateway accounts that sign in through Settings → Subscriptions
//! (`externalAccountId = subsfab:<login id>`) in step with that login's
//! health on the Sessions computer (Eoj 2026-09-30: one login system).
//!
//! A login that needs signing in again moves its account to EXPIRED, so the
//! existing cascade puts dependent bots in NEEDS_AUTH; a verification check
//! is BLOCKED, drift or an outage DEGRADED, a removed login REVOKED, and a
//! healthy login brings the account back to CONNECTED. Only accounts that
//! were already connected are moved: one still in the wizard, or revoked,
//! is left to the person.
//!
//! The same pull is the one path Subscriptions events take into the event
//! backbone (no push from the gateway): a login whose health changes to
//! needing a sign-in (or a bot check) records `subscription.login_needed`, and
//! one that comes back to ready from there records `subscription.signed_in`;
//! a gateway task stopped at `needs_user` records `subscription.task.needs_user`.
//! All three go in the owner ledger (`runtime_user_events`, V236) and reach the
//! cloud through `runtime_events`' forwarder, like every other runtime event.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use tracing::warn;

use crate::agent_gateway_routes::{apply_account_state, connection_path, subscription_linked_accounts, subscription_linked_owners};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

const SYNC_EVERY: Duration = Duration::from_secs(60);
/// Login health that means the person has to sign in (or clear a bot check).
const NEEDS_SIGN_IN: &[&str] = &["auth_required", "challenge_presented"];
/// A `needs_user` task older than this when first seen is not announced.
const TASK_FRESH_HOURS: i64 = 24;
/// States the sync may move an account out of (it was connected once).
const SYNCED_STATES: &[&str] = &["CONNECTED", "DEGRADED", "EXPIRED", "BLOCKED"];

/// A Subscriptions login's health → the account state it means.
pub(crate) fn health_target(health: Option<&str>) -> &'static str {
    match health {
        None => "REVOKED",
        Some("ready") => "CONNECTED",
        Some("auth_required") => "EXPIRED",
        Some("challenge_presented") | Some("account_restricted") => "BLOCKED",
        Some(_) => "DEGRADED",
    }
}

/// One move made: (account id, from, to).
pub(crate) type Moved = (String, String, String);

/// Applies a snapshot of the owner's Subscriptions logins (login id → health)
/// to their linked accounts. Pure over the DB, so tests drive it directly.
pub(crate) fn apply_snapshot(db: &DbHandle, owner: &str, logins: &HashMap<String, String>) -> rusqlite::Result<Vec<Moved>> {
    let conn = db.connect()?;
    let mut moved = vec![];
    for (aid, state, login) in subscription_linked_accounts(&conn, owner)? {
        if !SYNCED_STATES.contains(&state.as_str()) {
            continue;
        }
        let health = logins.get(&login).map(String::as_str);
        let target = health_target(health);
        if target == state {
            continue;
        }
        let Some(path) = connection_path(&state, target) else { continue };
        let mut from = state.clone();
        for to in path {
            let detail = json!({ "reason": "subscription sync", "subscriptionLogin": login, "health": health });
            apply_account_state(db, &conn, owner, &aid, &from, to, false, detail)?;
            from = to.to_string();
        }
        moved.push((aid, state, target.to_string()));
    }
    Ok(moved)
}

/// One Subscriptions login as the gateway lists it.
#[derive(Debug, Clone)]
pub(crate) struct LoginHealth {
    pub id: String,
    pub provider: Option<String>,
    pub health: String,
    pub label: Option<String>,
}

/// Records health changes of the owner's logins and turns the ones the person
/// must act on into ledger events. Race-safe: a change is claimed by a
/// conditional write, so two concurrent syncs record it once. Returns the
/// event types recorded.
pub(crate) fn record_login_health(db: &DbHandle, owner: &str, logins: &[LoginHealth]) -> rusqlite::Result<Vec<(String, &'static str)>> {
    let conn = db.connect()?;
    let known: i64 = conn.query_row("SELECT COUNT(*) FROM subscription_login_health WHERE owner = ?1", params![owner], |r| r.get(0))?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = vec![];
    for l in logins {
        let prev: Option<String> = conn
            .query_row("SELECT health FROM subscription_login_health WHERE owner = ?1 AND login_id = ?2", params![owner, l.id], |r| r.get(0))
            .optional()?;
        let claimed = match &prev {
            None => conn.execute(
                "INSERT OR IGNORE INTO subscription_login_health (owner, login_id, provider, health, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![owner, l.id, l.provider, l.health, now],
            )? == 1,
            Some(p) if *p == l.health => false,
            Some(p) => conn.execute(
                "UPDATE subscription_login_health SET health = ?3, provider = ?4, updated_at = ?5 WHERE owner = ?1 AND login_id = ?2 AND health = ?6",
                params![owner, l.id, l.health, l.provider, now, p],
            )? == 1,
        };
        if !claimed {
            continue;
        }
        let needs = |h: &str| NEEDS_SIGN_IN.contains(&h);
        let event = match prev.as_deref() {
            _ if needs(&l.health) && !prev.as_deref().is_some_and(needs) => Some("subscription.login_needed"),
            Some(p) if l.health == "ready" && needs(p) => Some("subscription.signed_in"),
            // A login added after the first sync that is already ready was just signed in.
            None if l.health == "ready" && known > 0 => Some("subscription.signed_in"),
            _ => None,
        };
        if let Some(ty) = event {
            let payload = json!({ "loginId": l.id, "provider": l.provider, "health": l.health, "previous": prev, "label": l.label });
            crate::runtime_events::record_user_event(&conn, owner, ty, None, &payload, None)?;
            out.push((l.id.clone(), ty));
        }
    }
    // Removed logins are forgotten; adding one back starts fresh.
    let live: Vec<&str> = logins.iter().map(|l| l.id.as_str()).collect();
    let mut stale = vec![];
    {
        let mut st = conn.prepare("SELECT login_id FROM subscription_login_health WHERE owner = ?1")?;
        for id in st.query_map(params![owner], |r| r.get::<_, String>(0))? {
            let id = id?;
            if !live.contains(&id.as_str()) {
                stale.push(id);
            }
        }
    }
    for id in stale {
        conn.execute("DELETE FROM subscription_login_health WHERE owner = ?1 AND login_id = ?2", params![owner, id])?;
    }
    Ok(out)
}

/// Records each recent gateway task stopped at `needs_user` once (keyed by task id).
/// Returns the task ids newly recorded.
pub(crate) fn record_needs_user_tasks(db: &DbHandle, owner: &str, tasks: &[Value]) -> rusqlite::Result<Vec<String>> {
    let conn = db.connect()?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(TASK_FRESH_HOURS);
    let mut out = vec![];
    for t in tasks {
        let (Some(id), Some("needs_user")) = (t["task_id"].as_str(), t["status"].as_str()) else { continue };
        let fresh = t["updated_at"].as_str().and_then(|u| chrono::DateTime::parse_from_rfc3339(u).ok()).map(|u| u.with_timezone(&chrono::Utc) >= cutoff).unwrap_or(true);
        if !fresh {
            continue;
        }
        let payload = json!({
            "taskId": id,
            "capability": t["capability"],
            "provider": t["provider"],
            "loginId": t["account_id"],
            "detail": t["status_detail"],
            "threadId": t["thread_id"],
        });
        if crate::runtime_events::record_user_event(&conn, owner, "subscription.task.needs_user", None, &payload, Some(&format!("subs-task:{id}:needs_user")))? {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

/// Owners whose Subscriptions gateway the sync reads: a bound Sessions
/// computer, a linked Agent Gateway account, or (when this runtime runs the
/// gateway itself) the owner it's paired as.
pub(crate) fn sync_owners(conn: &rusqlite::Connection, local_owner: Option<String>) -> rusqlite::Result<Vec<String>> {
    let mut owners = subscription_linked_owners(conn)?;
    let mut st = conn.prepare("SELECT user_id FROM subs_gateway_bindings")?;
    for o in st.query_map([], |r| r.get::<_, String>(0))? {
        owners.push(o?);
    }
    owners.extend(local_owner);
    owners.sort();
    owners.dedup();
    Ok(owners)
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

async fn gateway_get(state: &Arc<AppState>, owner: &str, path: &str, query: Option<&str>) -> Result<Value, String> {
    let resp = crate::subscription_routes::forward(state, &user_for(owner), path, Method::GET, &HeaderMap::new(), query, Bytes::new()).await;
    if !resp.status().is_success() {
        return Err(format!("subscriptions gateway answered {}", resp.status()));
    }
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

/// A `GET v1/accounts` listing → each login's health. A paused login serves
/// nothing: degraded, not signed out.
pub(crate) fn logins_from(rows: &[Value]) -> Vec<LoginHealth> {
    rows.iter()
        .filter_map(|r| {
            let id = r.get("account_id")?.as_str()?.to_string();
            let enabled = r.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let health = if enabled { r.get("session_health")?.as_str()?.to_string() } else { "paused".to_string() };
            Some(LoginHealth {
                id,
                provider: r.get("provider").and_then(Value::as_str).map(str::to_string),
                health,
                label: r.pointer("/user_action/label").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
            })
        })
        .collect()
}

/// Reads the owner's Subscriptions logins through their Sessions computer,
/// records login events, and applies them to linked accounts. An unreachable
/// gateway changes nothing (an outage isn't a sign-out).
pub(crate) async fn sync_owner(state: &Arc<AppState>, owner: &str) -> Result<Vec<Moved>, String> {
    let rows = gateway_get(state, owner, "v1/accounts", None).await?;
    let rows = rows.as_array().cloned().ok_or("unreadable account listing")?;
    let logins = logins_from(&rows);
    let db = state.db.clone();
    let o = owner.to_string();
    let l2 = logins.clone();
    tokio::task::spawn_blocking(move || record_login_health(&db, &o, &l2))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    // Tasks waiting on the person (best effort: an older gateway without the
    // listing just skips this).
    if let Ok(list) = gateway_get(state, owner, "v1/tasks", Some("status=needs_user&limit=100")).await {
        let tasks = list["tasks"].as_array().cloned().unwrap_or_default();
        let db = state.db.clone();
        let o = owner.to_string();
        let _ = tokio::task::spawn_blocking(move || record_needs_user_tasks(&db, &o, &tasks)).await;
    }
    let health: HashMap<String, String> = logins.into_iter().map(|l| (l.id, l.health)).collect();
    let db = state.db.clone();
    let o = owner.to_string();
    tokio::task::spawn_blocking(move || apply_snapshot(&db, &o, &health))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// `POST /gateway/provider-accounts/sync-subscriptions`: sync now (the UI
/// calls it after a sign-in so the account doesn't wait for the timer).
pub(crate) async fn sync_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    match sync_owner(&state, &user.user_id).await {
        Ok(moved) => Json(json!({
            "moved": moved.iter().map(|(id, from, to)| json!({ "accountId": id, "from": from, "to": to })).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": "subscriptions_unreachable", "detail": e }))).into_response(),
    }
}

async fn has_linked(state: &Arc<AppState>, owner: &str) -> bool {
    let db = state.db.clone();
    let o = owner.to_string();
    matches!(tokio::task::spawn_blocking(move || db.connect().and_then(|c| subscription_linked_accounts(&c, &o))).await, Ok(Ok(v)) if !v.is_empty())
}

/// Background loop: every minute, every owner with a linked account.
pub async fn run_sync_loop(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(SYNC_EVERY);
    loop {
        tick.tick().await;
        let db = state.db.clone();
        let local = crate::subscription_routes::local_gateway().and_then(|_| crate::relay_auth::process_secret().paired_owner());
        let owners = match tokio::task::spawn_blocking(move || db.connect().and_then(|c| sync_owners(&c, local))).await {
            Ok(Ok(o)) => o,
            _ => continue,
        };
        for owner in owners {
            if let Err(e) = sync_owner(&state, &owner).await {
                // A Sessions computer that's off is normal; only say so loudly
                // when a linked account is waiting on it.
                if has_linked(&state, &owner).await {
                    warn!(owner = %owner, error = %e, "subscription sync skipped");
                } else {
                    tracing::debug!(owner = %owner, error = %e, "subscription sync skipped");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_maps_to_account_state() {
        assert_eq!(health_target(Some("ready")), "CONNECTED");
        assert_eq!(health_target(Some("auth_required")), "EXPIRED");
        assert_eq!(health_target(Some("challenge_presented")), "BLOCKED");
        assert_eq!(health_target(Some("account_restricted")), "BLOCKED");
        assert_eq!(health_target(Some("ui_drift")), "DEGRADED");
        assert_eq!(health_target(Some("paused")), "DEGRADED");
        assert_eq!(health_target(None), "REVOKED");
    }

    #[test]
    fn paths_use_only_legal_hops() {
        assert_eq!(connection_path("CONNECTED", "EXPIRED"), Some(vec!["EXPIRED"]));
        assert_eq!(connection_path("EXPIRED", "CONNECTED"), Some(vec!["AUTHENTICATING", "VERIFYING", "CONNECTED"]));
        assert_eq!(connection_path("BLOCKED", "CONNECTED"), Some(vec!["AUTHENTICATING", "VERIFYING", "CONNECTED"]));
        assert_eq!(connection_path("DEGRADED", "CONNECTED"), Some(vec!["CONNECTED"]));
        assert_eq!(connection_path("CONNECTED", "CONNECTED"), Some(vec![]));
    }

    fn login(id: &str, health: &str) -> LoginHealth {
        LoginHealth { id: id.into(), provider: Some("chatgpt".into()), health: health.into(), label: None }
    }

    fn user_events(db: &DbHandle, owner: &str) -> Vec<(String, Value)> {
        let c = db.connect().unwrap();
        let mut st = c.prepare("SELECT event_type, payload FROM runtime_user_events WHERE user_id = ?1 ORDER BY seq").unwrap();
        st.query_map(params![owner], |r| Ok((r.get::<_, String>(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap()))).unwrap().map(Result::unwrap).collect()
    }

    async fn state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-subsync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    #[tokio::test]
    async fn login_health_changes_become_ledger_events() {
        let st = state().await;
        let db = &st.db;
        // First sight: a login that needs signing in is announced; a ready one is baseline.
        let got = record_login_health(db, "u", &[login("a", "auth_required"), login("b", "ready")]).unwrap();
        assert_eq!(got, vec![("a".to_string(), "subscription.login_needed")]);
        // No change, no event.
        assert!(record_login_health(db, "u", &[login("a", "auth_required"), login("b", "ready")]).unwrap().is_empty());
        // a signs in; b hits a bot check (also needs the person).
        let got = record_login_health(db, "u", &[login("a", "ready"), login("b", "challenge_presented")]).unwrap();
        assert_eq!(got, vec![("a".to_string(), "subscription.signed_in"), ("b".to_string(), "subscription.login_needed")]);
        // A check turning into a sign-in prompt is still the same need: no second event.
        assert!(record_login_health(db, "u", &[login("a", "ready"), login("b", "auth_required")]).unwrap().is_empty());
        // An outage and its recovery are not sign-ins.
        assert!(record_login_health(db, "u", &[login("a", "provider_down"), login("b", "auth_required")]).unwrap().is_empty());
        assert!(record_login_health(db, "u", &[login("a", "ready"), login("b", "auth_required")]).unwrap().is_empty());
        // A login added later that is already ready was just signed in.
        let got = record_login_health(db, "u", &[login("a", "ready"), login("b", "auth_required"), login("c", "ready")]).unwrap();
        assert_eq!(got, vec![("c".to_string(), "subscription.signed_in")]);
        // Removed logins are forgotten.
        record_login_health(db, "u", &[login("a", "ready")]).unwrap();
        let n: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM subscription_login_health WHERE owner = 'u'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let ev = user_events(db, "u");
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[1].1["previous"], "auth_required");
        assert_eq!(ev[1].1["provider"], "chatgpt");
        // Another owner is separate.
        assert!(user_events(db, "v").is_empty());
    }

    #[tokio::test]
    async fn needs_user_tasks_are_recorded_once_and_only_when_recent() {
        let st = state().await;
        let now = chrono::Utc::now().to_rfc3339();
        let old = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        let tasks = vec![
            json!({ "task_id": "t1", "status": "needs_user", "provider": "chatgpt", "status_detail": "Sign in", "updated_at": now, "prompt_preview": "private" }),
            json!({ "task_id": "t2", "status": "needs_user", "updated_at": old }),
            json!({ "task_id": "t3", "status": "running", "updated_at": now }),
        ];
        assert_eq!(record_needs_user_tasks(&st.db, "u", &tasks).unwrap(), vec!["t1"]);
        assert!(record_needs_user_tasks(&st.db, "u", &tasks).unwrap().is_empty());
        let ev = user_events(&st.db, "u");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "subscription.task.needs_user");
        assert_eq!(ev[0].1["detail"], "Sign in");
        assert!(ev[0].1.get("prompt_preview").is_none());
    }

    #[test]
    fn listing_rows_become_login_health() {
        let rows = vec![
            json!({ "account_id": "a", "provider": "claude", "session_health": "auth_required", "user_action": { "label": "Sign in" } }),
            json!({ "account_id": "b", "session_health": "ready", "enabled": false }),
            json!({ "provider": "x" }),
        ];
        let l = logins_from(&rows);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].health.as_str(), l[0].label.as_deref(), l[0].provider.as_deref()), ("auth_required", Some("Sign in"), Some("claude")));
        assert_eq!(l[1].health, "paused");
    }
}
