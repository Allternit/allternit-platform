//! GET /api/v1/setup/checks — the setup checklist's cloud-owned rows.
//!
//! Each row answers "does this work right now", decided from a real call:
//! the signed-in session that reached this route (account and plan), a
//! `list_models` call to every hosted model upstream and a re-validation of
//! the user's stored BYOK keys with their providers (brain), the computers'
//! heartbeat (`paired_computers.last_seen_at`), and the user's live channel
//! routes. A configured value alone is never `ready`.
//!
//! Rows run in parallel, each under [`CHECK_TIMEOUT`]; one failing, timing
//! out or panicking never fails the rest (it reads `not_checked`). Responses
//! carry verdicts and short sentences only: no keys, tokens, mesh addresses
//! or secrets. Local `allternit-api` proxies this route and adds the rows it
//! owns (connected accounts); see `cmd/allternit-api/src/setup_routes.rs`.

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::{error::ApiError, routes::paired_computers::signed_in_caller, services, ApiState};

/// How long one row may take before it reads `not_checked`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(6);
/// Per-upstream budget inside the brain row.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(4);
/// A computer that reported within this window counts as reachable.
const COMPUTER_FRESH_MINUTES: i64 = 5;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    NotChecked,
    NeedsSetup,
}

/// Where the fix lives: a Settings section of the app (`section` is a
/// Settings nav id such as `billing`), never another origin.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fix {
    pub label: String,
    pub section: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub title: String,
    pub required: bool,
    pub status: Status,
    pub detail: String,
    pub fix: Option<Fix>,
}

fn row(id: &str, title: &str, required: bool, status: Status, detail: impl Into<String>, fix: Option<(&str, &str)>) -> Row {
    Row {
        id: id.to_string(),
        title: title.to_string(),
        required,
        status,
        detail: detail.into(),
        fix: fix.map(|(label, section)| Fix { label: label.to_string(), section: section.to_string() }),
    }
}

/// A row for a check that did not answer in time or failed.
fn unchecked(id: &str, title: &str, required: bool, why: &str, fix: Option<(&str, &str)>) -> Row {
    row(id, title, required, Status::NotChecked, why, fix)
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/setup/checks", get(get_checks))
}

async fn get_checks(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let user_id = caller.user_id.clone();

    let (account, brain, computers, channels) = tokio::join!(
        guarded("account", "Account and plan", true, account_row(&state, &user_id)),
        guarded("brain", "Brain and model access", true, brain_row(&state, &user_id)),
        guarded("computers", "Your computers", false, computers_row(&state, &caller.user_id, caller.organization_id.as_deref())),
        guarded("channels", "Channels", false, channels_row(&state, &user_id)),
    );
    Ok(Json(json!({
        "scope": "cloud",
        "generatedAt": Utc::now().to_rfc3339(),
        "rows": [account, brain, computers, channels],
    })))
}

/// Run one check under the timeout; a timeout reads `not_checked`.
async fn guarded(id: &str, title: &str, required: bool, check: impl std::future::Future<Output = Row>) -> Row {
    match tokio::time::timeout(CHECK_TIMEOUT, check).await {
        Ok(row) => row,
        Err(_) => unchecked(id, title, required, "This check took too long. Try Check again.", None),
    }
}

fn plan_label(plan_id: &str) -> &'static str {
    match plan_id {
        "plus" => "Plus",
        "super" => "Super",
        "ultra" => "Ultra",
        _ => "Free",
    }
}

async fn account_row(state: &ApiState, user_id: &str) -> Row {
    let sub: Result<Option<(String, String)>, _> = sqlx::query_as(
        "SELECT plan_id, status FROM billing_subscriptions
         WHERE user_id = $1 AND status IN ('active', 'trialing')
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match sub {
        // The session that called this route is the real sign-in check.
        Ok(sub) => {
            let plan = if crate::auth::is_admin_user(user_id) {
                "ultra".to_string()
            } else {
                sub.map(|(plan, _)| plan).unwrap_or_else(|| "free".to_string())
            };
            let detail = format!("Signed in on the {} plan.", plan_label(&plan));
            row("account", "Account and plan", true, Status::Ready, detail, Some(("Open billing", "billing")))
        }
        Err(_) => unchecked(
            "account",
            "Account and plan",
            true,
            "You are signed in, but your plan could not be read just now.",
            Some(("Open billing", "billing")),
        ),
    }
}

/// What the BYOK and hosted-model calls found, reduced to a verdict.
#[derive(Debug, Default)]
pub(crate) struct BrainFacts {
    /// `(provider, Ok(accepted) | Err(unreachable))` for each stored BYOK key.
    pub byok: Vec<(String, Result<bool, ()>)>,
    /// `(upstream, answered)` for each hosted-model provider.
    pub upstreams: Vec<(String, bool)>,
    /// `Some(balance)` when the user has a credits row.
    pub credits: Option<f64>,
    pub paid: bool,
}

pub(crate) fn brain_verdict(facts: &BrainFacts) -> Row {
    const ID: &str = "brain";
    const TITLE: &str = "Brain and model access";
    let fix_keys = Some(("Open API keys", "api-keys"));
    if let Some((provider, _)) = facts.byok.iter().find(|(_, v)| matches!(v, Ok(true))) {
        return row(ID, TITLE, true, Status::Ready, format!("Your {provider} key was accepted by the provider."), None);
    }
    let hosted_ok = facts.upstreams.iter().any(|(_, ok)| *ok);
    if hosted_ok {
        let broke = !facts.paid && matches!(facts.credits, Some(balance) if balance <= 0.0);
        return if broke {
            row(ID, TITLE, true, Status::NeedsSetup, "Allternit models answered, but your credits are used up. Top up or add your own key.", Some(("Open billing", "billing")))
        } else {
            row(ID, TITLE, true, Status::Ready, "Allternit models answered a live request.", None)
        };
    }
    if facts.byok.iter().any(|(_, v)| matches!(v, Ok(false))) && facts.byok.iter().all(|(_, v)| !matches!(v, Err(_))) {
        return row(ID, TITLE, true, Status::NeedsSetup, "Your provider rejected the saved key. Add a fresh one.", fix_keys);
    }
    if facts.upstreams.is_empty() && facts.byok.is_empty() {
        return row(ID, TITLE, true, Status::NeedsSetup, "No model access is set up. Add your own key or choose a plan.", fix_keys);
    }
    unchecked(ID, TITLE, true, "No model provider answered in time. Try Check again.", fix_keys)
}

async fn brain_row(state: &ApiState, user_id: &str) -> Row {
    let byok = async {
        match &state.inference_key_service {
            Some(service) => service.verdicts_for_user(user_id).await,
            None => Vec::new(),
        }
    };
    let upstreams = state.model_router.check_upstreams(UPSTREAM_TIMEOUT);
    let sub = async {
        sqlx::query_scalar::<_, String>(
            "SELECT plan_id FROM billing_subscriptions WHERE user_id = $1 AND status IN ('active', 'trialing') LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    };
    let credits = async { services::credit_balance_row(&state.db, user_id).await.ok().flatten() };
    let (byok, upstreams, sub, credits) = tokio::join!(byok, upstreams, sub, credits);
    let facts = BrainFacts {
        byok,
        upstreams,
        credits,
        paid: sub.is_some() || crate::auth::is_admin_user(user_id),
    };
    brain_verdict(&facts)
}

/// One paired computer, as far as the checklist needs it.
#[derive(Debug, Clone)]
pub(crate) struct ComputerFact {
    pub last_seen_at: Option<DateTime<Utc>>,
}

pub(crate) fn computers_verdict(computers: &[ComputerFact], now: DateTime<Utc>) -> Row {
    const ID: &str = "computers";
    const TITLE: &str = "Your computers";
    let fix = Some(("Open computers", "computers"));
    if computers.is_empty() {
        return row(ID, TITLE, false, Status::NeedsSetup, "No computer is paired yet. Pair one so bots can work on a real machine.", fix);
    }
    let fresh = ChronoDuration::minutes(COMPUTER_FRESH_MINUTES);
    let online = computers
        .iter()
        .filter(|c| matches!(c.last_seen_at, Some(seen) if now - seen <= fresh))
        .count();
    if online > 0 {
        return row(ID, TITLE, false, Status::Ready, format!("{online} of {} paired computers reported in the last {COMPUTER_FRESH_MINUTES} minutes.", computers.len()), fix);
    }
    row(ID, TITLE, false, Status::NotChecked, "Your paired computers are asleep or offline. They are checked once they are running.", fix)
}

async fn computers_row(state: &ApiState, user_id: &str, organization_id: Option<&str>) -> Row {
    let rows: Result<Vec<(Option<DateTime<Utc>>,)>, _> = sqlx::query_as(
        "SELECT last_seen_at FROM paired_computers
         WHERE user_id = $1 OR (organization_id IS NOT NULL AND organization_id = $2)",
    )
    .bind(user_id)
    .bind(organization_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let facts: Vec<ComputerFact> = rows.into_iter().map(|(last_seen_at,)| ComputerFact { last_seen_at }).collect();
            computers_verdict(&facts, Utc::now())
        }
        Err(_) => unchecked("computers", "Your computers", false, "Your computers could not be listed just now.", Some(("Open computers", "computers"))),
    }
}

pub(crate) fn channels_verdict(providers: &[String]) -> Row {
    const ID: &str = "channels";
    const TITLE: &str = "Channels";
    let fix = Some(("Open connections", "subscriptions"));
    if providers.is_empty() {
        return row(ID, TITLE, false, Status::NeedsSetup, "No channel is connected. Connect one to reach your bots from chat apps.", fix);
    }
    let mut names: Vec<String> = providers.to_vec();
    names.sort();
    names.dedup();
    row(ID, TITLE, false, Status::Ready, format!("Connected: {}.", names.join(", ")), fix)
}

async fn channels_row(state: &ApiState, user_id: &str) -> Row {
    let rows: Result<Vec<(String,)>, _> = sqlx::query_as(
        "SELECT provider FROM channel_inbound_routes WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => channels_verdict(&rows.into_iter().map(|(p,)| p).collect::<Vec<_>>()),
        Err(_) => unchecked("channels", "Channels", false, "Your channels could not be listed just now.", Some(("Open connections", "subscriptions"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(ok: bool) -> Vec<(String, bool)> {
        vec![("hosted".to_string(), ok)]
    }

    #[test]
    fn brain_ready_needs_a_real_answer() {
        let facts = BrainFacts { upstreams: up(true), paid: true, ..Default::default() };
        assert_eq!(brain_verdict(&facts).status, Status::Ready);
        let silent = BrainFacts { upstreams: up(false), ..Default::default() };
        assert_eq!(brain_verdict(&silent).status, Status::NotChecked);
    }

    #[test]
    fn brain_rejected_key_needs_setup_and_exhausted_credits_too() {
        let rejected = BrainFacts { byok: vec![("openai".into(), Ok(false))], upstreams: up(false), ..Default::default() };
        assert_eq!(brain_verdict(&rejected).status, Status::NeedsSetup);
        let broke = BrainFacts { upstreams: up(true), credits: Some(0.0), ..Default::default() };
        assert_eq!(brain_verdict(&broke).status, Status::NeedsSetup);
        assert_eq!(brain_verdict(&BrainFacts::default()).status, Status::NeedsSetup);
    }

    #[test]
    fn computers_sleeping_is_not_checked() {
        let now = Utc::now();
        assert_eq!(computers_verdict(&[], now).status, Status::NeedsSetup);
        let old = ComputerFact { last_seen_at: Some(now - ChronoDuration::hours(2)) };
        assert_eq!(computers_verdict(&[old.clone()], now).status, Status::NotChecked);
        let fresh = ComputerFact { last_seen_at: Some(now - ChronoDuration::minutes(1)) };
        assert_eq!(computers_verdict(&[old, fresh], now).status, Status::Ready);
    }

    #[test]
    fn channels_list_providers_once() {
        let row = channels_verdict(&["telegram".into(), "telegram".into()]);
        assert_eq!(row.status, Status::Ready);
        assert_eq!(row.detail, "Connected: telegram.");
        assert_eq!(channels_verdict(&[]).status, Status::NeedsSetup);
    }
}
