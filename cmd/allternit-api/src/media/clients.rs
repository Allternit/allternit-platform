//! Provider protocol clients for the media plane.
//!
//! All HTTP goes through [`MediaTransport`] so tests can script responses
//! without touching the network. Errors are plain `String` messages, matching
//! this crate's gateway client pattern.

use serde_json::{json, Value};

const MINIMAX_DEFAULT_BASE: &str = "https://api.minimax.io";
const FAL_QUEUE_BASE: &str = "https://queue.fal.run";
const FAL_RUN_BASE: &str = "https://fal.run";
const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com";

#[async_trait::async_trait]
pub trait MediaTransport: Send + Sync {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<(u16, Value), String>;
    async fn get_json(&self, url: &str, headers: &[(&str, &str)])
        -> Result<(u16, Value), String>;
    /// status, body bytes, content-type header value
    async fn get_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>, String), String>;
}

/// Live reqwest transport. JSON calls time out at 30s; media downloads at
/// 120s (provider CDNs can be slow for multi-hundred-MB MP4s).
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self { client }
    }
}

#[async_trait::async_trait]
impl MediaTransport for ReqwestTransport {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<(u16, Value), String> {
        let mut req = self.client.post(url).json(body);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let res = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = res.status().as_u16();
        let value: Value = res
            .json()
            .await
            .map_err(|e| format!("invalid JSON response: {e}"))?;
        Ok((status, value))
    }

    async fn get_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Value), String> {
        let mut req = self.client.get(url);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let res = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = res.status().as_u16();
        let value: Value = res
            .json()
            .await
            .map_err(|e| format!("invalid JSON response: {e}"))?;
        Ok((status, value))
    }

    async fn get_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>, String), String> {
        let mut req = self
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(120));
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let res = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = res.status().as_u16();
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = res.bytes().await.map_err(|e| format!("download failed: {e}"))?;
        Ok((status, bytes.to_vec(), content_type))
    }
}

/// Decrypted provider credentials for one request (BYOK row or platform env).
#[derive(Debug, Clone)]
pub struct ProviderKey {
    pub api_key: String,
    pub base_url: Option<String>,
}

impl ProviderKey {
    fn base_or<'a>(&'a self, default: &'a str) -> &'a str {
        self.base_url
            .as_deref()
            .map(|s| s.trim_end_matches('/'))
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
    }
}

/// Video generation backends supported by the async job pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoBackend {
    MinimaxH3,
    FalSeedance,
}

impl VideoBackend {
    pub fn from_provider_id(id: &str) -> Option<Self> {
        match id {
            "minimax-h3" => Some(Self::MinimaxH3),
            "fal-seedance" => Some(Self::FalSeedance),
            _ => None,
        }
    }

    pub fn provider_id(&self) -> &'static str {
        match self {
            Self::MinimaxH3 => "minimax-h3",
            Self::FalSeedance => "fal-seedance",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoJobSubmit {
    pub prompt: String,
    pub duration_secs: u32,
    pub resolution: String,
    pub aspect_ratio: String,
    pub image_url: Option<String>,
    /// fal tier: fast queue endpoint vs standard.
    pub fast: bool,
}

/// Provider-side handle for an in-flight video job. MiniMax needs only the
/// task id (poll URL derived); fal hands back absolute status/response URLs.
#[derive(Debug, Clone)]
pub struct VideoJobHandle {
    pub provider_task_id: String,
    pub poll_status_url: Option<String>,
    pub poll_response_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoPoll {
    Queued,
    Processing,
    Succeeded { video_url: String },
    Failed { message: String },
}

/// A single generated image reference (remote URL or inline base64).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageEntry {
    Url { url: String, width: Option<u64>, height: Option<u64>, content_type: Option<String> },
    B64 { b64_json: String },
}

/// Submit a video generation job to the provider. Returns the provider-side
/// handle plus a `model` string to persist on the job row.
pub async fn submit_video(
    transport: &dyn MediaTransport,
    backend: VideoBackend,
    key: &ProviderKey,
    submit: &VideoJobSubmit,
) -> Result<VideoJobHandle, String> {
    match backend {
        VideoBackend::MinimaxH3 => minimax_submit(transport, key, submit).await,
        VideoBackend::FalSeedance => fal_submit_video(transport, key, submit).await,
    }
}

pub async fn poll_video(
    transport: &dyn MediaTransport,
    backend: VideoBackend,
    key: &ProviderKey,
    handle: &VideoJobHandle,
) -> Result<VideoPoll, String> {
    match backend {
        VideoBackend::MinimaxH3 => minimax_poll(transport, key, handle).await,
        VideoBackend::FalSeedance => fal_poll_video(transport, key, handle).await,
    }
}

fn bearer(api_key: &str) -> Vec<(&'static str, String)> {
    vec![("Authorization", format!("Bearer {api_key}"))]
}

fn fal_key_header(api_key: &str) -> Vec<(&'static str, String)> {
    vec![("Authorization", format!("Key {api_key}"))]
}

fn header_refs<'a>(headers: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    headers.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

fn require_ok(status: u16, body: &Value, what: &str) -> Result<(), String> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!(
            "{what} failed with status {status}: {}",
            body.get("message")
                .or_else(|| body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or(&body.to_string())
        ))
    }
}

fn value_to_message(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(map) => map
            .get("message")
            .or_else(|| map.get("error"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    }
}

// ─── MiniMax H3 (V2 API, verified against platform.minimax.io docs 2026-08) ──

async fn minimax_submit(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    submit: &VideoJobSubmit,
) -> Result<VideoJobHandle, String> {
    let base = key.base_or(MINIMAX_DEFAULT_BASE);
    let mut content = vec![json!({"type": "text", "text": submit.prompt})];
    let mut ratio = submit.aspect_ratio.clone();
    if let Some(image_url) = &submit.image_url {
        content.push(json!({
            "type": "image_url",
            "image_url": {"url": image_url},
            "role": "first_frame",
        }));
        // i2v requires the adaptive ratio — the frame dictates the aspect.
        ratio = "adaptive".to_string();
    }
    let body = json!({
        "model": "MiniMax-H3",
        "content": content,
        "duration": submit.duration_secs,
        "resolution": submit.resolution,
        "ratio": ratio,
    });
    let headers = bearer(&key.api_key);
    let (status, res) = transport
        .post_json(
            &format!("{base}/v2/video_generation"),
            &header_refs(&headers),
            &body,
        )
        .await?;
    require_ok(status, &res, "MiniMax video submit")?;
    let task_id = res
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("MiniMax submit response missing task_id: {res}"))?;
    Ok(VideoJobHandle {
        provider_task_id: task_id.to_string(),
        poll_status_url: None,
        poll_response_url: None,
    })
}

async fn minimax_poll(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    handle: &VideoJobHandle,
) -> Result<VideoPoll, String> {
    let base = key.base_or(MINIMAX_DEFAULT_BASE);
    let headers = bearer(&key.api_key);
    let (status, res) = transport
        .get_json(
            &format!(
                "{base}/v2/query/video_generation/{}",
                handle.provider_task_id
            ),
            &header_refs(&headers),
        )
        .await?;
    require_ok(status, &res, "MiniMax video poll")?;
    let task = res
        .get("task")
        .ok_or_else(|| format!("MiniMax poll response missing task: {res}"))?;
    match task.get("status").and_then(|v| v.as_str()).unwrap_or("") {
        // Non-terminal MiniMax statuses are capitalized.
        "Preparing" | "Queueing" => Ok(VideoPoll::Queued),
        "Processing" => Ok(VideoPoll::Processing),
        "succeeded" => {
            let video_url = task
                .get("content")
                .and_then(|c| c.get("url"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("MiniMax succeeded without content.url: {res}"))?;
            Ok(VideoPoll::Succeeded {
                video_url: video_url.to_string(),
            })
        }
        "failed" | "cancelled" => {
            let message = task
                .get("error")
                .map(value_to_message)
                .unwrap_or_else(|| "MiniMax task failed".to_string());
            Ok(VideoPoll::Failed { message })
        }
        other => Err(format!("MiniMax returned unknown task status: {other}")),
    }
}

// ─── fal (queue API for video, direct run for sync FLUX) ─────────────────────
// Prices from the fal.ai model pages (see catalog.rs).

async fn fal_submit_video(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    submit: &VideoJobSubmit,
) -> Result<VideoJobHandle, String> {
    let tier = if submit.fast { "fast" } else { "standard" };
    // Seedance i2v is a separate endpoint family (image_url is an extra field
    // on the i2v input, not accepted by the t2v endpoint).
    let mode = if submit.image_url.is_some() { "image-to-video" } else { "text-to-video" };
    let endpoint =
        format!("{FAL_QUEUE_BASE}/bytedance/seedance-2.0/{tier}/{mode}");
    let mut body = json!({
        "prompt": submit.prompt,
        "resolution": submit.resolution,
        // fal takes duration as a string ("5", "auto", …).
        "duration": submit.duration_secs.to_string(),
        "aspect_ratio": submit.aspect_ratio,
        "generate_audio": true,
    });
    if let Some(image_url) = &submit.image_url {
        body["image_url"] = json!(image_url);
    }
    let headers = fal_key_header(&key.api_key);
    let (status, res) = transport
        .post_json(&endpoint, &header_refs(&headers), &body)
        .await?;
    require_ok(status, &res, "fal video submit")?;
    let request_id = res
        .get("request_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("fal submit response missing request_id: {res}"))?;
    let status_url = res
        .get("status_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let response_url = res
        .get("response_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if status_url.is_none() || response_url.is_none() {
        return Err(format!("fal submit response missing status/response URLs: {res}"));
    }
    Ok(VideoJobHandle {
        provider_task_id: request_id.to_string(),
        poll_status_url: status_url,
        poll_response_url: response_url,
    })
}

async fn fal_poll_video(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    handle: &VideoJobHandle,
) -> Result<VideoPoll, String> {
    let status_url = handle
        .poll_status_url
        .clone()
        .ok_or_else(|| "fal handle missing status_url".to_string())?;
    let response_url = handle
        .poll_response_url
        .clone()
        .ok_or_else(|| "fal handle missing response_url".to_string())?;
    let headers = fal_key_header(&key.api_key);
    let (status, res) = transport
        .get_json(&status_url, &header_refs(&headers))
        .await?;
    require_ok(status, &res, "fal video poll")?;
    match res.get("status").and_then(|v| v.as_str()).unwrap_or("") {
        "IN_QUEUE" => Ok(VideoPoll::Queued),
        "IN_PROGRESS" => Ok(VideoPoll::Processing),
        "COMPLETED" => {
            // fal reports application errors as COMPLETED + error payload.
            if let Some(error) = res.get("error") {
                return Ok(VideoPoll::Failed {
                    message: value_to_message(error),
                });
            }
            let (status, res) = transport
                .get_json(&response_url, &header_refs(&headers))
                .await?;
            require_ok(status, &res, "fal video result")?;
            let video_url = res
                .get("video")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("fal result missing video.url: {res}"))?;
            Ok(VideoPoll::Succeeded {
                video_url: video_url.to_string(),
            })
        }
        other => {
            let message = res
                .get("error")
                .map(value_to_message)
                .unwrap_or_else(|| format!("fal queue status: {other}"));
            Ok(VideoPoll::Failed { message })
        }
    }
}

/// Synchronous FLUX schnell image generation (fast enough for a blocking
/// call per fal docs; $0.003/megapixel, rounded up to the nearest MP).
pub async fn generate_flux_images(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    prompt: &str,
    num_images: u32,
    seed: Option<u64>,
) -> Result<Vec<ImageEntry>, String> {
    let mut body = json!({
        "prompt": prompt,
        "image_size": "square_hd",
        "num_images": num_images,
    });
    if let Some(seed) = seed {
        body["seed"] = json!(seed);
    }
    let headers = fal_key_header(&key.api_key);
    let (status, res) = transport
        .post_json(
            &format!("{FAL_RUN_BASE}/fal-ai/flux/schnell"),
            &header_refs(&headers),
            &body,
        )
        .await?;
    require_ok(status, &res, "fal FLUX generate")?;
    extract_fal_images(&res)
}

fn extract_fal_images(res: &Value) -> Result<Vec<ImageEntry>, String> {
    let images = res
        .get("images")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("fal response missing images: {res}"))?;
    Ok(images
        .iter()
        .map(|img| ImageEntry::Url {
            url: img
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            width: img.get("width").and_then(|v| v.as_u64()),
            height: img.get("height").and_then(|v| v.as_u64()),
            content_type: img
                .get("content_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
        .collect())
}

// ─── OpenAI-compatible images (gpt-image) ────────────────────────────────────

/// gpt-image-2 generation via `{base}/v1/images/generations`.
/// Prices per 1024² image: low $0.006 / medium $0.053 / high $0.211
/// (Batch API is 50% off — not used here since this is interactive).
pub async fn generate_gpt_images(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    prompt: &str,
    size: &str,
    quality: &str,
    n: u32,
) -> Result<Vec<ImageEntry>, String> {
    let base = key.base_or(OPENAI_DEFAULT_BASE);
    let body = json!({
        "model": "gpt-image-2",
        "prompt": prompt,
        "size": size,
        "quality": quality,
        "n": n,
    });
    let headers = bearer(&key.api_key);
    let (status, res) = transport
        .post_json(
            &format!("{base}/v1/images/generations"),
            &header_refs(&headers),
            &body,
        )
        .await?;
    require_ok(status, &res, "gpt-image generate")?;
    extract_gpt_images(&res)
}

/// gpt-image-2 restyle of a reference photo via `{base}/v1/images/edits`
/// (JSON body, `images: [{image_url}]` with a base64 data URL). Priced per
/// output image like generation, plus input-image tokens.
pub async fn edit_gpt_images(
    transport: &dyn MediaTransport,
    key: &ProviderKey,
    prompt: &str,
    reference_data_url: &str,
    size: &str,
    quality: &str,
    n: u32,
) -> Result<Vec<ImageEntry>, String> {
    let base = key.base_or(OPENAI_DEFAULT_BASE);
    let body = json!({
        "model": "gpt-image-2",
        "prompt": prompt,
        "images": [{ "image_url": reference_data_url }],
        "size": size,
        "quality": quality,
        "n": n,
    });
    let headers = bearer(&key.api_key);
    let (status, res) = transport
        .post_json(
            &format!("{base}/v1/images/edits"),
            &header_refs(&headers),
            &body,
        )
        .await?;
    require_ok(status, &res, "gpt-image edit")?;
    extract_gpt_images(&res)
}

fn extract_gpt_images(res: &Value) -> Result<Vec<ImageEntry>, String> {
    let data = res
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("gpt-image response missing data: {res}"))?;
    let mut out = Vec::new();
    for entry in data {
        if let Some(b64) = entry.get("b64_json").and_then(|v| v.as_str()) {
            out.push(ImageEntry::B64 {
                b64_json: b64.to_string(),
            });
        } else if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
            out.push(ImageEntry::Url {
                url: url.to_string(),
                width: None,
                height: None,
                content_type: None,
            });
        }
    }
    if out.is_empty() {
        return Err(format!("gpt-image response had no image entries: {res}"));
    }
    Ok(out)
}
