//! The connect kit's account rows (CONTRACT-connections-2026-11, sections 3–5;
//! rules in services::connections).
//!
//! - `GET    /api/v1/connections`               → `{version, connections:[row]}`
//! - `POST   /api/v1/connections/:id/connect`   `{way?}` → `{login}` or `{state:"on"}`
//! - `POST   /api/v1/connections/:id/submit`    `{value}` → the updated row
//! - `GET    /api/v1/connections/:id/login`     → `{status, row}`
//! - `DELETE /api/v1/connections/:id`           → 204
//!
//! Signed-in Clerk session only: `auth::require_session_user` answers 403 for
//! every API token (platform `alt_…` keys included) and 401 with none. These
//! routes never call `require_compute`. Keys are checked with the vendor
//! before they are stored and are never returned, only a masked tail.
//!
//! Cloud `/login` is stateless: it reports `done` once the row is `on`,
//! `failed` once it needs the person, and `pending` otherwise. It never says
//! `expired`; clients stop polling at the login's `expires_at`.
//!
//! Plan rows (`claude`, `chatgpt`, `grok`, `kimi`; contract section 9) run
//! on the person's brain runner box. `GET` lists them as connectable only
//! with `?caps=runner` (older clients get `update_app`). `connect` takes
//! `{accept_disclosure?}` and answers `409 disclosure_required` with the
//! disclosure until the current version is accepted; `submit`, `login` and
//! `DELETE` go to the runner (`DELETE` signs out and deletes the CLI home).
//! None of them call `require_compute`: an account with no computer gets a
//! login, never `compute_required`.
//!
//! `copilot` (contract 9.5, 9.11) is a server flow: with the GitHub App
//! configured, `connect` answers an `oauth` login and
//! `GET /api/v1/connections/copilot/callback` (no session; the signed state
//! names the account) stores the sealed user token. Without the App env vars
//! the row stays "Coming soon.".
//!
//! Rows whose sign-in runs on the person's computer (coding tools, ChatGPT,
//! the local brain, Gmail/Drive/GitHub/Notion) are started on the runtime's
//! own `/api/v1/connections`; here `connect` answers 409.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::services::brain_runners::{self, BrainRunnerService, LoginStatus, Vendor, DISCLOSURE_VERSION};
use crate::services::connections::{self as conn, Row, State as ConnState};
use crate::services::voice_keys::VoiceKeyService;
use crate::ApiState;

/// No `Debug`: the body can hold an API key.
#[derive(Deserialize)]
struct SubmitBody {
    value: String,
}

/// `{way?, accept_disclosure?}`: every cloud row has one way today, so `way`
/// is accepted and not needed; `accept_disclosure` is for plan rows (9.2).
#[derive(Deserialize, Default)]
struct ConnectBody {
    #[serde(default)]
    #[allow(dead_code)]
    way: Option<String>,
    #[serde(default)]
    accept_disclosure: Option<String>,
    /// `copilot` only: where the callback sends the browser afterwards.
    #[serde(default)]
    return_to: Option<String>,
}

#[derive(Deserialize, Default)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    caps: Option<String>,
}

#[cfg(test)]
tokio::task_local! {
    /// Tests run the routes against a runner service with a fake backend.
    pub(crate) static TEST_RUNNERS: Arc<BrainRunnerService>;
}

/// The process's brain runner service.
pub(crate) fn runners(state: &ApiState) -> Arc<BrainRunnerService> {
    #[cfg(test)]
    if let Ok(service) = TEST_RUNNERS.try_with(Arc::clone) {
        return service;
    }
    brain_runners::shared(&state.db)
}

const DEFAULT_DOWNLOAD_URL: &str = "https://www.allternit.com/download";
const PHONE_URL: &str = "https://m.allternit.com";
/// How long a computer, phone or app login is polled.
const LOGIN_MINUTES: i64 = 15;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/connections", get(list))
        .route("/api/v1/connections/copilot/callback", get(copilot_callback))
        .route("/api/v1/connections/:id", axum::routing::delete(remove))
        .route("/api/v1/connections/:id/connect", post(connect))
        .route("/api/v1/connections/:id/submit", post(submit))
        .route("/api/v1/connections/:id/login", get(login))
}

/// The signed-in caller's id; API tokens are 403.
pub(crate) async fn session_user(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    // Tests stand in for a Clerk session with this header; compiled out of
    // every non-test build (same pattern as voice_keys::session_user).
    #[cfg(test)]
    if let Some(id) = headers.get("x-test-session-user").and_then(|v| v.to_str().ok()) {
        return Ok(id.to_string());
    }
    Ok(crate::auth::require_session_user(&state.db, headers).await?.id)
}

fn reply(result: Result<Response, ApiError>) -> Response {
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

async fn rows_for(state: &ApiState, user_id: &str, runner_caps: bool) -> Result<Vec<Row>, ApiError> {
    let keys = match &state.inference_key_service {
        Some(service) => conn::saved_keys(service.list_for_user(user_id).await?),
        None => Vec::new(),
    };
    let connected = crate::routes::runtime_relay::connected_runtime_ids().await;
    let mut facts = conn::load_facts(&state.db, user_id, keys, &connected, Utc::now()).await?;
    // A failed runner read (e.g. migration 140 not applied yet) leaves the
    // section 3 plan rows; it never takes the whole list down.
    facts.runners = conn::load_runner_facts(&state.db, &runners(state), user_id).await.unwrap_or_else(|error| {
        tracing::warn!(%error, "brain runner rows unavailable; showing the not-yet rows");
        Vec::new()
    });
    facts.runner_caps = runner_caps;
    let router = &state.model_router;
    let mut rows = conn::build_rows(&facts, &|provider| router.listed_aliases_for_provider(provider));
    if let Some(service) = crate::services::copilot::service(state) {
        if let Some(row) = rows.iter_mut().find(|row| row.id == "copilot") {
            copilot_row(&service, user_id, row).await;
        }
    }
    Ok(rows)
}

/// The `copilot` row with the GitHub App configured (contract 9.5): connectable,
/// person-only, its chat models listed while `on`.
async fn copilot_row(service: &crate::services::copilot::CopilotService, user_id: &str, row: &mut Row) {
    use crate::services::copilot::GrantState;
    row.available = true;
    row.unavailable_reason = None;
    row.reason_code = None;
    row.fallbacks = vec!["github"];
    row.person_only = true;
    // A failed read (e.g. migration 142 not applied yet) shows the row off.
    let grant = service.grant_state(user_id).await.unwrap_or_else(|error| {
        tracing::warn!(%error, "copilot grant unavailable; showing the row off");
        GrantState::Off
    });
    match grant {
        GrantState::Off => row.state = ConnState::Off,
        GrantState::Connecting => row.state = ConnState::Connecting,
        GrantState::On { login, models } => {
            row.state = ConnState::On;
            row.found = Some(login.map_or_else(|| "GitHub".to_string(), |login| format!("GitHub · @{login}")));
            row.models = models.into_iter().map(|model| format!("copilot/{model}")).collect();
        }
        GrantState::Expired => {
            row.state = ConnState::Needs;
            row.reason = Some(conn::reason_text("login_expired").to_string());
            row.reason_code = Some("login_expired");
        }
    }
}

/// The GitHub App's redirect URL. No session: the signed, single-use `state`
/// names the account. Always a 302 to the client's return URL.
async fn copilot_callback(
    State(state): State<Arc<ApiState>>,
    axum::extract::Query(query): axum::extract::Query<CallbackQuery>,
) -> Response {
    let target = match crate::services::copilot::service(&state) {
        Some(service) => service.callback(query.code.as_deref(), query.state.as_deref(), query.error.as_deref()).await,
        None => format!("{}&error=not_configured", crate::services::copilot::DEFAULT_RETURN),
    };
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(location) = HeaderValue::from_str(&target) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    reply(Ok(response))
}

/// The row as a client that knows plan rows sees it: `connect`, `submit`,
/// `login` and `DELETE` are only called by those clients.
async fn row_for(state: &ApiState, user_id: &str, id: &str) -> Result<Row, ApiError> {
    rows_for(state, user_id, true)
        .await?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("No connection {id:?}.")))
}

/// A 409 that names why the cloud cannot start this row, with the row.
fn cannot_start(code: &str, reason_code: Option<&str>, message: &str, row: &Row) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": { "code": code, "message": message },
            "reason_code": reason_code,
            "row": row,
        })),
    )
        .into_response()
}

fn expires_at(minutes: i64) -> String {
    (Utc::now() + Duration::minutes(minutes)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn login_reply(kind: &str, url: Option<&str>, poll: bool, minutes: Option<i64>) -> Response {
    Json(json!({ "login": {
        "kind": kind,
        "code": Value::Null,
        "url": url,
        "url_with_code": Value::Null,
        "expires_at": minutes.map(expires_at),
        "poll": poll,
    }}))
    .into_response()
}

async fn list(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<ListQuery>,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let caps = query.caps.as_deref().unwrap_or("").split(',').any(|cap| cap.trim() == "runner");
        let rows = rows_for(&state, &user, caps).await?;
        Ok(Json(json!({ "version": conn::VERSION, "connections": rows })).into_response())
    }
    .await)
}

/// The account's runtime a channel binds to: the default node when it is one
/// of the account's computers, else the one seen last.
async fn channel_runtime(db: &sqlx::PgPool, user_id: &str) -> Result<Option<String>, ApiError> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT d.id FROM runtime_devices d
        LEFT JOIN user_node_preferences p ON p.user_id = d.user_id AND p.surface = '*' AND p.node_id = d.id
        WHERE d.user_id = $1 AND d.revoked_at IS NULL AND d.capabilities LIKE '%"runtime:connect"%'
        ORDER BY (p.node_id IS NOT NULL) DESC, d.last_seen_at DESC NULLS LAST
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}

// ---- plan rows (contract section 9) ------------------------------------

/// A section 9 error body.
fn plan_error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
}

/// A brain runner error as its section 9 response.
fn runner_error(error: ApiError) -> Result<Response, ApiError> {
    match brain_runners::error_response(&error) {
        Some(response) => Ok(response),
        None => Err(error),
    }
}

/// The plan vendor behind a row id the server offers, or the 404 of 9.2.
fn offered_vendor(service: &BrainRunnerService, id: &str) -> Option<Result<Vendor, Response>> {
    let vendor = Vendor::parse(id)?;
    Some(if service.offers(vendor) {
        Ok(vendor)
    } else {
        Err(plan_error(StatusCode::NOT_FOUND, "not_found", "This plan cannot be connected here."))
    })
}

/// `409 disclosure_required` unless this account accepted the current text.
async fn disclosure_gate(service: &BrainRunnerService, user: &str, vendor: Vendor) -> Result<Option<Response>, ApiError> {
    if service.accepted_version(user, vendor).await?.as_deref() == Some(DISCLOSURE_VERSION) {
        return Ok(None);
    }
    let disclosure = service.disclosure_for(vendor).await;
    Ok(Some(
        (
            StatusCode::CONFLICT,
            Json(json!({ "error": {
                "code": "disclosure_required",
                "message": "Read and accept this first.",
                "disclosure": disclosure,
            }})),
        )
            .into_response(),
    ))
}

fn login_status_word(status: LoginStatus) -> &'static str {
    match status {
        LoginStatus::Pending => "pending",
        LoginStatus::Done => "done",
        LoginStatus::Expired => "expired",
        LoginStatus::Failed => "failed",
    }
}

async fn connect_plan(state: &ApiState, user: &str, vendor: Vendor, accept: Option<&str>) -> Result<Response, ApiError> {
    let service = runners(state);
    // A stale or missing version falls through to the 409 with the current text.
    if accept == Some(DISCLOSURE_VERSION) {
        service.accept_disclosure(user, vendor, DISCLOSURE_VERSION).await?;
    }
    if let Some(refusal) = disclosure_gate(&service, user, vendor).await? {
        return Ok(refusal);
    }
    match service.start_login(user, vendor).await {
        Ok(view) => Ok(Json(json!({ "login": {
            "kind": view.kind,
            "code": view.code,
            "url": view.url,
            "url_with_code": view.url_with_code,
            "expires_at": view.expires_at.map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
            "poll": view.poll,
        }}))
        .into_response()),
        Err(error) => runner_error(error),
    }
}

async fn connect(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: ConnectBody = if body.is_empty() {
            ConnectBody::default()
        } else {
            serde_json::from_slice(&body)
                .map_err(|_| ApiError::BadRequest("Expected a JSON body {way?, accept_disclosure?}.".to_string()))?
        };
        match offered_vendor(&runners(&state), &id) {
            Some(Ok(vendor)) => return connect_plan(&state, &user, vendor, body.accept_disclosure.as_deref()).await,
            Some(Err(not_offered)) => return Ok(not_offered),
            None => {}
        }
        let row = match row_for(&state, &user, &id).await {
            Ok(row) => row,
            // With a computer online the computer's rows are not listed here.
            Err(ApiError::NotFound(_)) if conn::runtime_owned(&id) => {
                return Ok((
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": { "code": "runtime_row", "message": "Connect this on your computer." },
                        "reason_code": Value::Null,
                    })),
                )
                    .into_response());
            }
            Err(error) => return Err(error),
        };
        if !row.available {
            return Ok(cannot_start(
                "not_available",
                row.reason_code,
                row.unavailable_reason.unwrap_or("Not available yet."),
                &row,
            ));
        }
        if id == "allternit" {
            return Ok(Json(json!({ "state": "on" })).into_response());
        }
        if id == "copilot" {
            if let Some(service) = crate::services::copilot::service(&state) {
                let (url, expires) = service.authorize_url(&user, body.return_to.as_deref()).await?;
                return Ok(Json(json!({ "login": {
                    "kind": "oauth",
                    "code": Value::Null,
                    "url": url,
                    "url_with_code": Value::Null,
                    "expires_at": expires.to_rfc3339_opts(SecondsFormat::Secs, true),
                    "poll": true,
                }}))
                .into_response());
            }
        }
        if conn::key_provider_for(&id).is_some() || conn::voice_provider_for(&id).is_some() || id == "api-key" {
            return Ok(login_reply("paste", Some(conn::key_page(&id)), false, None));
        }
        let download_url = std::env::var("ALLTERNIT_DOWNLOAD_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DOWNLOAD_URL.to_string());
        match id.as_str() {
            // Install, then the app pairs itself; /login turns done when it is online.
            "this-mac" | "server" => Ok(login_reply("pair", Some(&download_url), true, Some(LOGIN_MINUTES))),
            "phone" | "phone-control" => Ok(login_reply("scan", Some(PHONE_URL), true, Some(LOGIN_MINUTES))),
            "slack" => match crate::routes::slack_app::app_config() {
                Some(cfg) => {
                    let url = crate::routes::slack_app::authorize_url(&cfg, &user);
                    Ok(login_reply("oauth", Some(&url), true, Some(LOGIN_MINUTES)))
                }
                None => Ok(cannot_start("not_configured", Some("could_not_start"), "Slack is not set up on this server.", &row)),
            },
            "discord" | "whatsapp" => {
                let Some(runtime) = channel_runtime(&state.db, &user).await? else {
                    return Ok(cannot_start(
                        "no_computer",
                        Some("no_computer"),
                        "Channels answer through your bot's computer. Add a computer first.",
                        &row,
                    ));
                };
                let url = if id == "discord" {
                    let Some(cfg) = crate::routes::discord_app::DiscordConfig::from_env() else {
                        return Ok(cannot_start("not_configured", Some("could_not_start"), "Discord is not set up on this server.", &row));
                    };
                    let signed = crate::routes::discord_app::sign_state(&cfg.client_secret, &user, &runtime, Utc::now().timestamp());
                    crate::routes::discord_app::install_url(&cfg, &signed)
                } else {
                    if crate::routes::whatsapp_es::MetaConfig::from_env().is_none() {
                        return Ok(cannot_start("not_configured", Some("could_not_start"), "WhatsApp is not set up on this server.", &row));
                    }
                    crate::routes::whatsapp_es::signup_link(&state.db, &user, &runtime).await?
                };
                Ok(login_reply("oauth", Some(&url), true, Some(LOGIN_MINUTES)))
            }
            "cloud-computer" => {
                let config = crate::routes::account_compute::GateConfig::from_env();
                let paths = crate::routes::account_compute::paths_for(&state.db, &user, &config).await?;
                Ok((
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": { "code": "choose_plan", "message": "A cloud computer comes with a plan or credits." },
                        "reason_code": Value::Null,
                        "paths": paths,
                        "row": row,
                    })),
                )
                    .into_response())
            }
            "telegram" => Ok(cannot_start(
                "start_from_bot",
                Some("could_not_start"),
                "Telegram connects to one bot. Start it from that bot's channels.",
                &row,
            )),
            // Computer-side sign-ins: the runtime's own /api/v1/connections starts them.
            _ => Ok(cannot_start(
                "runtime_row",
                if row.runs_on == "computer" { Some("computer_offline") } else { Some("could_not_start") },
                "Connect this on your computer.",
                &row,
            )),
        }
    }
    .await)
}

async fn submit(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: SubmitBody =
            serde_json::from_slice(&body).map_err(|_| ApiError::BadRequest("Expected a JSON body {value}.".to_string()))?;
        let value = body.value.trim();
        if value.is_empty() {
            return Err(ApiError::BadRequest("Paste a key first.".to_string()));
        }
        let service = runners(&state);
        match offered_vendor(&service, &id) {
            Some(Ok(vendor)) => {
                if let Some(refusal) = disclosure_gate(&service, &user, vendor).await? {
                    return Ok(refusal);
                }
                if let Err(error) = service.submit_code(&user, vendor, value).await {
                    return runner_error(error);
                }
                return Ok(Json(row_for(&state, &user, &id).await?).into_response());
            }
            Some(Err(not_offered)) => return Ok(not_offered),
            None => {}
        }
        let keys = state
            .inference_key_service
            .as_ref()
            .ok_or_else(|| ApiError::ServiceUnavailable("Keys are not set up on this server.".to_string()));

        let checked = if let Some(provider) = conn::key_provider_for(&id) {
            keys?.upsert_and_validate(&user, provider, value).await.map(|_| ())
        } else if let Some(provider) = conn::voice_provider_for(&id) {
            // The OpenAI voice key is an OpenAI key (same vendor check); the
            // ElevenLabs key is checked with `GET /v1/user`.
            let check = keys?.check_only(provider, value).await;
            match check {
                Ok(()) => VoiceKeyService::new(state.db.clone(), state.credential_cipher.clone())
                    .upsert(&user, provider, value, None, None)
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            }
        } else {
            return Err(ApiError::BadRequest(format!("{id:?} does not take a pasted value here.")));
        };

        match checked {
            Ok(()) => Ok(Json(row_for(&state, &user, &id).await?).into_response()),
            // A rejected key is stored nowhere; the reply is the row as it
            // stands, marked `needs` with `key_rejected` (contract section 4).
            Err(ApiError::BadRequest(message)) if message == crate::services::inference_keys::KEY_REJECTED_MESSAGE => {
                let mut row = row_for(&state, &user, &id).await?;
                if row.state != ConnState::On {
                    row.state = ConnState::Needs;
                    row.reason = Some(conn::reason_text("key_rejected").to_string());
                    row.reason_code = Some("key_rejected");
                }
                Ok((StatusCode::UNPROCESSABLE_ENTITY, Json(row)).into_response())
            }
            Err(ApiError::ServiceUnavailable(message)) => Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": { "code": "could_not_start", "message": message },
                    "reason_code": "could_not_start",
                })),
            )
                .into_response()),
            Err(error) => Err(error),
        }
    }
    .await)
}

async fn login(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let service = runners(&state);
        match offered_vendor(&service, &id) {
            Some(Ok(vendor)) => {
                if let Some(refusal) = disclosure_gate(&service, &user, vendor).await? {
                    return Ok(refusal);
                }
                let status = match service.login_status(&user, vendor).await {
                    Ok(status) => status,
                    Err(error) => return runner_error(error),
                };
                let row = row_for(&state, &user, &id).await?;
                return Ok(Json(json!({ "status": login_status_word(status), "row": row })).into_response());
            }
            Some(Err(not_offered)) => return Ok(not_offered),
            None => {}
        }
        let row = row_for(&state, &user, &id).await?;
        let status = match row.state {
            ConnState::On => "done",
            ConnState::Needs => "failed",
            ConnState::Off | ConnState::Connecting => "pending",
        };
        Ok(Json(json!({ "status": status, "row": row })).into_response())
    }
    .await)
}

async fn remove(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let runner_service = runners(&state);
        match offered_vendor(&runner_service, &id) {
            // Sign out on the runner and delete that CLI home; the consent stays.
            Some(Ok(vendor)) => {
                return match runner_service.logout(&user, vendor).await {
                    Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
                    Err(error) => runner_error(error),
                };
            }
            Some(Err(not_offered)) => return Ok(not_offered),
            None => {}
        }
        // Copilot: revoke at GitHub, then delete the sealed grant.
        if id == "copilot" {
            if let Some(service) = crate::services::copilot::service(&state) {
                service.disconnect(&user).await?;
                return Ok(StatusCode::NO_CONTENT.into_response());
            }
        }
        if let Some(provider) = conn::key_provider_for(&id) {
            if let Some(service) = &state.inference_key_service {
                service.delete(&user, provider).await?;
            }
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        if let Some(provider) = conn::voice_provider_for(&id) {
            sqlx::query("DELETE FROM user_voice_keys WHERE user_id = $1 AND provider = $2")
                .bind(&user)
                .bind(provider)
                .execute(&state.db)
                .await?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        let row = row_for(&state, &user, &id).await?;
        Ok(cannot_start(
            "remove_in_place",
            None,
            "Remove this from its own settings (computers, phones, channels and apps keep their own remove step).",
            &row,
        ))
    }
    .await)
}

#[cfg(test)]
#[path = "connections_tests.rs"]
mod tests;
