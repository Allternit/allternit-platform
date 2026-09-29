//! Image Generation and Edit endpoints (A4, A5).
//!
//! Mounted under `/v1` by the LLM gateway router so the path surface matches
//! `POST /v1/images/generations` and `POST /v1/images/edits`. These endpoints
//! use the gateway's existing virtual-key middleware chain for authentication.
//!
//! Generations and edits run on the key owner's gpt-image key through the
//! media plane (BYOK OpenAI credential, then the flag-gated platform lane).
//! Edits accept a base64 PNG/JPEG/WebP source image (and optional mask).
//!
//! With no key: the placeholder SVG output is only returned when
//! `ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS=1` is set (dev machines, tests).
//! Without it — the default — the images endpoints return HTTP 501 with
//! code `allternit.not_configured`, because silently billing for placeholder
//! images on a production surface is never acceptable.

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::media::clients::{ImageEntry, MediaTransport, ReqwestTransport};
use crate::media::handlers::{base64_image_data_url, gateway_gpt_images, GatewayImageRequest};
use crate::AppState;

use super::{
    auth::LlmKeyContext,
    translate::{error_code, OpenAiErrorResponse},
};

/// Env var that re-enables fake provider output (dev only). See module header.
const ALLOW_FAKE_PROVIDERS_ENV: &str = "ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS";

/// Gatekeeper for the no-provider fallback: returns `Some(501
/// allternit.not_configured)` unless the fake-provider escape hatch is set.
/// `env_value` is injected so tests stay deterministic without touching
/// process-wide environment.
fn fake_provider_gate(env_value: Option<&str>, provider_kind: &str) -> Option<OpenAiErrorResponse> {
    if env_value == Some("1") {
        return None;
    }
    Some(OpenAiErrorResponse::new(
        StatusCode::NOT_IMPLEMENTED,
        format!("no {provider_kind} provider configured"),
        "not_configured",
        None,
        Some(error_code::NOT_CONFIGURED),
    ))
}

// ─── Request types ──────────────────────────────────────────────────────────

/// `POST /v1/images/generations` request body.
#[derive(Debug, Deserialize)]
pub struct CreateImageRequest {
    pub prompt: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_n")]
    pub n: u32,
    #[serde(default = "default_size")]
    pub size: String,
    #[serde(default = "default_quality")]
    pub quality: String,
    #[serde(default = "default_style")]
    pub style: Option<String>,
    #[serde(default = "default_response_format")]
    pub response_format: String,
    #[serde(default)]
    pub user: Option<String>,
}

fn default_model() -> String {
    "allternit-image-1".to_string()
}
fn default_n() -> u32 {
    1
}
fn default_size() -> String {
    "1024x1024".to_string()
}
fn default_quality() -> String {
    "standard".to_string()
}
fn default_style() -> Option<String> {
    Some("vivid".to_string())
}
fn default_response_format() -> String {
    "url".to_string()
}

/// `POST /v1/images/edits` request body.
#[derive(Debug, Deserialize)]
pub struct EditImageRequest {
    /// Base64-encoded source image to edit.
    pub image: String,
    pub prompt: String,
    /// Optional base64-encoded mask for inpainting.
    #[serde(default)]
    pub mask: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_n")]
    pub n: u32,
    #[serde(default = "default_size")]
    pub size: String,
    #[serde(default = "default_response_format")]
    pub response_format: String,
    #[serde(default)]
    pub user: Option<String>,
}

/// Variations request body.
#[derive(Debug, Deserialize)]
pub struct ImageVariationsRequest {
    /// Base64-encoded source image.
    pub image: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_n")]
    pub n: u32,
    #[serde(default = "default_size")]
    pub size: String,
    #[serde(default = "default_response_format")]
    pub response_format: String,
}

// ─── Response types ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ImageResponse {
    pub created: i64,
    pub data: Vec<ImageData>,
}

#[derive(Debug, Serialize)]
pub struct ImageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b64_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

// ─── Validation ─────────────────────────────────────────────────────────────

const VALID_SIZES: &[&str] = &[
    "256x256", "512x512", "1024x1024", "1792x1024", "1024x1792",
];

fn validate_size(size: &str) -> Result<(), OpenAiErrorResponse> {
    if VALID_SIZES.contains(&size) {
        Ok(())
    } else {
        Err(OpenAiErrorResponse::invalid_request(
            format!("`size` must be one of {VALID_SIZES:?}, got `{size}`."),
            Some("size"),
        ))
    }
}

fn validate_n(n: u32) -> Result<(), OpenAiErrorResponse> {
    if (1..=10).contains(&n) {
        Ok(())
    } else {
        Err(OpenAiErrorResponse::invalid_request(
            "`n` must be between 1 and 10.",
            Some("n"),
        ))
    }
}

fn validate_response_format(fmt: &str) -> Result<(), OpenAiErrorResponse> {
    match fmt {
        "url" | "b64_json" => Ok(()),
        _ => Err(OpenAiErrorResponse::invalid_request(
            "`response_format` must be `url` or `b64_json`.",
            Some("response_format"),
        )),
    }
}

// ─── Real provider path ─────────────────────────────────────────────────────

/// Shape gpt-image entries as an OpenAI images response in the requested
/// `response_format`. URL entries are downloaded when `b64_json` is asked for.
pub async fn entries_to_image_data(
    transport: &dyn MediaTransport,
    entries: Vec<ImageEntry>,
    response_format: &str,
) -> Result<Vec<ImageData>, String> {
    let mut data = Vec::new();
    for entry in entries {
        let item = match (entry, response_format) {
            (ImageEntry::B64 { b64_json }, "b64_json") => ImageData {
                url: None,
                b64_json: Some(b64_json),
                revised_prompt: None,
            },
            (ImageEntry::B64 { b64_json }, _) => ImageData {
                url: Some(format!("data:image/png;base64,{b64_json}")),
                b64_json: None,
                revised_prompt: None,
            },
            (ImageEntry::Url { url, .. }, "b64_json") => {
                let (status, bytes, _) = transport.get_bytes(&url, &[]).await?;
                if !(200..300).contains(&status) {
                    return Err(format!("image download status {status}"));
                }
                ImageData {
                    url: None,
                    b64_json: Some(STANDARD.encode(bytes)),
                    revised_prompt: None,
                }
            }
            (ImageEntry::Url { url, .. }, _) => ImageData {
                url: Some(url),
                b64_json: None,
                revised_prompt: None,
            },
        };
        data.push(item);
    }
    Ok(data)
}

/// Run the request on the caller's gpt-image key. `None` = no key available;
/// the handler then falls back to the not_configured gate.
async fn real_images(
    state: &AppState,
    user_id: &str,
    req: &GatewayImageRequest,
    response_format: &str,
) -> Option<Response> {
    let transport = ReqwestTransport::new();
    let result = match gateway_gpt_images(&state.db, &transport, user_id, req).await {
        Ok(None) => return None,
        Ok(Some(entries)) => entries_to_image_data(&transport, entries, response_format).await,
        Err(message) => Err(message),
    };
    Some(match result {
        Ok(data) => (
            StatusCode::OK,
            Json(json!({ "created": chrono::Utc::now().timestamp(), "data": data })),
        )
            .into_response(),
        Err(message) => OpenAiErrorResponse::new(
            StatusCode::BAD_GATEWAY,
            format!("image provider error: {message}"),
            "api_error",
            None,
            None,
        )
        .into_response(),
    })
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `POST /v1/images/generations` — generate one or more images from a prompt.
pub async fn create_images(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Json(body): Json<CreateImageRequest>,
) -> Response {
    if let Err(e) = validate_n(body.n) {
        return e.into_response();
    }
    if let Err(e) = validate_size(&body.size) {
        return e.into_response();
    }
    if let Err(e) = validate_response_format(&body.response_format) {
        return e.into_response();
    }
    if body.prompt.is_empty() {
        return OpenAiErrorResponse::invalid_request(
            "`prompt` must not be empty.",
            Some("prompt"),
        )
        .into_response();
    }

    let real_req = GatewayImageRequest {
        prompt: body.prompt.clone(),
        reference_data_url: None,
        mask_data_url: None,
        size: body.size.clone(),
        quality: body.quality.clone(),
        n: body.n,
    };
    if let Some(response) = real_images(&state, &key.user_id, &real_req, &body.response_format).await {
        return response;
    }
    if let Some(err) =
        fake_provider_gate(std::env::var(ALLOW_FAKE_PROVIDERS_ENV).ok().as_deref(), "images")
    {
        return err.into_response();
    }

    let created = chrono::Utc::now().timestamp();

    // Dev placeholder path: store generation metadata in SQLite.
    let generation_id = format!("img_{}", uuid::Uuid::new_v4().simple());
    let db = state.db.clone();
    let gid = generation_id.clone();
    let prompt = body.prompt.clone();
    let model = body.model.clone();
    let size = body.size.clone();
    let quality = body.quality.clone();
    let n = body.n;

    let store_result = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = db.connect()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS image_generations (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                model TEXT NOT NULL,
                size TEXT NOT NULL,
                quality TEXT NOT NULL,
                n INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "INSERT INTO image_generations (id, prompt, model, size, quality, n, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![gid, prompt, model, size, quality, n as i64, created],
        )?;
        Ok(())
    })
    .await;

    if let Err(e) = store_result {
        tracing::warn!(error = %e, "Failed to store image generation metadata");
    }

    // Generate placeholder SVG images (1x1 transparent pixel per image).
    // In production, this would proxy to a configured image provider.
    let data: Vec<ImageData> = (0..body.n)
        .map(|i| {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="100%" height="100%" fill="#1a1a2e"/><text x="50%" y="50%" text-anchor="middle" dy=".3em" fill="#e0e0e0" font-family="monospace" font-size="14">{prompt_short}</text></svg>"##,
                w = body.size.split('x').next().unwrap_or("1024"),
                h = body.size.split('x').nth(1).unwrap_or("1024"),
                prompt_short = if body.prompt.len() > 40 {
                    format!("{}...", &body.prompt[..37])
                } else {
                    body.prompt.clone()
                },
            );
            let encoded = STANDARD.encode(svg.as_bytes());
            if body.response_format == "b64_json" {
                ImageData {
                    url: None,
                    b64_json: Some(encoded),
                    revised_prompt: Some(format!(
                        "{} (image {}/{})",
                        body.prompt,
                        i + 1,
                        body.n
                    )),
                }
            } else {
                ImageData {
                    url: Some(format!(
                        "data:image/svg+xml;base64,{encoded}"
                    )),
                    b64_json: None,
                    revised_prompt: Some(format!(
                        "{} (image {}/{})",
                        body.prompt,
                        i + 1,
                        body.n
                    )),
                }
            }
        })
        .collect();

    (StatusCode::OK, Json(json!({
        "created": created,
        "data": data,
    })))
        .into_response()
}

/// `POST /v1/images/edits` — edit an existing image using a prompt.
pub async fn edit_images(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Json(body): Json<EditImageRequest>,
) -> Response {
    if let Err(e) = validate_n(body.n) {
        return e.into_response();
    }
    if let Err(e) = validate_size(&body.size) {
        return e.into_response();
    }
    if let Err(e) = validate_response_format(&body.response_format) {
        return e.into_response();
    }
    if body.prompt.is_empty() {
        return OpenAiErrorResponse::invalid_request(
            "`prompt` must not be empty.",
            Some("prompt"),
        )
        .into_response();
    }

    // The source image (and mask) must be base64 PNG, JPEG, or WebP.
    let Some(reference) = base64_image_data_url(&body.image) else {
        return OpenAiErrorResponse::invalid_request(
            "`image` must be base64-encoded PNG, JPEG, or WebP data.",
            Some("image"),
        )
        .into_response();
    };
    let mask = match body.mask.as_deref().map(base64_image_data_url) {
        None => None,
        Some(Some(mask)) => Some(mask),
        Some(None) => {
            return OpenAiErrorResponse::invalid_request(
                "`mask` must be base64-encoded PNG data.",
                Some("mask"),
            )
            .into_response();
        }
    };

    let real_req = GatewayImageRequest {
        prompt: body.prompt.clone(),
        reference_data_url: Some(reference),
        mask_data_url: mask,
        size: body.size.clone(),
        quality: "medium".to_string(),
        n: body.n,
    };
    if let Some(response) = real_images(&state, &key.user_id, &real_req, &body.response_format).await {
        return response;
    }
    if let Some(err) =
        fake_provider_gate(std::env::var(ALLOW_FAKE_PROVIDERS_ENV).ok().as_deref(), "images")
    {
        return err.into_response();
    }

    let created = chrono::Utc::now().timestamp();

    // Store edit metadata.
    let edit_id = format!("imgedit_{}", uuid::Uuid::new_v4().simple());
    let db = state.db.clone();
    let eid = edit_id.clone();
    let prompt = body.prompt.clone();
    let model = body.model.clone();
    let size = body.size.clone();
    let n = body.n;

    let store_result = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = db.connect()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS image_edits (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                model TEXT NOT NULL,
                size TEXT NOT NULL,
                n INTEGER NOT NULL,
                has_mask INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "INSERT INTO image_edits (id, prompt, model, size, n, has_mask, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                eid,
                prompt,
                model,
                size,
                n as i64,
                body.mask.is_some() as i64,
                created,
            ],
        )?;
        Ok(())
    })
    .await;

    if let Err(e) = store_result {
        tracing::warn!(error = %e, "Failed to store image edit metadata");
    }

    // Return placeholder edited images.
    let data: Vec<ImageData> = (0..body.n)
        .map(|i| {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="100%" height="100%" fill="#16213e"/><text x="50%" y="50%" text-anchor="middle" dy=".3em" fill="#e0e0e0" font-family="monospace" font-size="14">edited: {prompt_short}</text></svg>"##,
                w = body.size.split('x').next().unwrap_or("1024"),
                h = body.size.split('x').nth(1).unwrap_or("1024"),
                prompt_short = if body.prompt.len() > 35 {
                    format!("{}...", &body.prompt[..32])
                } else {
                    body.prompt.clone()
                },
            );
            let encoded = STANDARD.encode(svg.as_bytes());
            if body.response_format == "b64_json" {
                ImageData {
                    url: None,
                    b64_json: Some(encoded),
                    revised_prompt: Some(format!(
                        "Edited: {} (image {}/{})",
                        body.prompt,
                        i + 1,
                        body.n
                    )),
                }
            } else {
                ImageData {
                    url: Some(format!("data:image/svg+xml;base64,{encoded}")),
                    b64_json: None,
                    revised_prompt: Some(format!(
                        "Edited: {} (image {}/{})",
                        body.prompt,
                        i + 1,
                        body.n
                    )),
                }
            }
        })
        .collect();

    (StatusCode::OK, Json(json!({
        "created": created,
        "data": data,
    })))
        .into_response()
}

/// `POST /v1/images/variations` — generate variations of an existing image.
pub async fn create_image_variations(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Json(body): Json<ImageVariationsRequest>,
) -> Response {
    if let Some(err) =
        fake_provider_gate(std::env::var(ALLOW_FAKE_PROVIDERS_ENV).ok().as_deref(), "images")
    {
        return err.into_response();
    }
    if let Err(e) = validate_n(body.n) {
        return e.into_response();
    }
    if let Err(e) = validate_size(&body.size) {
        return e.into_response();
    }
    if let Err(e) = validate_response_format(&body.response_format) {
        return e.into_response();
    }

    if STANDARD.decode(&body.image).is_err() {
        return OpenAiErrorResponse::invalid_request(
            "`image` must be valid base64-encoded image data.",
            Some("image"),
        )
        .into_response();
    }

    let created = chrono::Utc::now().timestamp();

    // Return placeholder variations.
    let data: Vec<ImageData> = (0..body.n)
        .map(|i| {
            let hue = 200 + (i * 15) % 160;
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="100%" height="100%" fill="hsl({hue},40%,20%)"/><text x="50%" y="50%" text-anchor="middle" dy=".3em" fill="#e0e0e0" font-family="monospace" font-size="14">variation {idx}</text></svg>"##,
                w = body.size.split('x').next().unwrap_or("1024"),
                h = body.size.split('x').nth(1).unwrap_or("1024"),
                idx = i + 1,
            );
            let encoded = STANDARD.encode(svg.as_bytes());
            if body.response_format == "b64_json" {
                ImageData {
                    url: None,
                    b64_json: Some(encoded),
                    revised_prompt: Some(format!("Variation {}/{}", i + 1, body.n)),
                }
            } else {
                ImageData {
                    url: Some(format!("data:image/svg+xml;base64,{encoded}")),
                    b64_json: None,
                    revised_prompt: Some(format!("Variation {}/{}", i + 1, body.n)),
                }
            }
        })
        .collect();

    let _ = state; // suppress unused warning

    (StatusCode::OK, Json(json!({
        "created": created,
        "data": data,
    })))
        .into_response()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_size() {
        assert!(validate_size("1024x1024").is_ok());
        assert!(validate_size("512x512").is_ok());
        assert!(validate_size("999x999").is_err());
    }

    #[test]
    fn validates_n() {
        assert!(validate_n(1).is_ok());
        assert!(validate_n(10).is_ok());
        assert!(validate_n(0).is_err());
        assert!(validate_n(11).is_err());
    }

    #[test]
    fn validates_response_format() {
        assert!(validate_response_format("url").is_ok());
        assert!(validate_response_format("b64_json").is_ok());
        assert!(validate_response_format("raw").is_err());
    }

    #[test]
    fn fake_provider_gate_blocks_without_escape_hatch() {
        let err = fake_provider_gate(None, "images").expect("must block when env is unset");
        assert_eq!(err.status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(err.error.code.as_deref(), Some(error_code::NOT_CONFIGURED));
        assert!(
            err.error.message.contains("no images provider configured"),
            "detail must name the provider kind: {}",
            err.error.message
        );
    }

    #[test]
    fn fake_provider_gate_allows_with_escape_hatch() {
        assert!(fake_provider_gate(Some("1"), "images").is_none());
        assert!(fake_provider_gate(Some("true"), "images").is_some());
    }
}
