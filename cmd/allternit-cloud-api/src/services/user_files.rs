//! User file storage in the private R2 bucket `allternit-user-files`.
//!
//! Upload flow: `begin_upload` checks the plan caps and the global R2 guard and
//! presigns a PUT for the exact declared size; the client uploads straight to
//! R2; `complete_upload` HEAD-verifies the size and only then records the row.
//! Pending uploads leave no row (an unverified object is overwritten on retry).

use crate::error::ApiError;
use crate::services::r2::{ObjectStore, R2Error};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

pub const BUCKET: &str = "allternit-user-files";
const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// Per-plan storage caps: total across live files, and the largest single file.
pub struct PlanCaps {
    pub total_bytes: u64,
    pub file_bytes: u64,
}

pub fn caps_for_plan(plan_id: &str) -> PlanCaps {
    match plan_id {
        "plus" => PlanCaps { total_bytes: 2 * GB, file_bytes: 100 * MB },
        "super" | "ultra" => PlanCaps { total_bytes: 10 * GB, file_bytes: 500 * MB },
        _ => PlanCaps { total_bytes: 100 * MB, file_bytes: 10 * MB },
    }
}

/// New uploads are refused once all of R2 is at or above this, so the 10 GB
/// allowance cannot be exceeded.
pub const GLOBAL_R2_CEILING_BYTES: u64 = 9_500_000_000;
const USAGE_BUCKET: &str = "allternit-backups";
const USAGE_KEY: &str = "status/r2-usage.json";
pub const PUT_TTL: Duration = Duration::from_secs(15 * 60);
pub const GET_TTL: Duration = Duration::from_secs(10 * 60);
/// A caller may ask for a longer link (a message that carries the file), up to 7 days.
pub const MAX_GET_TTL_SECS: u64 = 7 * 24 * 3600;

/// A refusal the route turns into `{code, message}` with this status.
#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

fn refuse(status: u16, code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { status, code, message: message.into() }
}

#[derive(Debug)]
pub enum Failure {
    Refused(Refusal),
    Api(ApiError),
}
impl From<sqlx::Error> for Failure {
    fn from(e: sqlx::Error) -> Self {
        Failure::Api(e.into())
    }
}
impl From<ApiError> for Failure {
    fn from(e: ApiError) -> Self {
        Failure::Api(e)
    }
}
impl From<Refusal> for Failure {
    fn from(r: Refusal) -> Self {
        Failure::Refused(r)
    }
}

fn r2_failure(e: R2Error) -> Failure {
    match e {
        R2Error::Unavailable => refuse(503, "storage-unavailable", "File storage is not available right now.").into(),
        other => {
            tracing::warn!(error = %other, "user files: R2 call failed");
            refuse(502, "storage-error", "File storage did not answer. Try again.").into()
        }
    }
}

/// Keep only a safe file name: no path parts, no control characters, bounded.
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if matches!(c, '"' | '<' | '>' | ':' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() { "file".to_string() } else { cleaned }
}

pub fn file_key(user_id: &str, file_id: Uuid, name: &str) -> String {
    format!("u/{user_id}/{file_id}/{name}")
}

/// Total bytes of R2 in use from the daily `r2-usage.json`, written by the
/// R2 guard (`allternit-r2-guard.py`, after the nightly backup):
/// `{"total_gb": 4.4, "buckets": {"<name>": {"bytes": N, "objects": N}}, ...}`.
/// The per-bucket byte sum is exact; `total_gb` is the fallback.
/// `None` when the report is missing or unreadable.
pub fn parse_total_usage(raw: &[u8]) -> Option<u64> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    if let Some(buckets) = v.get("buckets").and_then(Value::as_object) {
        let sum: Option<u64> = buckets.values().map(|b| b.get("bytes").and_then(Value::as_u64)).sum();
        if sum.is_some() {
            return sum;
        }
    }
    v.get("total_gb").and_then(Value::as_f64).filter(|g| *g >= 0.0).map(|g| (g * 1e9) as u64)
}

pub async fn global_usage(store: &dyn ObjectStore) -> Option<u64> {
    match store.get(USAGE_BUCKET, USAGE_KEY).await {
        Ok(Some(raw)) => parse_total_usage(&raw),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "user files: could not read the R2 usage report");
            None
        }
    }
}

pub async fn used_bytes(db: &PgPool, user_id: &str) -> Result<u64, sqlx::Error> {
    let n: Option<i64> = sqlx::query_scalar("SELECT SUM(bytes)::bigint FROM user_files WHERE user_id = $1 AND deleted_at IS NULL")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    Ok(n.unwrap_or(0).max(0) as u64)
}

pub struct Upload {
    pub file_id: Uuid,
    pub key: String,
    pub put_url: String,
}

/// Plan caps (per file, total) and the global R2 guard for adding `bytes`.
async fn check_caps(db: &PgPool, store: &dyn ObjectStore, user_id: &str, plan_id: &str, bytes: u64) -> Result<(), Failure> {
    let caps = caps_for_plan(plan_id);
    if bytes > caps.file_bytes {
        return Err(refuse(
            413,
            "file-too-large",
            format!("This plan allows files up to {} MB.", caps.file_bytes / MB),
        )
        .into());
    }
    let used = used_bytes(db, user_id).await?;
    if used + bytes > caps.total_bytes {
        return Err(refuse(
            413,
            "storage-quota-exceeded",
            format!("This would pass the {} MB of file storage on your plan. Delete files or upgrade.", caps.total_bytes / MB),
        )
        .into());
    }
    if let Some(total) = global_usage(store).await {
        if total >= GLOBAL_R2_CEILING_BYTES {
            return Err(refuse(507, "storage-full", "File storage is full right now. Try again later.").into());
        }
    }
    Ok(())
}

pub async fn begin_upload(
    db: &PgPool,
    store: &dyn ObjectStore,
    user_id: &str,
    plan_id: &str,
    name: &str,
    content_type: &str,
    bytes: u64,
) -> Result<Upload, Failure> {
    if bytes == 0 {
        return Err(Failure::Api(ApiError::BadRequest("bytes must be greater than zero.".into())));
    }
    check_caps(db, store, user_id, plan_id, bytes).await?;
    let file_id = Uuid::new_v4();
    let name = safe_name(name);
    let key = file_key(user_id, file_id, &name);
    let ct = if content_type.trim().is_empty() { "application/octet-stream" } else { content_type.trim() };
    let put_url = store.presign_put(BUCKET, &key, PUT_TTL, ct, Some(bytes)).map_err(r2_failure)?;
    Ok(Upload { file_id, key, put_url })
}

/// Store bytes the server already holds (a channel attachment the runtime or a
/// platform hands us) under the same plan caps as a client upload. The row is
/// recorded only after the object is stored; a failed insert removes it again.
#[allow(clippy::too_many_arguments)]
pub async fn store_bytes(
    db: &PgPool,
    store: &dyn ObjectStore,
    user_id: &str,
    plan_id: &str,
    name: &str,
    content_type: &str,
    data: Vec<u8>,
    now: DateTime<Utc>,
) -> Result<Value, Failure> {
    let size = data.len() as u64;
    check_caps(db, store, user_id, plan_id, size).await?;
    let file_id = Uuid::new_v4();
    let name = safe_name(name);
    let key = file_key(user_id, file_id, &name);
    let ct = if content_type.trim().is_empty() { "application/octet-stream" } else { content_type.trim() };
    store.put(BUCKET, &key, data, ct).await.map_err(r2_failure)?;
    let inserted = sqlx::query("INSERT INTO user_files (id, user_id, key, name, content_type, bytes, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(file_id)
        .bind(user_id)
        .bind(&key)
        .bind(&name)
        .bind(ct)
        .bind(size as i64)
        .bind(now)
        .execute(db)
        .await;
    if let Err(e) = inserted {
        let _ = store.delete(BUCKET, &key).await;
        return Err(e.into());
    }
    Ok(json!({ "fileId": file_id, "name": name, "contentType": ct, "bytes": size, "createdAt": now, "linkUrl": capability_url(file_id) }))
}

/// Re-derives the key from the user + id + name the client sends back, so the
/// client never names an arbitrary object.
#[allow(clippy::too_many_arguments)]
pub async fn complete_upload(
    db: &PgPool,
    store: &dyn ObjectStore,
    user_id: &str,
    file_id: Uuid,
    name: &str,
    content_type: &str,
    declared: u64,
    now: DateTime<Utc>,
) -> Result<Value, Failure> {
    let name = safe_name(name);
    let key = file_key(user_id, file_id, &name);
    let actual = store.head(BUCKET, &key).await.map_err(r2_failure)?;
    match actual {
        None => return Err(refuse(409, "upload-missing", "The upload has not arrived.").into()),
        Some(n) if n != declared => {
            // Wrong size: discard the object rather than keep an unaccounted file.
            let _ = store.delete(BUCKET, &key).await;
            return Err(refuse(409, "size-mismatch", "The uploaded size did not match.").into());
        }
        Some(_) => {}
    }
    let ct = if content_type.trim().is_empty() { "application/octet-stream" } else { content_type.trim() };
    sqlx::query(
        "INSERT INTO user_files (id, user_id, key, name, content_type, bytes, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(file_id)
    .bind(user_id)
    .bind(&key)
    .bind(&name)
    .bind(ct)
    .bind(declared as i64)
    .bind(now)
    .execute(db)
    .await?;
    Ok(json!({ "fileId": file_id, "name": name, "contentType": ct, "bytes": declared, "createdAt": now, "linkUrl": capability_url(file_id) }))
}

async fn owned(db: &PgPool, user_id: &str, id: Uuid) -> Result<(String, String), Failure> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT key, name FROM user_files WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL")
            .bind(id)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    row.ok_or_else(|| refuse(404, "not-found", "No such file.").into())
}

pub async fn download_url(db: &PgPool, store: &dyn ObjectStore, user_id: &str, id: Uuid, expires: Option<u64>) -> Result<Value, Failure> {
    let (key, name) = owned(db, user_id, id).await?;
    let ttl = expires.map(|s| Duration::from_secs(s.clamp(60, MAX_GET_TTL_SECS))).unwrap_or(GET_TTL);
    let url = store.presign_get(BUCKET, &key, ttl).map_err(r2_failure)?;
    Ok(json!({ "fileId": id, "name": name, "url": url, "linkUrl": capability_url(id), "expiresInSeconds": ttl.as_secs() }))
}

/// Largest media item copied in from a carrier message.
pub const MAX_REMOTE_BYTES: u64 = 25 * MB;
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Fetches a remote file for [`ingest_remote`]; tests substitute a fake.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    /// Body bytes and content type; an error for anything over `max` bytes.
    async fn fetch(&self, url: &str, max: u64) -> Result<(Vec<u8>, String), String>;
}

pub struct HttpFetcher;

#[async_trait::async_trait]
impl Fetcher for HttpFetcher {
    async fn fetch(&self, url: &str, max: u64) -> Result<(Vec<u8>, String), String> {
        let u = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
        if u.scheme() != "https" {
            return Err("media url is not https".into());
        }
        let mut resp = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build().map_err(|e| e.to_string())?.get(u).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("media host answered {}", resp.status().as_u16()));
        }
        if resp.content_length().is_some_and(|n| n > max) {
            return Err("media item is too large".into());
        }
        let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let mut out = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if out.len() as u64 + chunk.len() as u64 > max {
                return Err("media item is too large".into());
            }
            out.extend_from_slice(&chunk);
        }
        Ok((out, ct))
    }
}

/// A remote file copied into the owner's cloud storage.
#[derive(Debug, Clone)]
pub struct Ingested {
    pub file_id: Uuid,
    pub name: String,
    pub content_type: String,
    pub bytes: u64,
    /// Permanent link; `None` when `ALLTERNIT_FILE_LINK_SECRET` is unset.
    pub link_url: Option<String>,
}

/// Copies `url` into `allternit-user-files` as `user_id` (plan caps apply, at most
/// [`MAX_REMOTE_BYTES`], content type kept). `Err` carries the reason so the caller
/// can keep the original URL and log it.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_remote(
    db: &PgPool,
    store: &dyn ObjectStore,
    fetcher: &dyn Fetcher,
    user_id: &str,
    plan_id: &str,
    url: &str,
    hint_content_type: &str,
    now: DateTime<Utc>,
) -> Result<Ingested, String> {
    let (data, fetched_ct) = fetcher.fetch(url, MAX_REMOTE_BYTES).await?;
    if data.is_empty() {
        return Err("media item is empty".into());
    }
    let bytes = data.len() as u64;
    let ct = [fetched_ct.trim(), hint_content_type.trim()].into_iter().find(|c| !c.is_empty()).unwrap_or("application/octet-stream").to_string();
    check_caps(db, store, user_id, plan_id, bytes).await.map_err(|f| match f {
        Failure::Refused(r) => format!("{} ({})", r.message, r.code),
        Failure::Api(e) => e.to_string(),
    })?;
    let raw_name = reqwest::Url::parse(url).ok().and_then(|u| u.path_segments().and_then(|s| s.last().map(str::to_string))).unwrap_or_default();
    let name = safe_name(if raw_name.is_empty() { "media" } else { &raw_name });
    let file_id = Uuid::new_v4();
    let key = file_key(user_id, file_id, &name);
    store.put(BUCKET, &key, data, &ct).await.map_err(|e| e.to_string())?;
    complete_upload(db, store, user_id, file_id, &name, &ct, bytes, now).await.map_err(|f| match f {
        Failure::Refused(r) => r.message,
        Failure::Api(e) => e.to_string(),
    })?;
    Ok(Ingested { file_id, name, content_type: ct, bytes, link_url: capability_url(file_id) })
}

const LINK_SECRET_ENV: &str = "ALLTERNIT_FILE_LINK_SECRET";
const DEFAULT_PUBLIC_API: &str = "https://api.allternit.com";

fn link_secret() -> Option<String> {
    std::env::var(LINK_SECRET_ENV).ok().filter(|s| !s.trim().is_empty())
}

/// `base64url(HMAC-SHA256(secret, fileId))`: the capability token in a file link.
pub fn link_token(secret: &str, file_id: Uuid) -> String {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let mut m = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    m.update(file_id.to_string().as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(m.finalize().into_bytes())
}

/// Constant-time check of a presented token.
pub fn verify_link_token(secret: &str, file_id: Uuid, token: &str) -> bool {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.trim_end_matches('=')) else {
        return false;
    };
    let mut m = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    m.update(file_id.to_string().as_bytes());
    m.verify_slice(&raw).is_ok()
}

fn capability_url_with(secret: Option<&str>, base: &str, file_id: Uuid) -> Option<String> {
    let secret = secret?;
    Some(format!("{}/api/v1/files/{}/raw?k={}", base.trim_end_matches('/'), file_id, link_token(secret, file_id)))
}

/// Whether permanent file links are configured (`ALLTERNIT_FILE_LINK_SECRET` set).
pub fn links_configured() -> bool {
    link_secret().is_some()
}

/// The permanent link for a file, or `None` when `ALLTERNIT_FILE_LINK_SECRET` is unset.
pub fn capability_url(file_id: Uuid) -> Option<String> {
    let base = std::env::var("ALLTERNIT_PUBLIC_API_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_PUBLIC_API.to_string());
    capability_url_with(link_secret().as_deref(), &base, file_id)
}

/// Resolves a capability link to a fresh 10-minute presigned GET. 503 when links
/// are not configured; 404 for a bad token or a deleted file (no distinction).
pub async fn raw_redirect_url(db: &PgPool, store: &dyn ObjectStore, id: Uuid, token: &str) -> Result<String, Failure> {
    raw_redirect_with(link_secret().as_deref(), db, store, id, token).await
}

async fn raw_redirect_with(secret: Option<&str>, db: &PgPool, store: &dyn ObjectStore, id: Uuid, token: &str) -> Result<String, Failure> {
    let Some(secret) = secret else {
        return Err(refuse(503, "file-links-unavailable", "Permanent file links are not configured.").into());
    };
    if !verify_link_token(secret, id, token) {
        return Err(refuse(404, "not-found", "No such file.").into());
    }
    let row: Option<(String,)> = sqlx::query_as("SELECT key FROM user_files WHERE id = $1 AND deleted_at IS NULL").bind(id).fetch_optional(db).await?;
    let (key,) = row.ok_or_else(|| Failure::from(refuse(404, "not-found", "No such file.")))?;
    store.presign_get(BUCKET, &key, GET_TTL).map_err(r2_failure)
}


/// Deletes the object first; the row is only marked deleted once R2 confirms,
/// so a failed delete can be retried and never leaves an uncounted object.
pub async fn delete_file(db: &PgPool, store: &dyn ObjectStore, user_id: &str, id: Uuid, now: DateTime<Utc>) -> Result<(), Failure> {
    let (key, _) = owned(db, user_id, id).await?;
    store.delete(BUCKET, &key).await.map_err(r2_failure)?;
    sqlx::query("UPDATE user_files SET deleted_at = $3 WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .bind(now)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn usage_json(db: &PgPool, user_id: &str, plan_id: &str) -> Result<Value, sqlx::Error> {
    let caps = caps_for_plan(plan_id);
    Ok(json!({ "plan": plan_id, "usedBytes": used_bytes(db, user_id).await?, "totalBytes": caps.total_bytes, "maxFileBytes": caps.file_bytes }))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn caps_follow_the_plans() {
        assert_eq!((caps_for_plan("free").total_bytes, caps_for_plan("free").file_bytes), (100 * MB, 10 * MB));
        assert_eq!((caps_for_plan("plus").total_bytes, caps_for_plan("plus").file_bytes), (2 * GB, 100 * MB));
        for p in ["super", "ultra"] {
            assert_eq!((caps_for_plan(p).total_bytes, caps_for_plan(p).file_bytes), (10 * GB, 500 * MB));
        }
        assert_eq!(caps_for_plan("unknown").file_bytes, 10 * MB);
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("a\\b\\c.txt"), "c.txt");
        assert_eq!(safe_name("  .hidden "), "hidden");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name("re:port?.pdf"), "re_port_.pdf");
    }

    #[test]
    fn usage_report_shapes() {
        // The guard's real shape: exact per-bucket bytes win over total_gb.
        let real = br#"{"at": "2026-10-04T03:40:00Z", "total_gb": 4.4, "warn_gb": 8.0, "allowance_gb": 10,
            "buckets": {"allternit-runtime": {"bytes": 3000000000, "objects": 20}, "allternit-backups": {"bytes": 1400000000, "objects": 40}}, "errors": {}}"#;
        assert_eq!(parse_total_usage(real), Some(4_400_000_000));
        assert_eq!(parse_total_usage(br#"{"total_gb": 9.6}"#), Some(9_600_000_000));
        assert_eq!(parse_total_usage(br#"{"x": 1}"#), None);
        assert_eq!(parse_total_usage(b"nope"), None);
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use async_trait::async_trait;
    use serial_test::serial;
    use std::{collections::HashMap, sync::Mutex, sync::Arc};

    #[derive(Default)]
    struct Fake {
        objects: Mutex<HashMap<String, u64>>,
        usage: Option<Vec<u8>>,
        last_put_len: Mutex<Option<u64>>,
    }
    #[async_trait]
    impl ObjectStore for Fake {
        fn presign_get(&self, b: &str, k: &str, _t: Duration) -> Result<String, R2Error> {
            Ok(format!("https://r2.test/{b}/{k}?get"))
        }
        fn presign_put(&self, b: &str, k: &str, _t: Duration, _c: &str, len: Option<u64>) -> Result<String, R2Error> {
            *self.last_put_len.lock().unwrap() = len;
            Ok(format!("https://r2.test/{b}/{k}?put"))
        }
        async fn head(&self, _b: &str, k: &str) -> Result<Option<u64>, R2Error> {
            Ok(self.objects.lock().unwrap().get(k).copied())
        }
        async fn put(&self, _b: &str, k: &str, bytes: Vec<u8>, _c: &str) -> Result<(), R2Error> {
            self.objects.lock().unwrap().insert(k.to_string(), bytes.len() as u64);
            Ok(())
        }
        async fn delete(&self, _b: &str, k: &str) -> Result<(), R2Error> {
            self.objects.lock().unwrap().remove(k);
            Ok(())
        }
        async fn get(&self, _b: &str, k: &str) -> Result<Option<Vec<u8>>, R2Error> {
            Ok(if k == USAGE_KEY { self.usage.clone() } else { None })
        }
    }

    async fn db() -> Arc<crate::ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/042_user_files.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .unwrap();
        state
    }

    async fn upload(state: &crate::ApiState, store: &Fake, user: &str, plan: &str, name: &str, bytes: u64) -> Result<Value, Failure> {
        let u = begin_upload(&state.db, store, user, plan, name, "text/plain", bytes).await?;
        assert!(u.key.starts_with(&format!("u/{user}/{}/", u.file_id)));
        store.objects.lock().unwrap().insert(u.key.clone(), bytes);
        complete_upload(&state.db, store, user, u.file_id, name, "text/plain", bytes, Utc::now()).await
    }

    fn refusal(r: Result<impl std::fmt::Debug, Failure>) -> (u16, &'static str) {
        match r {
            Err(Failure::Refused(r)) => (r.status, r.code),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    #[serial]
    async fn per_file_and_total_caps_per_plan_answer_413() {
        let state = db().await;
        let store = Fake::default();
        // Free: 10 MB per file.
        assert_eq!(refusal(begin_upload(&state.db, &store, "u1", "free", "a.bin", "", 10 * MB + 1).await.map(|_| ())), (413, "file-too-large"));
        // Free total is 100 MB: nine 10 MB files fit, the eleventh does not.
        for i in 0..10 {
            upload(&state, &store, "u1", "free", &format!("f{i}.bin"), 10 * MB).await.ok().unwrap();
        }
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 100 * MB);
        assert_eq!(refusal(begin_upload(&state.db, &store, "u1", "free", "x", "", 1).await.map(|_| ())), (413, "storage-quota-exceeded"));
        // Another user is unaffected, and Plus allows 100 MB files.
        assert!(begin_upload(&state.db, &store, "u2", "plus", "big", "", 100 * MB).await.is_ok());
        assert_eq!(refusal(begin_upload(&state.db, &store, "u2", "plus", "big", "", 100 * MB + 1).await.map(|_| ())), (413, "file-too-large"));
        assert!(begin_upload(&state.db, &store, "u3", "ultra", "huge", "", 500 * MB).await.is_ok());
        // The presigned PUT is signed for the exact size.
        assert_eq!(*store.last_put_len.lock().unwrap(), Some(500 * MB));
    }

    #[tokio::test]
    #[serial]
    async fn the_global_guard_answers_507_at_9_5_gb_and_not_below() {
        let state = db().await;
        let full = Fake { usage: Some(br#"{"total_gb": 9.5, "buckets": {"allternit-backups": {"bytes": 9500000000, "objects": 1}}}"#.to_vec()), ..Default::default() };
        assert_eq!(refusal(begin_upload(&state.db, &full, "u1", "ultra", "a", "", 5).await.map(|_| ())), (507, "storage-full"));
        let ok = Fake { usage: Some(br#"{"total_gb": 9.5, "buckets": {"allternit-backups": {"bytes": 9499999999, "objects": 1}}}"#.to_vec()), ..Default::default() };
        assert!(begin_upload(&state.db, &ok, "u1", "ultra", "a", "", 5).await.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn complete_verifies_the_size_and_records_nothing_on_mismatch() {
        let state = db().await;
        let store = Fake::default();
        let u = begin_upload(&state.db, &store, "u1", "free", "n.txt", "text/plain", 100).await.unwrap();
        // Nothing uploaded yet.
        assert_eq!(refusal(complete_upload(&state.db, &store, "u1", u.file_id, "n.txt", "text/plain", 100, Utc::now()).await), (409, "upload-missing"));
        // Uploaded a different size: refused, object discarded, no row.
        store.objects.lock().unwrap().insert(u.key.clone(), 99);
        assert_eq!(refusal(complete_upload(&state.db, &store, "u1", u.file_id, "n.txt", "text/plain", 100, Utc::now()).await), (409, "size-mismatch"));
        assert!(store.objects.lock().unwrap().is_empty());
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 0);
        // Another user cannot complete it onto their own account: the key is re-derived per user.
        store.objects.lock().unwrap().insert(u.key.clone(), 100);
        assert_eq!(refusal(complete_upload(&state.db, &store, "u2", u.file_id, "n.txt", "text/plain", 100, Utc::now()).await), (409, "upload-missing"));
        let done = complete_upload(&state.db, &store, "u1", u.file_id, "n.txt", "text/plain", 100, Utc::now()).await.ok().unwrap();
        assert_eq!(done["bytes"], 100);
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 100);
    }

    #[tokio::test]
    #[serial]
    async fn only_the_owner_downloads_or_deletes_and_delete_frees_quota() {
        let state = db().await;
        let store = Fake::default();
        let v = upload(&state, &store, "u1", "free", "../evil/n.txt", 50).await.ok().unwrap();
        assert_eq!(v["name"], "n.txt");
        let id: Uuid = serde_json::from_value(v["fileId"].clone()).unwrap();
        assert_eq!(refusal(download_url(&state.db, &store, "u2", id, None).await), (404, "not-found"));
        assert_eq!(refusal(delete_file(&state.db, &store, "u2", id, Utc::now()).await.map(|_| ())), (404, "not-found"));
        let d = download_url(&state.db, &store, "u1", id, None).await.ok().unwrap();
        assert!(d["url"].as_str().unwrap().contains(&format!("u/u1/{id}/n.txt")));
        delete_file(&state.db, &store, "u1", id, Utc::now()).await.ok().unwrap();
        assert!(store.objects.lock().unwrap().is_empty(), "the object is deleted");
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 0);
        assert_eq!(refusal(download_url(&state.db, &store, "u1", id, None).await), (404, "not-found"));
        assert_eq!(refusal(delete_file(&state.db, &store, "u1", id, Utc::now()).await.map(|_| ())), (404, "not-found"));
    }

    struct FakeFetch(Result<(Vec<u8>, String), String>);
    #[async_trait]
    impl Fetcher for FakeFetch {
        async fn fetch(&self, _u: &str, _m: u64) -> Result<(Vec<u8>, String), String> {
            self.0.clone()
        }
    }

    #[tokio::test]
    #[serial]
    async fn remote_media_is_copied_under_the_owner_and_respects_caps() {
        let state = db().await;
        let store = Fake::default();
        let ok = FakeFetch(Ok((vec![7u8; 1000], "image/jpeg".into())));
        let got = ingest_remote(&state.db, &store, &ok, "u1", "free", "https://media.test/a/b/pic.jpg?sig=1", "", Utc::now()).await.unwrap();
        assert_eq!((got.name.as_str(), got.content_type.as_str(), got.bytes), ("pic.jpg", "image/jpeg", 1000));
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 1000);
        assert!(store.objects.lock().unwrap().contains_key(&format!("u/u1/{}/pic.jpg", got.file_id)));
        // The carrier's content type is a fallback only when the host sends none.
        let none = FakeFetch(Ok((vec![1u8; 10], String::new())));
        let g2 = ingest_remote(&state.db, &store, &none, "u1", "free", "https://media.test/x", "image/png", Utc::now()).await.unwrap();
        assert_eq!((g2.name.as_str(), g2.content_type.as_str()), ("x", "image/png"));
        // Over the plan's per-file cap (free: 10 MB): refused, nothing stored.
        let big = FakeFetch(Ok((vec![0u8; (10 * MB + 1) as usize], "video/mp4".into())));
        let before = used_bytes(&state.db, "u1").await.unwrap();
        let err = ingest_remote(&state.db, &store, &big, "u1", "free", "https://media.test/v.mp4", "", Utc::now()).await.unwrap_err();
        assert!(err.contains("file-too-large"), "{err}");
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), before);
        // Fetch failures and empty bodies are errors too, so the caller keeps the original URL.
        assert!(ingest_remote(&state.db, &store, &FakeFetch(Err("timeout".into())), "u1", "free", "https://m.test/a", "", Utc::now()).await.is_err());
        assert!(ingest_remote(&state.db, &store, &FakeFetch(Ok((vec![], "image/png".into()))), "u1", "free", "https://m.test/a", "", Utc::now()).await.is_err());
    }

    #[tokio::test]
    #[serial]
    async fn capability_links_redirect_and_die_with_the_file() {
        let state = db().await;
        let store = Fake::default();
        let v = upload(&state, &store, "u1", "free", "n.txt", 50).await.ok().unwrap();
        let id: Uuid = serde_json::from_value(v["fileId"].clone()).unwrap();
        let tok = link_token("s3cret", id);
        // Unset secret: 503.
        assert_eq!(refusal(raw_redirect_with(None, &state.db, &store, id, &tok).await), (503, "file-links-unavailable"));
        // Bad token (and a token from another secret): 404.
        assert_eq!(refusal(raw_redirect_with(Some("s3cret"), &state.db, &store, id, "bogus").await), (404, "not-found"));
        assert_eq!(refusal(raw_redirect_with(Some("s3cret"), &state.db, &store, id, &link_token("other", id)).await), (404, "not-found"));
        // Good token: a fresh presigned GET for the object.
        let url = raw_redirect_with(Some("s3cret"), &state.db, &store, id, &tok).await.ok().unwrap();
        assert_eq!(url, format!("https://r2.test/{BUCKET}/u/u1/{id}/n.txt?get"));
        // Deleted file: the same valid token now answers 404.
        delete_file(&state.db, &store, "u1", id, Utc::now()).await.ok().unwrap();
        assert_eq!(refusal(raw_redirect_with(Some("s3cret"), &state.db, &store, id, &tok).await), (404, "not-found"));
    }

    #[tokio::test]
    #[serial]
    async fn stored_bytes_follow_the_plan_caps() {
        let state = db().await;
        let store = Fake::default();
        let a = store_bytes(&state.db, &store, "u1", "free", "../r.pdf", "application/pdf", vec![7u8; 1024], Utc::now()).await.ok().unwrap();
        assert_eq!((a["name"].as_str(), a["bytes"].as_u64()), (Some("r.pdf"), Some(1024)));
        let id: Uuid = serde_json::from_value(a["fileId"].clone()).unwrap();
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 1024);
        assert_eq!(store.objects.lock().unwrap().len(), 1);
        // Over the per-file cap, or empty: nothing is stored.
        assert_eq!(refusal(store_bytes(&state.db, &store, "u1", "free", "big", "", vec![0u8; (10 * MB + 1) as usize], Utc::now()).await), (413, "file-too-large"));
        assert!(matches!(store_bytes(&state.db, &store, "u1", "free", "empty", "", vec![], Utc::now()).await, Err(Failure::Api(_))));
        assert_eq!(store.objects.lock().unwrap().len(), 1);
        // The stored file gets a permanent link when links are configured, and its record is the owner's.
        assert!(a.get("linkUrl").is_some());
        delete_file(&state.db, &store, "u1", id, Utc::now()).await.ok().unwrap();
        assert_eq!(used_bytes(&state.db, "u1").await.unwrap(), 0);
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn tokens_verify_only_for_their_file_and_secret() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let t = link_token("k", a);
        assert!(verify_link_token("k", a, &t));
        assert!(!verify_link_token("k", b, &t));
        assert!(!verify_link_token("k2", a, &t));
        assert!(!verify_link_token("k", a, ""));
        assert!(!verify_link_token("k", a, "!!!"));
        let url = capability_url_with(Some("k"), "https://x.test/", a).unwrap();
        assert_eq!(url, format!("https://x.test/api/v1/files/{a}/raw?k={t}"));
        assert!(capability_url_with(None, "https://x.test", a).is_none());
    }
}
