//! HAR-derived API capture routes.
//!
//! Accepts browser HAR archives, extracts repeatable API calls, persists them
//! in SQLite, and exposes server-side replay + client generation so the Site
//! APIs surface works across web, desktop, and extension contexts.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::get_user;
use crate::AppState;

pub fn har_api_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/har-derived-api/ingest", post(ingest_har))
        .route("/har-derived-api/contracts", get(list_contracts))
        .route("/har-derived-api/contracts/:contract_id", get(get_contract))
        .route("/har-derived-api/contracts/:contract_id", delete(delete_contract))
        .route("/har-derived-api/contracts/:contract_id/replay/:endpoint_id", post(replay_endpoint))
        .route("/har-derived-api/client", post(generate_client))
        .merge(crate::teach_routes::teach_router())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "Unauthorized" })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct HarArchive {
    log: HarLog,
}

#[derive(Debug, Deserialize)]
struct HarLog {
    entries: Vec<HarEntry>,
}

#[derive(Debug, Deserialize)]
struct HarEntry {
    request: HarRequest,
    response: HarResponse,
    #[serde(default)]
    time: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct HarRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<HarHeader>,
    #[serde(default)]
    query_string: Vec<HarParam>,
    #[serde(default)]
    post_data: Option<HarPostData>,
}

#[derive(Debug, Deserialize)]
struct HarResponse {
    status: u16,
    #[serde(default)]
    content: Option<HarContent>,
}

#[derive(Debug, Deserialize)]
struct HarHeader {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct HarParam {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct HarPostData {
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HarContent {
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApiEndpoint {
    id: String,
    method: String,
    url: String,
    host: String,
    path: String,
    path_template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    query_params: Vec<TemplatedParam>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    path_params: Vec<TemplatedParam>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    headers: Vec<TemplatedParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    body_params: Vec<TemplatedParam>,
    status_code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_sample: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hit_count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct TemplatedParam {
    name: String,
    value: String,
    templated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_default: Option<String>,
}

#[derive(Debug, Serialize)]
struct IngestResponse {
    contract_id: String,
    domain: String,
    endpoints: Vec<ApiEndpoint>,
    stats: IngestStats,
}

#[derive(Debug, Serialize)]
struct IngestStats {
    total_entries: usize,
    api_entries: usize,
    hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GenerateClientRequest {
    endpoints: Vec<String>,
    language: String,
    #[serde(default)]
    include_auth: bool,
}

#[derive(Debug, Serialize)]
struct GenerateClientResponse {
    language: String,
    code: String,
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IngestHarRequest {
    har: String,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Deserialize)]
struct ReplayRequest {
    #[serde(default)]
    path_params: HashMap<String, String>,
    #[serde(default)]
    query_params: HashMap<String, String>,
    #[serde(default)]
    headers: Vec<HeaderInput>,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct HeaderInput {
    name: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct ReplayResponse {
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn ingest_har(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<IngestHarRequest>) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let archive: HarArchive = match serde_json::from_str(&req.har) {
        Ok(a) => a,
        Err(err) => {
            warn!(error = %err, "Failed to parse HAR JSON");
            return bad_request(&format!("Invalid HAR JSON: {}", err));
        }
    };

    let endpoints = extract_endpoints(&archive.log.entries);
    let hosts = collect_hosts(&endpoints);
    let domain = hosts.first().cloned().unwrap_or_else(|| "unknown".to_string());

    info!(
        total = archive.log.entries.len(),
        extracted = endpoints.len(),
        user_id = %user.user_id,
        "HAR-derived API ingest complete"
    );

    let contract_id = format!("contract-{}", uuid::Uuid::new_v4());
    let derived_at = chrono::Utc::now().to_rfc3339();
    let source = if req.source.is_empty() { "upload".to_string() } else { req.source.clone() };

    if let Err(err) = state.db.create_contract(&contract_id, &user.user_id, &domain, &source, &derived_at) {
        warn!(error = %err, "Failed to create API capture contract");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to persist contract" })),
        )
            .into_response();
    }

    for ep in &endpoints {
        let endpoint_id = format!("endpoint-{}", uuid::Uuid::new_v4());
        if let Err(err) = state.db.create_endpoint(
            &endpoint_id,
            &contract_id,
            &ep.method,
            &ep.url,
            &ep.host,
            &ep.path,
            &ep.path_template,
            ep.summary.as_deref(),
            &serde_json::to_string(&ep.query_params).unwrap_or_else(|_| "[]".to_string()),
            &serde_json::to_string(&ep.path_params).unwrap_or_else(|_| "[]".to_string()),
            &serde_json::to_string(&ep.headers).unwrap_or_else(|_| "[]".to_string()),
            ep.body_template.as_deref(),
            ep.body_mime_type.as_deref(),
            &serde_json::to_string(&ep.body_params).unwrap_or_else(|_| "[]".to_string()),
            ep.status_code,
            ep.response_sample.as_deref(),
            ep.hit_count.unwrap_or(1),
        ) {
            warn!(error = %err, "Failed to create API capture endpoint");
        }
    }

    (
        StatusCode::OK,
        Json(IngestResponse {
            contract_id,
            domain,
            stats: IngestStats {
                total_entries: archive.log.entries.len(),
                api_entries: endpoints.len(),
                hosts,
            },
            endpoints,
        }),
    )
        .into_response()
}

async fn list_contracts(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    match state.db.list_capture_contracts(&user.user_id) {
        Ok(contracts) => (StatusCode::OK, Json(json!({ "contracts": contracts }))).into_response(),
        Err(err) => {
            warn!(error = %err, "Failed to list API capture contracts");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to list contracts" })),
            )
                .into_response()
        }
    }
}

async fn get_contract(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(contract_id): Path<String>,
) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    match state.db.get_contract_with_endpoints(&contract_id) {
        Ok(Some((contract, endpoints))) if contract.user_id == user.user_id => {
            (StatusCode::OK, Json(serde_json::json!({
                "id": contract.id,
                "domain": contract.domain,
                "source": contract.source,
                "derived_at": contract.derived_at,
                "endpoints": endpoints,
            }))).into_response()
        }
        Ok(Some(_)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Contract not found" }))).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Contract not found" }))).into_response(),
        Err(err) => {
            warn!(error = %err, "Failed to get API capture contract");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to get contract" })),
            )
                .into_response()
        }
    }
}

async fn delete_contract(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(contract_id): Path<String>,
) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    match state.db.delete_capture_contract(&contract_id, &user.user_id) {
        Ok(true) => (StatusCode::NO_CONTENT, ()).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Contract not found" }))).into_response(),
        Err(err) => {
            warn!(error = %err, "Failed to delete API capture contract");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to delete contract" })),
            )
                .into_response()
        }
    }
}

async fn replay_endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((contract_id, endpoint_id)): Path<(String, String)>,
    Json(req): Json<ReplayRequest>,
) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Verify the endpoint belongs to a contract owned by the user.
    let endpoint = match state.db.get_endpoint_by_id(&endpoint_id) {
        Ok(Some(ep)) => serde_json::to_value(ep).unwrap_or_else(|_| json!({})),
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "Endpoint not found" }))).into_response(),
        Err(err) => {
            warn!(error = %err, "Failed to get API capture endpoint");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to get endpoint" })),
            )
                .into_response();
        }
    };

    if endpoint["contract_id"].as_str() != Some(&contract_id) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "Endpoint not found in contract" }))).into_response();
    }

    let method = endpoint["method"].as_str().unwrap_or("GET").to_uppercase();
    let url = endpoint["url"].as_str().unwrap_or("");
    let path_template = endpoint["path_template"].as_str().unwrap_or(url);

    let mut path = path_template.to_string();
    for (key, value) in &req.path_params {
        path = path.replace(&format!("{{{}}}", key), &urlencoding::encode(value));
    }

    let base_url = match reqwest::Url::parse(url) {
        Ok(u) => format!("{}://{}{}", u.scheme(), u.host_str().unwrap_or(""), u.port().map(|p| format!(":{}", p)).unwrap_or_default()),
        Err(_) => return bad_request("Invalid endpoint URL"),
    };

    let mut replay_url = match reqwest::Url::parse(&format!("{}{}", base_url, path)) {
        Ok(u) => u,
        Err(err) => return bad_request(&format!("Invalid replay URL: {}", err)),
    };

    for (key, value) in &req.query_params {
        replay_url.query_pairs_mut().append_pair(key, value);
    }

    let mut request_headers = reqwest::header::HeaderMap::new();
    let mut content_type_set = false;

    // Rebuild headers from the recorded endpoint, skipping templated/sensitive ones.
    if let Some(headers) = endpoint["headers"].as_array() {
        for h in headers {
            if let (Some(name), Some(value)) = (h["name"].as_str(), h["value"].as_str()) {
                let lower = name.to_ascii_lowercase();
                if is_hop_by_hop_or_sensitive(name) || h["templated"].as_bool().unwrap_or(false) {
                    continue;
                }
                if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
                    if lower == "content-type" {
                        content_type_set = true;
                    }
                    if let Ok(header_value) = reqwest::header::HeaderValue::from_str(value) {
                        request_headers.insert(header_name, header_value);
                    }
                }
            }
        }
    }

    // Apply user-supplied headers (can override recorded ones).
    for h in &req.headers {
        if let Ok(header_name) = HeaderName::from_bytes(h.name.as_bytes()) {
            if let Ok(header_value) = reqwest::header::HeaderValue::from_str(&h.value) {
                if h.name.to_ascii_lowercase() == "content-type" {
                    content_type_set = true;
                }
                request_headers.insert(header_name, header_value);
            }
        }
    }

    let body_mime_type = endpoint["body_mime_type"].as_str().unwrap_or("application/json");
    if !content_type_set && method != "GET" && method != "HEAD" {
        if let Ok(ct) = reqwest::header::HeaderValue::from_str(body_mime_type) {
            request_headers.insert(reqwest::header::CONTENT_TYPE, ct);
        }
    }

    let body_text = if let Some(body) = &req.body {
        match serde_json::to_string(body) {
            Ok(text) => Some(text),
            Err(err) => return bad_request(&format!("Invalid request body: {}", err)),
        }
    } else {
        endpoint["body_template"].as_str().map(|s| s.to_string())
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut request_builder = client.request(
        reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
        replay_url,
    );

    if let Some(body) = body_text {
        request_builder = request_builder.body(body);
    }

    match request_builder.headers(request_headers).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let body_text = match response.text().await {
                Ok(text) => text,
                Err(err) => {
                    return (StatusCode::OK, Json(ReplayResponse {
                        status,
                        body: None,
                        error: Some(format!("Response body read failed: {}", err)),
                    })).into_response();
                }
            };

            let body = if body_text.trim().is_empty() {
                None
            } else {
                match serde_json::from_str(&body_text) {
                    Ok(json) => Some(json),
                    Err(_) => Some(Value::String(body_text)),
                }
            };

            (StatusCode::OK, Json(ReplayResponse { status, body, error: None })).into_response()
        }
        Err(err) => {
            warn!(error = %err, "API capture replay request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(ReplayResponse {
                    status: 0,
                    body: None,
                    error: Some(err.to_string()),
                }),
            )
                .into_response()
        }
    }
}

async fn generate_client(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<GenerateClientRequest>) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Resolve the requested endpoints from persisted contracts.
    let mut endpoints: Vec<Value> = Vec::new();
    for endpoint_id in &req.endpoints {
        match state.db.get_endpoint_by_id(endpoint_id) {
            Ok(Some(ep)) => endpoints.push(serde_json::to_value(ep).unwrap_or_else(|_| json!({}))),
            Ok(None) => {}
            Err(err) => {
                warn!(error = %err, endpoint_id, "Failed to resolve endpoint for client generation");
            }
        }
    }

    if endpoints.is_empty() {
        return bad_request("No valid endpoints provided");
    }

    let code = match req.language.as_str() {
        "python" => generate_python_client(&endpoints, req.include_auth),
        "typescript" => generate_typescript_client(&endpoints, req.include_auth),
        "curl" => generate_curl_client(&endpoints, req.include_auth),
        _ => return bad_request("Unsupported language"),
    };

    let notes = vec![
        "Review extracted parameters and secrets before committing.".to_string(),
        "Replace hard-coded auth values with environment variables.".to_string(),
    ];

    (
        StatusCode::OK,
        Json(GenerateClientResponse {
            language: req.language,
            code,
            notes,
        }),
    )
        .into_response()
}

fn extract_endpoints(entries: &[HarEntry]) -> Vec<ApiEndpoint> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for entry in entries {
        // Skip non-API responses and errors.
        if entry.response.status >= 400 {
            continue;
        }

        let parsed = match reqwest::Url::parse(&entry.request.url) {
            Ok(u) => u,
            Err(_) => continue,
        };

        let host = parsed.host_str().unwrap_or("").to_string();
        let path = parsed.path().to_string();

        // Skip common static assets.
        if is_static_asset(&path) {
            continue;
        }

        let id = format!("{} {}", entry.request.method.to_uppercase(), entry.request.url);
        if seen.contains(&id) {
            continue;
        }
        seen.insert(id.clone());

        let query_params: Vec<TemplatedParam> = entry
            .request
            .query_string
            .iter()
            .map(|p| TemplatedParam {
                name: p.name.clone(),
                value: p.value.clone(),
                templated: is_likely_variable(&p.value),
                suggested_default: if is_likely_variable(&p.value) { Some(p.value.clone()) } else { None },
            })
            .collect();

        let (path_template, path_params) = extract_path_params(&path);

        let headers: Vec<TemplatedParam> = entry
            .request
            .headers
            .iter()
            .filter(|h| !is_hop_by_hop_or_sensitive(&h.name))
            .map(|h| TemplatedParam {
                name: h.name.clone(),
                value: h.value.clone(),
                templated: is_likely_secret(&h.name, &h.value),
                suggested_default: None,
            })
            .collect();

        let (body_template, body_mime_type, body_params) = entry
            .request
            .post_data
            .as_ref()
            .map(|pd| {
                let params = if pd.mime_type.as_deref() == Some("application/x-www-form-urlencoded") {
                    parse_form_body(pd.text.as_deref().unwrap_or(""))
                } else if pd.text.as_deref().map(|t| t.trim().starts_with('{')).unwrap_or(false) {
                    extract_json_template_params(pd.text.as_deref().unwrap_or(""))
                } else {
                    Vec::new()
                };
                (pd.text.clone(), pd.mime_type.clone(), params)
            })
            .unwrap_or((None, None, Vec::new()));

        let response_sample = entry
            .response
            .content
            .as_ref()
            .and_then(|c| c.text.clone())
            .map(|t| truncate(&t, 2000));

        result.push(ApiEndpoint {
            id,
            method: entry.request.method.to_uppercase(),
            url: entry.request.url.clone(),
            host,
            path: path.clone(),
            path_template,
            summary: None,
            query_params,
            path_params,
            headers,
            body_template,
            body_mime_type,
            body_params,
            status_code: entry.response.status,
            response_sample,
            hit_count: Some(1),
        });
    }

    result
}

fn is_static_asset(path: &str) -> bool {
    let lower = path.to_lowercase();
    [
        ".js", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico", ".woff", ".woff2",
        ".ttf", ".eot", ".otf", ".map", ".json", ".xml", ".webp",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn is_hop_by_hop_or_sensitive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "host", "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
        "te", "trailers", "transfer-encoding", "upgrade", "cookie", "set-cookie",
        "authorization", "x-api-key", "api-key",
    ]
    .contains(&lower.as_str())
}

fn is_likely_variable(value: &str) -> bool {
    value.len() > 8 || value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok()
}

fn is_likely_secret(name: &str, value: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("auth") || lower.contains("token") || lower.contains("key") || value.len() > 32
}

fn extract_path_params(path: &str) -> (String, Vec<TemplatedParam>) {
    let mut params = Vec::new();
    let mut template_parts: Vec<String> = Vec::new();
    let mut id_counter = 1;

    for segment in path.split('/') {
        let is_variable = !segment.is_empty()
            && (segment.parse::<i64>().is_ok()
                || (segment.len() >= 12 && segment.contains('-'))
                || uuid_like(segment));

        if is_variable {
            let name = if id_counter == 1 {
                "id".to_string()
            } else {
                format!("id{}", id_counter)
            };
            id_counter += 1;
            params.push(TemplatedParam {
                name: name.clone(),
                value: segment.to_string(),
                templated: true,
                suggested_default: Some(segment.to_string()),
            });
            template_parts.push(format!("{{{}}}", name));
        } else {
            template_parts.push(segment.to_string());
        }
    }

    (template_parts.join("/"), params)
}

fn uuid_like(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4
}

fn parse_form_body(text: &str) -> Vec<TemplatedParam> {
    text.split('&')
        .filter_map(|part| {
            let mut kv = part.splitn(2, '=');
            let name = kv.next()?.to_string();
            let raw_value = kv.next().unwrap_or("").to_string();
            let decoded = urlencoding::decode(&raw_value).map(|c| c.into_owned()).unwrap_or(raw_value.clone());
            Some(TemplatedParam {
                name,
                value: decoded,
                templated: is_likely_variable(&raw_value),
                suggested_default: None,
            })
        })
        .collect()
}

fn extract_json_template_params(text: &str) -> Vec<TemplatedParam> {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut params = Vec::new();
    collect_json_leaf_values(&value, "", &mut params);
    params
}

fn collect_json_leaf_values(value: &Value, prefix: &str, out: &mut Vec<TemplatedParam>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{}.{}", prefix, k) };
                collect_json_leaf_values(v, &key, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                collect_json_leaf_values(v, &format!("{}[{}]", prefix, i), out);
            }
        }
        Value::String(s) => {
            if is_likely_variable(s) {
                out.push(TemplatedParam {
                    name: prefix.to_string(),
                    value: s.clone(),
                    templated: true,
                    suggested_default: Some(s.clone()),
                });
            }
        }
        Value::Number(n) => {
            out.push(TemplatedParam {
                name: prefix.to_string(),
                value: n.to_string(),
                templated: true,
                suggested_default: Some(n.to_string()),
            });
        }
        _ => {}
    }
}

fn collect_hosts(endpoints: &[ApiEndpoint]) -> Vec<String> {
    let mut hosts: Vec<String> = endpoints.iter().map(|e| e.host.clone()).collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len])
    }
}

fn generate_python_client(endpoints: &[Value], include_auth: bool) -> String {
    let mut lines = vec![
        "import os".to_string(),
        "import requests".to_string(),
        "from urllib.parse import urlencode".to_string(),
        "".to_string(),
        "class SiteApiClient:".to_string(),
        "    def __init__(self, base_url=None, api_key=None):".to_string(),
        "        self.base_url = (base_url or os.environ.get('API_BASE_URL', '')).rstrip('/')".to_string(),
    ];

    if include_auth {
        lines.push("        self.api_key = api_key or os.environ.get('API_KEY')".to_string());
        lines.push("        self.session = requests.Session()".to_string());
        lines.push("        if self.api_key:".to_string());
        lines.push("            self.session.headers['Authorization'] = f'Bearer {self.api_key}'".to_string());
    } else {
        lines.push("        self.session = requests.Session()".to_string());
    }

    lines.push("".to_string());

    for ep in endpoints {
        let method = ep["method"].as_str().unwrap_or("GET").to_lowercase();
        let path_template = ep["path_template"].as_str().unwrap_or("");
        let host = ep["host"].as_str().unwrap_or("");
        let fn_name = sanitize_fn_name(&format!("{}_{}", method, path_template));

        let path_params = extract_path_param_names(path_template);
        let query_params: Vec<String> = ep["query_params"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|p| p["name"].as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let mut sig_parts = path_params.clone();
        if !query_params.is_empty() {
            sig_parts.push("params".to_string());
        }
        if method == "post" || method == "put" || method == "patch" {
            sig_parts.push("body".to_string());
        }

        lines.push(format!("    def {}(self, {}):", fn_name, sig_parts.join(", ")));
        lines.push(format!("        \"\"\"{} {}\"\"\"", ep["method"].as_str().unwrap_or("GET"), path_template));

        // Build URL
        let mut url_line = format!("        url = f'{}{}'", host, path_template);
        for param in &path_params {
            url_line = url_line.replace(&format!("{{{}}}", param), &format!("{{{{{}}}}}", param));
        }
        lines.push(url_line);

        if !query_params.is_empty() {
            lines.push("        if params:".to_string());
            lines.push("            url += '?' + urlencode(params)".to_string());
        }

        let mut call_args = format!("url, method='{}'", method.to_uppercase());
        if method == "post" || method == "put" || method == "patch" {
            call_args.push_str(", json=body");
        }

        lines.push(format!("        return self.session.request({}).json()", call_args));
        lines.push("".to_string());
    }

    lines.join("\n")
}

fn generate_typescript_client(endpoints: &[Value], include_auth: bool) -> String {
    let mut lines = vec![
        "export class SiteApiClient {".to_string(),
        "  private baseUrl: string;".to_string(),
    ];

    if include_auth {
        lines.push("  private apiKey?: string;".to_string());
        lines.push("".to_string());
        lines.push("  constructor(baseUrl = process.env.API_BASE_URL, apiKey = process.env.API_KEY) {".to_string());
        lines.push("    this.baseUrl = (baseUrl || '').replace(/\\/$/, '');".to_string());
        lines.push("    this.apiKey = apiKey;".to_string());
    } else {
        lines.push("".to_string());
        lines.push("  constructor(baseUrl = process.env.API_BASE_URL) {".to_string());
        lines.push("    this.baseUrl = (baseUrl || '').replace(/\\/$/, '');".to_string());
    }

    lines.push("  }".to_string());
    lines.push("".to_string());
    lines.push("  private async request(path: string, init: RequestInit = {}) {".to_string());
    lines.push("    const headers: Record<string, string> = { 'Content-Type': 'application/json' };".to_string());
    if include_auth {
        lines.push("    if (this.apiKey) headers['Authorization'] = `Bearer ${this.apiKey}`;".to_string());
    }
    lines.push("    const res = await fetch(`${this.baseUrl}${path}`, { ...init, headers });".to_string());
    lines.push("    if (!res.ok) throw new Error(`HTTP ${res.status}`);".to_string());
    lines.push("    return res.json();".to_string());
    lines.push("  }".to_string());
    lines.push("".to_string());

    for ep in endpoints {
        let method = ep["method"].as_str().unwrap_or("GET").to_lowercase();
        let path_template = ep["path_template"].as_str().unwrap_or("");
        let fn_name = sanitize_fn_name(&format!("{}_{}", method, path_template));

        let path_params = extract_path_param_names(path_template);
        let query_params: Vec<String> = ep["query_params"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|p| p["name"].as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let mut sig_parts: Vec<String> = path_params.iter().map(|p| format!("{}: string", p)).collect();
        if !query_params.is_empty() {
            sig_parts.push("params?: Record<string, string>".to_string());
        }
        if method == "post" || method == "put" || method == "patch" {
            sig_parts.push("body?: unknown".to_string());
        }

        lines.push(format!("  async {}({}) {{", fn_name, sig_parts.join(", ")));

        let mut path_expr = format!("`{}`", path_template);
        for param in &path_params {
            path_expr = path_expr.replace(&format!("{{{}}}", param), &format!("${{{}}}", param));
        }

        if !query_params.is_empty() {
            lines.push(format!("    const query = params ? '?' + new URLSearchParams(params).toString() : '';"));
            lines.push(format!("    return this.request(`${{{}}}${{query}}`, {{ method: '{}' }});", path_expr, method.to_uppercase()));
        } else if method == "post" || method == "put" || method == "patch" {
            lines.push(format!("    return this.request({}, {{ method: '{}', body: body ? JSON.stringify(body) : undefined }});", path_expr, method.to_uppercase()));
        } else {
            lines.push(format!("    return this.request({}, {{ method: '{}' }});", path_expr, method.to_uppercase()));
        }

        lines.push("  }".to_string());
        lines.push("".to_string());
    }

    lines.push("}".to_string());
    lines.join("\n")
}

fn generate_curl_client(endpoints: &[Value], include_auth: bool) -> String {
    let mut lines = vec![
        "# HAR-derived API replay templates".to_string(),
        "# Set these in your environment before running:".to_string(),
        "# export API_BASE_URL=\"...\"".to_string(),
    ];

    if include_auth {
        lines.push("# export API_KEY=\"...\"".to_string());
    }
    lines.push("".to_string());

    for ep in endpoints {
        let method = ep["method"].as_str().unwrap_or("GET");
        let path_template = ep["path_template"].as_str().unwrap_or("");
        let host = ep["host"].as_str().unwrap_or("");

        let mut cmd = format!("curl -s -X {} \"{}{}\"", method, host, path_template);

        if include_auth {
            cmd.push_str(" \\\n  -H \"Authorization: Bearer $API_KEY\"");
        }

        if let Some(body) = ep["body_template"].as_str() {
            cmd.push_str(&format!(" \\\n  -H \"Content-Type: application/json\" \\\n  -d '{}'", body.replace('\\', "\\\\").replace('"', "\\\"")));
        }

        lines.push(cmd);
        lines.push("".to_string());
    }

    lines.join("\n")
}

fn sanitize_fn_name(name: &str) -> String {
    name.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "_")
        .replace("__", "_")
        .trim_matches('_')
        .to_string()
}

fn extract_path_param_names(path_template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut chars = path_template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            while let Some(ch) = chars.next() {
                if ch == '}' {
                    break;
                }
                name.push(ch);
            }
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
}
