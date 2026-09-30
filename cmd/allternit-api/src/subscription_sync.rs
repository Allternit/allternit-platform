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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tracing::warn;

use crate::agent_gateway_routes::{apply_account_state, connection_path, subscription_linked_accounts, subscription_linked_owners};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

const SYNC_EVERY: Duration = Duration::from_secs(60);
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

/// Reads the owner's Subscriptions logins through their Sessions computer and
/// applies them. An unreachable gateway changes nothing (an outage isn't a
/// sign-out).
pub(crate) async fn sync_owner(state: &Arc<AppState>, owner: &str) -> Result<Vec<Moved>, String> {
    let db = state.db.clone();
    let o = owner.to_string();
    let linked = tokio::task::spawn_blocking(move || db.connect().and_then(|c| subscription_linked_accounts(&c, &o)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if linked.is_empty() {
        return Ok(vec![]);
    }
    let resp = crate::subscription_routes::forward(state, &user_for(owner), "v1/accounts", Method::GET, &HeaderMap::new(), None, Bytes::new()).await;
    if !resp.status().is_success() {
        return Err(format!("subscriptions gateway answered {}", resp.status()));
    }
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.map_err(|e| e.to_string())?;
    let rows: Vec<Value> = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let logins: HashMap<String, String> = rows
        .iter()
        .filter_map(|r| {
            let id = r.get("account_id")?.as_str()?.to_string();
            let enabled = r.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            // A paused login serves nothing: degraded, not signed out.
            let health = if enabled { r.get("session_health")?.as_str()?.to_string() } else { "paused".to_string() };
            Some((id, health))
        })
        .collect();
    let db = state.db.clone();
    let o = owner.to_string();
    tokio::task::spawn_blocking(move || apply_snapshot(&db, &o, &logins))
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

/// Background loop: every minute, every owner with a linked account.
pub async fn run_sync_loop(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(SYNC_EVERY);
    loop {
        tick.tick().await;
        let db = state.db.clone();
        let owners = match tokio::task::spawn_blocking(move || db.connect().and_then(|c| subscription_linked_owners(&c))).await {
            Ok(Ok(o)) => o,
            _ => continue,
        };
        for owner in owners {
            if let Err(e) = sync_owner(&state, &owner).await {
                warn!(owner = %owner, error = %e, "subscription sync skipped");
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
}
