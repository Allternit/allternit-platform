//! Account preferences that follow the person across devices, even with no
//! computer (migration `145_account_prefs.sql`, one JSON document per
//! `(user_id, key)`). Project folders plan, step C3.
//!
//! - `GET /api/v1/account/pad-templates` → `{ templates, modeDefaults }`; with
//!   nothing saved, no templates and the built-in default per mode
//!   ([`default_pad_templates`]).
//! - `PUT /api/v1/account/pad-templates` `{ templates, modeDefaults }` →
//!   the saved document. The whole document is replaced (upsert). Checked by
//!   [`validate_pad_templates`]; anything invalid is a 400 with a plain
//!   message and nothing is saved. Fields the contract doesn't name are dropped.
//!
//! Stored under key `pad_templates`. Pads themselves live on the runtime
//! (`/project-pad/:scope`); templates belong to the person, so they live here.
//!
//! Auth: a Clerk session, or an API token with the `account` scope
//! ([`crate::auth::resolve_user_scoped`]); none is 401. Never gated on
//! compute. Erased with the account (`account_prefs` in account_erase.rs).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::error::ApiError;
use crate::ApiState;

/// The `account_prefs.key` the pad templates document is stored under.
pub const PAD_TEMPLATES_KEY: &str = "pad_templates";
/// Largest PUT body accepted.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

const MAX_TEMPLATES: usize = 24;
const MAX_TILES: usize = 24;
const MAX_ACTIONS: usize = 12;
const MAX_NAME_CHARS: usize = 40;
const MAX_LABEL_CHARS: usize = 40;
const MAX_VALUE_CHARS: usize = 2048;
const MAX_ACTION_ID_CHARS: usize = 64;

/// Pad tile ids: the 8 Pulse tiles plus the 5 pad tiles.
pub const TILE_IDS: &[&str] = &[
    "decision", "countdown", "progress", "team", "chart", "schedule", "files", "activity",
    "actions", "links", "notes", "checklist", "threads",
];
/// Grid sizes: S 1x1, M 2x1, L 2x2, W 4x1.
pub const TILE_SIZES: &[&str] = &["S", "M", "L", "W"];
/// Built-in template ids (they live in client code, never in the stored document).
pub const BUILT_IN_TEMPLATES: &[&str] = &["launch", "client", "build", "research", "launchpad"];
/// Each mode and its built-in default template.
pub const MODE_DEFAULTS: &[(&str, &str)] = &[("chat", "research"), ("code", "build"), ("design", "client"), ("bot", "launchpad")];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/account/pad-templates", get(get_pad_templates).put(put_pad_templates))
}

/// What GET answers when the person never saved templates.
pub fn default_pad_templates() -> Value {
    let defaults: Map<String, Value> =
        MODE_DEFAULTS.iter().map(|(mode, template)| (mode.to_string(), Value::String(template.to_string()))).collect();
    json!({ "templates": [], "modeDefaults": defaults })
}

/// `u_` followed by a hyphenated UUID.
fn is_user_template_id(id: &str) -> bool {
    id.strip_prefix("u_").is_some_and(|rest| rest.len() == 36 && uuid::Uuid::parse_str(rest).is_ok())
}

/// An absolute http(s) URL with a host. Rejects `javascript:`, `data:` and the like.
fn is_http_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()))
}

fn text<'a>(object: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    object.get(field).and_then(Value::as_str)
}

/// The document to store for a PUT body, or a plain message saying what is wrong.
pub fn validate_pad_templates(body: &Value) -> Result<Value, String> {
    let body = body.as_object().ok_or_else(|| "Expected a JSON object { templates, modeDefaults }.".to_string())?;
    let templates = body.get("templates").and_then(Value::as_array).ok_or_else(|| "templates must be an array.".to_string())?;
    if templates.len() > MAX_TEMPLATES {
        return Err(format!("At most {MAX_TEMPLATES} templates."));
    }

    let mut ids: Vec<&str> = Vec::with_capacity(templates.len());
    let mut out = Vec::with_capacity(templates.len());
    for (index, template) in templates.iter().enumerate() {
        let n = index + 1;
        let template = template.as_object().ok_or_else(|| format!("Template {n} must be an object."))?;

        let id = text(template, "id").unwrap_or("");
        if !is_user_template_id(id) {
            return Err(format!("Template {n}: id must be \"u_\" followed by a UUID."));
        }
        if ids.contains(&id) {
            return Err(format!("Template id {id} is used twice."));
        }
        ids.push(id);

        let name = text(template, "name").map(str::trim).unwrap_or("");
        let name_chars = name.chars().count();
        if name_chars == 0 || name_chars > MAX_NAME_CHARS {
            return Err(format!("Template {n}: name must be 1 to {MAX_NAME_CHARS} characters."));
        }

        let tiles = template.get("tiles").and_then(Value::as_array).ok_or_else(|| format!("Template {n}: tiles must be an array."))?;
        if tiles.len() > MAX_TILES {
            return Err(format!("Template {n}: at most {MAX_TILES} tiles."));
        }
        let mut tiles_out = Vec::with_capacity(tiles.len());
        for tile in tiles {
            let tile = tile.as_object().ok_or_else(|| format!("Template {n}: each tile must be an object."))?;
            let tile_id = text(tile, "id").unwrap_or("");
            if !TILE_IDS.contains(&tile_id) {
                return Err(format!("Template {n}: unknown tile \"{tile_id}\"."));
            }
            let size = text(tile, "size").unwrap_or("");
            if !TILE_SIZES.contains(&size) {
                return Err(format!("Template {n}: tile size must be S, M, L or W."));
            }
            tiles_out.push(json!({ "id": tile_id, "size": size }));
        }

        let actions: &[Value] = match template.get("actions") {
            None | Some(Value::Null) => &[],
            Some(Value::Array(actions)) => actions.as_slice(),
            Some(_) => return Err(format!("Template {n}: actions must be an array.")),
        };
        if actions.len() > MAX_ACTIONS {
            return Err(format!("Template {n}: at most {MAX_ACTIONS} actions."));
        }
        let mut actions_out = Vec::with_capacity(actions.len());
        for action in actions {
            let action = action.as_object().ok_or_else(|| format!("Template {n}: each action must be an object."))?;
            let action_id = text(action, "id").unwrap_or("");
            if action_id.is_empty() || action_id.chars().count() > MAX_ACTION_ID_CHARS {
                return Err(format!("Template {n}: each action needs an id of 1 to {MAX_ACTION_ID_CHARS} characters."));
            }
            let kind = text(action, "kind").unwrap_or("");
            if !matches!(kind, "thread" | "link") {
                return Err(format!("Template {n}: action kind must be thread or link."));
            }
            let label = text(action, "label").ok_or_else(|| format!("Template {n}: each action needs a label."))?;
            if label.chars().count() > MAX_LABEL_CHARS {
                return Err(format!("Template {n}: an action label holds at most {MAX_LABEL_CHARS} characters."));
            }
            let value = text(action, "value").ok_or_else(|| format!("Template {n}: each action needs a value."))?;
            if value.chars().count() > MAX_VALUE_CHARS {
                return Err(format!("Template {n}: an action value holds at most {MAX_VALUE_CHARS} characters."));
            }
            if kind == "link" && !is_http_url(value) {
                return Err(format!("Template {n}: a link action needs an http or https URL."));
            }
            actions_out.push(json!({ "id": action_id, "kind": kind, "label": label, "value": value }));
        }

        let created_at = template
            .get("createdAt")
            .filter(|v| v.as_f64().is_some_and(|at| at >= 0.0))
            .ok_or_else(|| format!("Template {n}: createdAt must be a number."))?;

        out.push(json!({ "id": id, "name": name, "tiles": tiles_out, "actions": actions_out, "createdAt": created_at }));
    }

    let defaults = body.get("modeDefaults").and_then(Value::as_object).ok_or_else(|| "modeDefaults must be an object.".to_string())?;
    if defaults.len() != MODE_DEFAULTS.len() || MODE_DEFAULTS.iter().any(|(mode, _)| !defaults.contains_key(*mode)) {
        return Err("modeDefaults must have exactly the keys chat, code, design and bot.".to_string());
    }
    let mut defaults_out = Map::new();
    for (mode, _) in MODE_DEFAULTS {
        let template = defaults.get(*mode).and_then(Value::as_str).unwrap_or("");
        if !BUILT_IN_TEMPLATES.contains(&template) && !ids.contains(&template) {
            return Err(format!("modeDefaults.{mode} must be a built-in template or one of your templates."));
        }
        defaults_out.insert(mode.to_string(), Value::String(template.to_string()));
    }

    Ok(json!({ "templates": out, "modeDefaults": defaults_out }))
}

/// The saved document for `key`, or `None` when there is none.
pub async fn read_pref(db: &PgPool, user: &str, key: &str) -> Result<Option<Value>, ApiError> {
    Ok(sqlx::query_scalar::<_, Value>("SELECT value FROM account_prefs WHERE user_id = $1 AND key = $2")
        .bind(user)
        .bind(key)
        .fetch_optional(db)
        .await?)
}

/// Replace the document for `key`.
pub async fn write_pref(db: &PgPool, user: &str, key: &str, value: &Value) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO account_prefs (user_id, key, value, updated_at) VALUES ($1, $2, $3, now())
         ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(user)
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

fn private(result: Result<Response, ApiError>) -> Response {
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

async fn get_pad_templates(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    private(
        async {
            let user = crate::auth::resolve_user_scoped(&state.db, &headers, "account").await?;
            let saved = read_pref(&state.db, &user.id, PAD_TEMPLATES_KEY).await?;
            Ok(Json(saved.unwrap_or_else(default_pad_templates)).into_response())
        }
        .await,
    )
}

async fn put_pad_templates(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    private(
        async {
            let user = crate::auth::resolve_user_scoped(&state.db, &headers, "account").await?;
            if body.len() > MAX_BODY_BYTES {
                return Err(ApiError::BadRequest(format!("The body holds at most {} KB.", MAX_BODY_BYTES / 1024)));
            }
            let parsed: Value = serde_json::from_slice(&body)
                .map_err(|_| ApiError::BadRequest("Expected a JSON body { templates, modeDefaults }.".to_string()))?;
            let document = validate_pad_templates(&parsed).map_err(ApiError::BadRequest)?;
            write_pref(&state.db, &user.id, PAD_TEMPLATES_KEY, &document).await?;
            Ok(Json(document).into_response())
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, test_state, MockGateway};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const PATH: &str = "/api/v1/account/pad-templates";
    const TEMPLATE_ID: &str = "u_6f1c2a4e-9b3d-4c1a-8f2e-1a2b3c4d5e6f";

    async fn router() -> Router {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/145_account_prefs.sql")).execute(&state.db).await.expect("145 applies");
        routes().with_state(state)
    }

    async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    fn valid_body() -> Value {
        json!({
            "templates": [{
                "id": TEMPLATE_ID,
                "name": "  Weekly launch  ",
                "tiles": [{ "id": "progress", "size": "S" }, { "id": "notes", "size": "W" }, { "id": "actions", "size": "M" }],
                "actions": [
                    { "id": "a1", "kind": "thread", "label": "Plan the week", "value": "Plan this week's launch tasks" },
                    { "id": "a2", "kind": "link", "label": "Board", "value": "https://example.com/board" },
                ],
                "createdAt": 1763164800000_i64,
                "extra": "dropped",
            }],
            "modeDefaults": { "chat": "research", "code": TEMPLATE_ID, "design": "client", "bot": "launchpad" },
        })
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn account_prefs_pad_templates_need_a_signed_in_caller() {
        let router = router().await;
        for method in ["GET", "PUT"] {
            let request = Request::builder()
                .method(method)
                .uri(PATH)
                .header("content-type", "application/json")
                .body(Body::from(valid_body().to_string()))
                .unwrap();
            let (status, _) = send(&router, request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}");
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn account_prefs_pad_templates_default_then_round_trip() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let router = router().await;

        let (status, body) = send(&router, authed_request("GET", PATH, "")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({ "templates": [], "modeDefaults": { "chat": "research", "code": "build", "design": "client", "bot": "launchpad" } })
        );

        let (status, saved) = send(&router, authed_request("PUT", PATH, &valid_body().to_string())).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["templates"][0]["name"], "Weekly launch");
        assert!(saved["templates"][0].get("extra").is_none(), "unknown fields are dropped: {saved}");

        let (status, read) = send(&router, authed_request("GET", PATH, "")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read, saved);
        assert_eq!(read["modeDefaults"]["code"], TEMPLATE_ID);
        assert_eq!(read["templates"][0]["tiles"], json!([{ "id": "progress", "size": "S" }, { "id": "notes", "size": "W" }, { "id": "actions", "size": "M" }]));
        assert_eq!(read["templates"][0]["actions"][1]["value"], "https://example.com/board");
        assert_eq!(read["templates"][0]["createdAt"], 1763164800000_i64);

        // A second save replaces the first (upsert).
        let (status, _) = send(&router, authed_request("PUT", PATH, &json!({
            "templates": [], "modeDefaults": { "chat": "launch", "code": "build", "design": "client", "bot": "launchpad" },
        }).to_string())).await;
        assert_eq!(status, StatusCode::OK);
        let (_, read) = send(&router, authed_request("GET", PATH, "")).await;
        assert_eq!((read["templates"].as_array().map(Vec::len), read["modeDefaults"]["chat"].as_str()), (Some(0), Some("launch")));
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn account_prefs_pad_templates_reject_invalid_bodies() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let router = router().await;

        let mut bad_size = valid_body();
        bad_size["templates"][0]["tiles"][0]["size"] = json!("XL");
        let mut script_link = valid_body();
        script_link["templates"][0]["actions"][1]["value"] = json!("javascript:alert(1)");
        let mut unknown_default = valid_body();
        unknown_default["modeDefaults"]["code"] = json!("u_00000000-0000-4000-8000-000000000000");
        let mut extra_mode = valid_body();
        extra_mode["modeDefaults"]["voice"] = json!("research");
        let mut bad_id = valid_body();
        bad_id["templates"][0]["id"] = json!("tmpl-1");
        bad_id["modeDefaults"]["code"] = json!("build");
        let mut unknown_tile = valid_body();
        unknown_tile["templates"][0]["tiles"][0]["id"] = json!("weather");
        let mut long_name = valid_body();
        long_name["templates"][0]["name"] = json!("x".repeat(41));
        let too_big = format!(r#"{{"templates":[],"modeDefaults":{{"chat":"research","code":"build","design":"client","bot":"launchpad"}},"pad":"{}"}}"#, "x".repeat(MAX_BODY_BYTES));

        let cases = [
            ("bad size", bad_size.to_string()),
            ("javascript: url", script_link.to_string()),
            ("unknown mode default", unknown_default.to_string()),
            ("extra mode key", extra_mode.to_string()),
            ("bad template id", bad_id.to_string()),
            ("unknown tile", unknown_tile.to_string()),
            ("long name", long_name.to_string()),
            ("over 64 KB", too_big),
            ("not json", "nope".to_string()),
        ];
        for (case, body) in cases {
            let (status, error) = send(&router, authed_request("PUT", PATH, &body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {error}");
            assert!(error["message"].as_str().is_some_and(|m| !m.is_empty()), "{case}: a plain message: {error}");
        }

        // Nothing invalid was saved.
        let (_, read) = send(&router, authed_request("GET", PATH, "")).await;
        assert_eq!(read, default_pad_templates());
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
