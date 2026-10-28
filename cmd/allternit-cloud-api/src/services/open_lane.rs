//! Open lane: per-user terms + handoff settings, the hard-line block log,
//! and the pure decisions the router hooks use.
//!
//! Tables (migration 105): `user_open_lane` (AUP acceptance, age
//! confirmation, handoff opt-in) and `open_lane_blocks` (category only — no
//! prompt text is ever stored).

use axum::{
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::{error::ApiError, model_router::ChatCompletionRequest};

/// The open model a refused request is handed to unless the request names one.
pub const DEFAULT_HANDOFF_MODEL: &str = "deepseek-v4.1-flash-open";

/// Response header naming a handoff: `<from>-><to>`.
pub const HANDOFF_HEADER: &str = "x-allternit-handoff";

/// Version of the acceptable use text the UI shows; stored acceptance is
/// for this text.
pub const AUP_VERSION: &str = "2026-10-28";

/// One user's open-lane settings. No row = nothing accepted, handoff off.
#[derive(Debug, Clone, Default, Serialize, sqlx::FromRow)]
pub struct OpenLaneSettings {
    pub aup_accepted_at: Option<DateTime<Utc>>,
    pub age_confirmed_at: Option<DateTime<Utc>>,
    pub handoff_enabled: bool,
}

impl OpenLaneSettings {
    /// Both gates passed: acceptable use accepted and 18+ confirmed.
    pub fn terms_ok(&self) -> bool {
        self.aup_accepted_at.is_some() && self.age_confirmed_at.is_some()
    }
}

/// Per-request Allternit options, read from (and removed from) the request's
/// `allternit` object so they never reach an upstream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestOptions {
    /// `allternit.handoff_model`: the open model to hand a refusal to.
    pub handoff_model: Option<String>,
    /// `allternit.handoff: false` turns handoff off for this one request.
    pub handoff_disabled: bool,
}

/// Remove `allternit` from the request and return its options.
pub fn take_request_options(request: &mut ChatCompletionRequest) -> RequestOptions {
    let Some(value) = request.extra.remove("allternit") else {
        return RequestOptions::default();
    };
    RequestOptions {
        handoff_model: value["handoff_model"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        handoff_disabled: value["handoff"].as_bool() == Some(false),
    }
}

/// Whether a refused completion should be handed to an open model.
/// `requested_is_open`: the request already targeted an open-lane model (no
/// second hop). `has_credits`: the open lane is paid; free-allowance users
/// are not handed off. `target_available`: the open model's upstream is set.
pub fn should_hand_off(
    settings: &OpenLaneSettings,
    options: &RequestOptions,
    requested_is_open: bool,
    has_credits: bool,
    target_available: bool,
) -> bool {
    settings.handoff_enabled
        && settings.terms_ok()
        && !options.handoff_disabled
        && !requested_is_open
        && has_credits
        && target_available
}

/// The `allternit.handoff` object added to a handed-off response.
pub fn handoff_field(from: &str, to: &str) -> Value {
    json!({ "from": from, "to": to, "reason": "refusal" })
}

/// `<from>-><to>` header value (falls back to `handoff` if not ASCII-safe).
pub fn handoff_header_value(from: &str, to: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{from}->{to}")).unwrap_or_else(|_| HeaderValue::from_static("handoff"))
}

/// Add `allternit.handoff` to a non-streaming completion body.
pub fn annotate_handoff(body: &mut Value, from: &str, to: &str) {
    if let Some(object) = body.as_object_mut() {
        let allternit = object.entry("allternit").or_insert_with(|| json!({}));
        if let Some(a) = allternit.as_object_mut() {
            a.insert("handoff".to_string(), handoff_field(from, to));
        }
    }
}

/// The SSE event announcing a streaming handoff (an OpenAI chunk with no
/// choices, like the usage chunk, carrying `allternit.handoff`).
pub fn handoff_sse_event(from: &str, to: &str) -> String {
    let chunk = json!({
        "id": format!("handoff-{}", uuid::Uuid::new_v4().simple()),
        "object": "chat.completion.chunk",
        "created": Utc::now().timestamp(),
        "model": to,
        "choices": [],
        "allternit": { "handoff": handoff_field(from, to) },
    });
    format!("data: {chunk}\n\n")
}

fn error_response(status: StatusCode, code: &str, message: &str, extra: Option<(&str, Value)>) -> Response {
    let mut error = json!({ "message": message, "type": "invalid_request_error", "code": code });
    if let Some((key, value)) = extra {
        error[key] = value;
    }
    (status, Json(json!({ "error": error }))).into_response()
}

/// 403 `open_lane_terms_required`.
pub fn terms_required_response() -> Response {
    error_response(
        StatusCode::FORBIDDEN,
        "open_lane_terms_required",
        "Open models need the acceptable use policy accepted and age (18+) confirmed. Turn them on in Settings > Open models.",
        None,
    )
}

/// 402 `open_lane_requires_credits` (the open lane is a paid feature).
pub fn credits_required_response() -> Response {
    error_response(
        StatusCode::PAYMENT_REQUIRED,
        "open_lane_requires_credits",
        "Open models are a paid feature. Add credits or a plan to use them.",
        None,
    )
}

/// 400 `content_policy_hard_line`.
pub fn hard_line_response(category: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        "content_policy_hard_line",
        "This request falls under a hard line no Allternit model serves.",
        Some(("category", Value::String(category.to_string()))),
    )
}

/// 503 `open_lane_moderation_unavailable` (fail closed).
pub fn moderation_unavailable_response() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "open_lane_moderation_unavailable",
        "Open models are briefly unavailable because the safety check could not run. Try again shortly.",
        None,
    )
}

/// Load a user's settings (defaults when there is no row).
pub async fn load_settings(db: &PgPool, user_id: &str) -> Result<OpenLaneSettings, ApiError> {
    let row = sqlx::query_as::<_, OpenLaneSettings>(
        "SELECT aup_accepted_at, age_confirmed_at, handoff_enabled FROM user_open_lane WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.unwrap_or_default())
}

/// Write a user's settings (upsert).
pub async fn save_settings(db: &PgPool, user_id: &str, settings: &OpenLaneSettings) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO user_open_lane (user_id, aup_accepted_at, age_confirmed_at, handoff_enabled, updated_at)
        VALUES ($1, $2, $3, $4, now())
        ON CONFLICT (user_id) DO UPDATE SET
            aup_accepted_at = EXCLUDED.aup_accepted_at,
            age_confirmed_at = EXCLUDED.age_confirmed_at,
            handoff_enabled = EXCLUDED.handoff_enabled,
            updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(settings.aup_accepted_at)
    .bind(settings.age_confirmed_at)
    .bind(settings.handoff_enabled)
    .execute(db)
    .await?;
    Ok(())
}

/// Log a hard-line block: category only. A logging failure never changes
/// the outcome (the request is blocked either way).
pub async fn record_block(db: &PgPool, user_id: &str, category: &str) {
    if let Err(error) = sqlx::query("INSERT INTO open_lane_blocks (user_id, category) VALUES ($1, $2)")
        .bind(user_id)
        .bind(category)
        .execute(db)
        .await
    {
        tracing::warn!(%error, category, "failed to log open-lane block");
    }
    tracing::info!(user_id, category, "open lane hard-line block");
}

/// Apply a settings update. Withdrawing acceptance or the age confirmation
/// also turns handoff off; handoff can only be turned on once both gates are
/// passed.
pub fn apply_update(
    mut settings: OpenLaneSettings,
    accept_aup: Option<bool>,
    confirm_age: Option<bool>,
    handoff_enabled: Option<bool>,
    now: DateTime<Utc>,
) -> Result<OpenLaneSettings, String> {
    match accept_aup {
        Some(true) => {
            settings.aup_accepted_at.get_or_insert(now);
        }
        Some(false) => settings.aup_accepted_at = None,
        None => {}
    }
    match confirm_age {
        Some(true) => {
            settings.age_confirmed_at.get_or_insert(now);
        }
        Some(false) => settings.age_confirmed_at = None,
        None => {}
    }
    if let Some(enabled) = handoff_enabled {
        if enabled && !settings.terms_ok() {
            return Err("Accept the acceptable use policy and confirm you are 18 or older first".to_string());
        }
        settings.handoff_enabled = enabled;
    }
    if !settings.terms_ok() {
        settings.handoff_enabled = false;
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(handoff: bool) -> OpenLaneSettings {
        OpenLaneSettings {
            aup_accepted_at: Some(Utc::now()),
            age_confirmed_at: Some(Utc::now()),
            handoff_enabled: handoff,
        }
    }

    #[test]
    fn hands_off_only_when_every_condition_holds() {
        let opts = RequestOptions::default();
        assert!(should_hand_off(&accepted(true), &opts, false, true, true));
        assert!(!should_hand_off(&accepted(false), &opts, false, true, true));
        assert!(!should_hand_off(&OpenLaneSettings { handoff_enabled: true, ..Default::default() }, &opts, false, true, true));
        assert!(!should_hand_off(&accepted(true), &opts, true, true, true), "no second hop from an open model");
        assert!(!should_hand_off(&accepted(true), &opts, false, false, true), "free users are not handed off");
        assert!(!should_hand_off(&accepted(true), &opts, false, true, false), "open upstream unset");
        let off = RequestOptions { handoff_disabled: true, ..Default::default() };
        assert!(!should_hand_off(&accepted(true), &off, false, true, true));
    }

    #[test]
    fn request_options_are_removed_from_the_request() {
        let mut request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [],
            "allternit": { "handoff_model": "glm-5.3-flash-open", "handoff": true }
        }))
        .unwrap();
        let options = take_request_options(&mut request);
        assert_eq!(options.handoff_model.as_deref(), Some("glm-5.3-flash-open"));
        assert!(!options.handoff_disabled);
        assert!(!request.extra.contains_key("allternit"));
    }

    #[test]
    fn update_rules() {
        let now = Utc::now();
        assert!(apply_update(OpenLaneSettings::default(), None, None, Some(true), now).is_err());
        let s = apply_update(OpenLaneSettings::default(), Some(true), Some(true), Some(true), now).unwrap();
        assert!(s.terms_ok() && s.handoff_enabled);
        let s = apply_update(s, Some(false), None, None, now).unwrap();
        assert!(!s.terms_ok() && !s.handoff_enabled, "withdrawing the AUP turns handoff off");
    }

    #[test]
    fn handoff_annotations() {
        let mut body = json!({ "choices": [] });
        annotate_handoff(&mut body, "gpt-4o", "deepseek-v4.1-flash-open");
        assert_eq!(body["allternit"]["handoff"]["to"], "deepseek-v4.1-flash-open");
        assert_eq!(handoff_header_value("a", "b").to_str().unwrap(), "a->b");
        let event = handoff_sse_event("a", "b");
        assert!(event.starts_with("data: ") && event.ends_with("\n\n"));
        assert!(event.contains("\"handoff\""));
    }
}
