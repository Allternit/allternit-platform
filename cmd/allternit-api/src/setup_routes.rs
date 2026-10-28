//! GET /setup/checks (served at `/api/v1/setup/checks`): the setup checklist.
//!
//! One row per thing the user's Allternit depends on, each `ready`,
//! `not_checked` or `needs_setup`, decided from a real call, never from a
//! configured value alone. The rows split by who can see the truth:
//!
//! - Cloud-owned rows (account and plan, brain and model access, computers,
//!   channels) come from the cloud API's `GET /api/v1/setup/checks`, called
//!   with the caller's own Bearer. The hosted web app reaches the same route
//!   on cloud-api directly. When the cloud cannot be reached, those four rows
//!   read `not_checked` and say why.
//! - Rows owned by this runtime (Google Gmail and Calendar, other connected
//!   accounts) are decided here: the connector sidecar is asked which
//!   connections it still holds, and native connections are checked for a
//!   usable token. Token values are never read into a response.
//!
//! Every check runs in parallel under a short timeout; a failing check turns
//! its own row into `not_checked` and never the response into an error.
//! Responses carry verdicts and short sentences only, no secrets.

use axum::{extract::Extension, extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::auth::AuthUser;
use crate::AppState;

const CLOUD_TIMEOUT: Duration = Duration::from_secs(8);
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(5);

const GOOGLE_IDS: [(&str, &str); 2] = [("gmail", "Gmail"), ("google_calendar", "Google Calendar")];

pub fn setup_router() -> Router<Arc<AppState>> {
    Router::new().route("/setup/checks", get(get_setup_checks))
}

fn row(id: &str, title: &str, required: bool, status: &str, detail: String, fix: Option<(&str, &str)>) -> Value {
    json!({
        "id": id,
        "title": title,
        "required": required,
        "status": status,
        "detail": detail,
        "fix": fix.map(|(label, section)| json!({ "label": label, "section": section })),
    })
}

/// The four rows the cloud owns, as `not_checked` with the reason.
fn cloud_placeholders(why: &str) -> Vec<Value> {
    [
        ("account", "Account and plan", true, "Open billing", "billing"),
        ("brain", "Brain and model access", true, "Open API keys", "api-keys"),
        ("computers", "Your computers", false, "Open computers", "computers"),
        ("channels", "Channels", false, "Open connections", "subscriptions"),
    ]
    .into_iter()
    .map(|(id, title, required, label, section)| row(id, title, required, "not_checked", why.to_string(), Some((label, section))))
    .collect()
}

async fn cloud_rows(state: &AppState, headers: &HeaderMap) -> Vec<Value> {
    let Some(base) = state.config.cloud_api_url() else {
        return cloud_placeholders("This backend is not connected to the Allternit cloud, so these could not be checked.");
    };
    let url = format!("{}/api/v1/setup/checks", base.trim_end_matches('/'));
    let mut request = reqwest::Client::new().get(&url).timeout(CLOUD_TIMEOUT);
    if let Some(authorization) = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        request = request.header(axum::http::header::AUTHORIZATION, authorization);
    }
    let body = match request.send().await {
        Ok(response) if response.status().is_success() => response.json::<Value>().await.ok(),
        _ => None,
    };
    match body.and_then(|b| b.get("rows").and_then(|r| r.as_array()).cloned()) {
        Some(rows) if !rows.is_empty() => rows,
        _ => cloud_placeholders("The Allternit cloud did not answer. Try Check again."),
    }
}

/// `(connector_id, status, backend, expires_at, has_refresh_token)` for each of the user's connections.
type ConnRow = (String, String, String, Option<String>, bool);

async fn local_connections(state: &AppState, user_id: &str) -> Result<Vec<ConnRow>, String> {
    let db = state.db.clone();
    let uid = user_id.to_string();
    tokio::task::spawn_blocking(move || -> Result<Vec<ConnRow>, String> {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT connector_id, status, backend, expires_at,
                        (refresh_token IS NOT NULL AND refresh_token != '')
                 FROM connector_connections WHERE user_id = ?1 AND status != 'disconnected'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)? != 0)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok().map(|t| t.and_utc()))
}

/// Whether one connection can still be used, given what the sidecar holds.
/// `live` is `None` when the sidecar could not be asked.
pub(crate) fn connection_state(row: &ConnRow, live: Option<&HashSet<String>>, now: DateTime<Utc>) -> Option<bool> {
    let (id, status, backend, expires_at, has_refresh) = row;
    if status != "connected" {
        return Some(false);
    }
    if backend == "rust_native" {
        let expired = expires_at.as_deref().and_then(parse_time).map(|t| t <= now).unwrap_or(false);
        return Some(!expired || *has_refresh);
    }
    // Sidecar-held credentials: only the sidecar knows they still exist.
    live.map(|set| set.contains(id))
}

pub(crate) fn google_row(conns: &[ConnRow], live: Option<&HashSet<String>>, now: DateTime<Utc>) -> Value {
    let fix = Some(("Connect Google", "subscriptions"));
    let mut ready = Vec::new();
    let mut broken = Vec::new();
    let mut unknown = Vec::new();
    let mut missing = Vec::new();
    for (id, label) in GOOGLE_IDS {
        match conns.iter().find(|c| c.0 == id) {
            None => missing.push(label),
            Some(c) => match connection_state(c, live, now) {
                Some(true) => ready.push(label),
                Some(false) => broken.push(label),
                None => unknown.push(label),
            },
        }
    }
    let (status, detail) = if ready.len() == GOOGLE_IDS.len() {
        ("ready", "Gmail and Google Calendar are connected and the sign-in is still valid.".to_string())
    } else if !broken.is_empty() {
        ("needs_setup", format!("{} needs to be reconnected; the saved sign-in no longer works.", broken.join(" and ")))
    } else if !unknown.is_empty() {
        ("not_checked", format!("{} could not be confirmed because the connector service did not answer.", unknown.join(" and ")))
    } else {
        ("needs_setup", format!("{} not connected yet.", missing.join(" and ")))
    };
    row("google", "Google (Gmail and Calendar)", false, status, detail, fix)
}

pub(crate) fn other_accounts_row(conns: &[ConnRow], live: Option<&HashSet<String>>, now: DateTime<Utc>) -> Value {
    let fix = Some(("Open connections", "subscriptions"));
    let others: Vec<&ConnRow> = conns.iter().filter(|c| !GOOGLE_IDS.iter().any(|(id, _)| *id == c.0)).collect();
    if others.is_empty() {
        return row("accounts", "Other connected accounts", false, "needs_setup", "No other accounts are connected. Add Slack, GitHub, Notion and more when you need them.".to_string(), fix);
    }
    let mut bad: Vec<String> = Vec::new();
    let mut unknown = 0;
    for c in &others {
        match connection_state(c, live, now) {
            Some(true) => {}
            Some(false) => bad.push(c.0.clone()),
            None => unknown += 1,
        }
    }
    bad.sort();
    bad.dedup();
    if !bad.is_empty() {
        return row("accounts", "Other connected accounts", false, "needs_setup", format!("Reconnect: {}.", bad.join(", ")), fix);
    }
    if unknown > 0 {
        return row("accounts", "Other connected accounts", false, "not_checked", "The connector service did not answer, so these accounts were not confirmed.".to_string(), fix);
    }
    row("accounts", "Other connected accounts", false, "ready", format!("{} connected account(s) confirmed.", others.len()), fix)
}

async fn local_rows(state: &AppState, user_id: &str) -> Vec<Value> {
    let (conns, live) = tokio::join!(
        local_connections(state, user_id),
        tokio::time::timeout(SIDECAR_TIMEOUT, crate::open_connector_proxy::list_user_connections_all(user_id)),
    );
    let conns = match conns {
        Ok(conns) => conns,
        Err(_) => {
            let why = "Your connected accounts could not be read just now. Try Check again.".to_string();
            return vec![
                row("google", "Google (Gmail and Calendar)", false, "not_checked", why.clone(), Some(("Connect Google", "subscriptions"))),
                row("accounts", "Other connected accounts", false, "not_checked", why, Some(("Open connections", "subscriptions"))),
            ];
        }
    };
    // Only ask the sidecar's verdict when something depends on it.
    let live: Option<HashSet<String>> = match live {
        Ok(Ok(list)) => Some(list.into_iter().map(|(service, _, _)| service).collect()),
        _ => None,
    };
    let now = Utc::now();
    vec![google_row(&conns, live.as_ref(), now), other_accounts_row(&conns, live.as_ref(), now)]
}

async fn get_setup_checks(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Json<Value> {
    let (cloud, local) = tokio::join!(cloud_rows(&state, &headers), local_rows(&state, &user.user_id));
    let mut rows = cloud;
    rows.extend(local);
    Json(json!({
        "scope": "runtime",
        "generatedAt": Utc::now().to_rfc3339(),
        "rows": rows,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: &str, status: &str, backend: &str) -> ConnRow {
        (id.to_string(), status.to_string(), backend.to_string(), None, false)
    }

    #[test]
    fn google_ready_only_when_sidecar_still_holds_both() {
        let conns = vec![conn("gmail", "connected", "open_connector"), conn("google_calendar", "connected", "open_connector")];
        let both: HashSet<String> = ["gmail", "google_calendar"].iter().map(|s| s.to_string()).collect();
        assert_eq!(google_row(&conns, Some(&both), Utc::now())["status"], "ready");
        let one: HashSet<String> = ["gmail"].iter().map(|s| s.to_string()).collect();
        assert_eq!(google_row(&conns, Some(&one), Utc::now())["status"], "needs_setup");
        assert_eq!(google_row(&conns, None, Utc::now())["status"], "not_checked");
        assert_eq!(google_row(&[], Some(&both), Utc::now())["status"], "needs_setup");
    }

    #[test]
    fn native_token_expiry_needs_a_refresh_token() {
        let mut c = conn("github", "connected", "rust_native");
        c.3 = Some("2000-01-01 00:00:00".to_string());
        assert_eq!(connection_state(&c, None, Utc::now()), Some(false));
        c.4 = true;
        assert_eq!(connection_state(&c, None, Utc::now()), Some(true));
    }
}
