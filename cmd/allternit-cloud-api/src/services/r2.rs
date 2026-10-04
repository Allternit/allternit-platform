//! Small S3 SigV4 client for Cloudflare R2 (region `auto`, service `s3`,
//! path-style URLs). No SDK: `hmac` + `sha2` + `reqwest`.
//!
//! Configure with `ALLTERNIT_R2_ENDPOINT` (`https://<account>.r2.cloudflarestorage.com`),
//! `ALLTERNIT_R2_ACCESS_KEY` and `ALLTERNIT_R2_SECRET`. [`R2Client::from_env`]
//! answers [`R2Error::Unavailable`] when any is missing; routes map that to 503.
//!
//! ```ignore
//! let r2 = R2Client::from_env()?;
//! let url = r2.presign_get("allternit-call-recordings", "calls/abc.ogg", Duration::from_secs(600))?;
//! let size = r2.head("allternit-call-recordings", "calls/abc.ogg").await?; // None = gone
//! ```
//!
//! Presigned URLs are bearer credentials: never log them, and never log the
//! secret. `Debug` on the client redacts both.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
/// R2 accepts presigned URLs for at most 7 days.
const MAX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, thiserror::Error)]
pub enum R2Error {
    /// The `ALLTERNIT_R2_*` env is not set. Routes answer 503.
    #[error("R2 is not configured")]
    Unavailable,
    #[error("R2 request failed: {0}")]
    Http(String),
    #[error("R2 answered {0}")]
    Status(u16),
    #[error("R2 sent an unreadable answer")]
    Parse,
}

/// What routes need from R2. Implemented by [`R2Client`]; tests substitute a fake.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    fn presign_get(&self, bucket: &str, key: &str, ttl: Duration) -> Result<String, R2Error>;
    /// Size in bytes, or `None` when the object does not exist.
    async fn head(&self, bucket: &str, key: &str) -> Result<Option<u64>, R2Error>;
}

#[derive(Clone)]
pub struct R2Client {
    endpoint: String,
    access_key: String,
    secret: String,
    region: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for R2Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2Client").field("endpoint", &self.endpoint).field("access_key", &"<redacted>").field("secret", &"<redacted>").finish()
    }
}

impl R2Client {
    pub fn from_env() -> Result<Self, R2Error> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, R2Error> {
        let val = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        match (val("ALLTERNIT_R2_ENDPOINT"), val("ALLTERNIT_R2_ACCESS_KEY"), val("ALLTERNIT_R2_SECRET")) {
            (Some(endpoint), Some(access_key), Some(secret)) => Ok(Self::new(&endpoint, &access_key, &secret, "auto")),
            _ => Err(R2Error::Unavailable),
        }
    }

    pub fn new(endpoint: &str, access_key: &str, secret: &str, region: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            access_key: access_key.to_string(),
            secret: secret.to_string(),
            region: region.to_string(),
            http: reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_default(),
        }
    }

    fn host(&self) -> &str {
        self.endpoint.split_once("://").map(|(_, h)| h).unwrap_or(&self.endpoint)
    }

    fn object_path(bucket: &str, key: &str) -> String {
        format!("/{}/{}", uri_encode(bucket, true), uri_encode(key, true))
    }

    /// Presigned GET, valid for `ttl` (capped at 7 days).
    pub fn presign_get(&self, bucket: &str, key: &str, ttl: Duration) -> Result<String, R2Error> {
        Ok(self.presign_at("GET", &Self::object_path(bucket, key), ttl, &[], Utc::now()))
    }

    /// Presigned PUT. `content_type` is signed, so the uploader must send the
    /// same `Content-Type`. With `content_length`, that exact `Content-Length`
    /// is signed too (SigV4 URLs cannot express a maximum, only an exact size).
    pub fn presign_put(&self, bucket: &str, key: &str, ttl: Duration, content_type: &str, content_length: Option<u64>) -> Result<String, R2Error> {
        let mut headers = vec![("content-type".to_string(), content_type.to_string())];
        if let Some(n) = content_length {
            headers.push(("content-length".to_string(), n.to_string()));
        }
        Ok(self.presign_at("PUT", &Self::object_path(bucket, key), ttl, &headers, Utc::now()))
    }

    pub async fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), R2Error> {
        let hash = hex::encode(Sha256::digest(&bytes));
        let path = Self::object_path(bucket, key);
        let signed = self.sign_request("PUT", &path, "", &hash, &[("content-type", content_type)], Utc::now());
        let resp = self.send(self.http.put(format!("{}{path}", self.endpoint)).body(bytes), signed).await?;
        ok_status(resp.status().as_u16())
    }

    pub async fn delete(&self, bucket: &str, key: &str) -> Result<(), R2Error> {
        let path = Self::object_path(bucket, key);
        let signed = self.sign_request("DELETE", &path, "", &hex::encode(Sha256::digest(b"")), &[], Utc::now());
        let resp = self.send(self.http.delete(format!("{}{path}", self.endpoint)), signed).await?;
        match resp.status().as_u16() {
            404 => Ok(()),
            s => ok_status(s),
        }
    }

    /// Object size in bytes; `None` when it does not exist.
    pub async fn head(&self, bucket: &str, key: &str) -> Result<Option<u64>, R2Error> {
        let path = Self::object_path(bucket, key);
        let signed = self.sign_request("HEAD", &path, "", &hex::encode(Sha256::digest(b"")), &[], Utc::now());
        let resp = self.send(self.http.head(format!("{}{path}", self.endpoint)), signed).await?;
        match resp.status().as_u16() {
            404 => Ok(None),
            s if (200..300).contains(&s) => Ok(Some(resp.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).ok_or(R2Error::Parse)?)),
            s => Err(R2Error::Status(s)),
        }
    }

    /// Total bytes of all objects whose key starts with `prefix`.
    pub async fn list_prefix_size(&self, bucket: &str, prefix: &str) -> Result<u64, R2Error> {
        let path = format!("/{}", uri_encode(bucket, true));
        let (mut total, mut token) = (0u64, None::<String>);
        loop {
            let mut q = vec![("list-type".to_string(), "2".to_string()), ("prefix".to_string(), prefix.to_string())];
            if let Some(t) = &token {
                q.push(("continuation-token".to_string(), t.clone()));
            }
            let query = canonical_query(&q);
            let signed = self.sign_request("GET", &path, &query, &hex::encode(Sha256::digest(b"")), &[], Utc::now());
            let resp = self.send(self.http.get(format!("{}{path}?{query}", self.endpoint)), signed).await?;
            ok_status(resp.status().as_u16())?;
            let body = resp.text().await.map_err(|e| R2Error::Http(e.to_string()))?;
            let (size, next) = parse_list(&body);
            total += size;
            match next {
                Some(t) => token = Some(t),
                None => return Ok(total),
            }
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder, headers: Vec<(String, String)>) -> Result<reqwest::Response, R2Error> {
        let mut req = req;
        for (k, v) in headers {
            req = req.header(k, v);
        }
        // reqwest errors can embed the URL; keep only the kind.
        req.send().await.map_err(|e| R2Error::Http(if e.is_timeout() { "timeout".into() } else if e.is_connect() { "connect".into() } else { "request".into() }))
    }

    // ------------------------------------------------------------- signing

    /// Query-string presign of `method path` at `now`. `extra` are lowercase
    /// header names and values that get signed besides `host`.
    pub(crate) fn presign_at(&self, method: &str, path: &str, ttl: Duration, extra: &[(String, String)], now: DateTime<Utc>) -> String {
        let ttl = ttl.min(MAX_TTL).max(Duration::from_secs(1));
        let (amz_date, day) = (now.format("%Y%m%dT%H%M%SZ").to_string(), now.format("%Y%m%d").to_string());
        let scope = format!("{day}/{}/s3/aws4_request", self.region);

        let mut headers: Vec<(String, String)> = vec![("host".into(), self.host().to_string())];
        headers.extend(extra.iter().cloned());
        headers.sort();
        let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");

        let query = vec![
            ("X-Amz-Algorithm".to_string(), "AWS4-HMAC-SHA256".to_string()),
            ("X-Amz-Credential".to_string(), format!("{}/{scope}", self.access_key)),
            ("X-Amz-Date".to_string(), amz_date.clone()),
            ("X-Amz-Expires".to_string(), ttl.as_secs().to_string()),
            ("X-Amz-SignedHeaders".to_string(), signed_headers.clone()),
        ];
        let query = canonical_query(&query);
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
        let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{UNSIGNED_PAYLOAD}");
        let sig = self.signature(&canonical, &amz_date, &day, &scope);
        format!("{}{path}?{query}&X-Amz-Signature={sig}", self.endpoint)
    }

    /// Authorization-header signing. Returns the headers to attach.
    pub(crate) fn sign_request(&self, method: &str, path: &str, query: &str, payload_hash: &str, extra: &[(&str, &str)], now: DateTime<Utc>) -> Vec<(String, String)> {
        let (amz_date, day) = (now.format("%Y%m%dT%H%M%SZ").to_string(), now.format("%Y%m%d").to_string());
        let scope = format!("{day}/{}/s3/aws4_request", self.region);
        let mut headers: Vec<(String, String)> = vec![
            ("host".into(), self.host().to_string()),
            ("x-amz-content-sha256".into(), payload_hash.to_string()),
            ("x-amz-date".into(), amz_date.clone()),
        ];
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        headers.sort();
        let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
        let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
        let sig = self.signature(&canonical, &amz_date, &day, &scope);
        let auth = format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={sig}", self.access_key);
        // `host` is set by the HTTP client itself.
        let mut out: Vec<(String, String)> = headers.into_iter().filter(|(k, _)| k != "host").collect();
        out.push(("authorization".into(), auth));
        out
    }

    fn signature(&self, canonical_request: &str, amz_date: &str, day: &str, scope: &str) -> String {
        let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex::encode(Sha256::digest(canonical_request.as_bytes())));
        hex::encode(hmac(&signing_key(&self.secret, day, &self.region, "s3"), to_sign.as_bytes()))
    }
}

#[async_trait]
impl ObjectStore for R2Client {
    fn presign_get(&self, bucket: &str, key: &str, ttl: Duration) -> Result<String, R2Error> {
        R2Client::presign_get(self, bucket, key, ttl)
    }
    async fn head(&self, bucket: &str, key: &str) -> Result<Option<u64>, R2Error> {
        R2Client::head(self, bucket, key).await
    }
}

fn ok_status(s: u16) -> Result<(), R2Error> {
    if (200..300).contains(&s) {
        Ok(())
    } else {
        Err(R2Error::Status(s))
    }
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

pub(crate) fn signing_key(secret: &str, day: &str, region: &str, service: &str) -> Vec<u8> {
    let k = hmac(format!("AWS4{secret}").as_bytes(), day.as_bytes());
    let k = hmac(&k, region.as_bytes());
    let k = hmac(&k, service.as_bytes());
    hmac(&k, b"aws4_request")
}

/// RFC 3986 encoding as SigV4 wants it: only `A-Za-z0-9-_.~` stay; `/` stays when `keep_slash`.
pub(crate) fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn canonical_query(params: &[(String, String)]) -> String {
    let mut p: Vec<(String, String)> = params.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
    p.sort();
    p.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// Sum of `<Size>` and the next continuation token (when `<IsTruncated>true`).
fn parse_list(xml: &str) -> (u64, Option<String>) {
    let tag = |s: &str, name: &str| -> Vec<String> {
        let (open, close) = (format!("<{name}>"), format!("</{name}>"));
        let mut out = vec![];
        let mut rest = s;
        while let Some(i) = rest.find(&open) {
            let after = &rest[i + open.len()..];
            let Some(j) = after.find(&close) else { break };
            out.push(after[..j].to_string());
            rest = &after[j + close.len()..];
        }
        out
    };
    let size = tag(xml, "Size").iter().filter_map(|s| s.parse::<u64>().ok()).sum();
    let truncated = tag(xml, "IsTruncated").first().map(|v| v == "true").unwrap_or(false);
    let next = if truncated { tag(xml, "NextContinuationToken").into_iter().next() } else { None };
    (size, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn signing_key_matches_aws_documented_vector() {
        // https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html
        let k = signing_key("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830", "us-east-1", "iam");
        assert_eq!(hex::encode(k), "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9");
    }

    #[test]
    fn presign_matches_aws_s3_presigned_url_example() {
        // https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html
        let c = R2Client::new("https://examplebucket.s3.amazonaws.com", "AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "us-east-1");
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let url = c.presign_at("GET", "/test.txt", Duration::from_secs(86400), &[], now);
        assert_eq!(
            url,
            "https://examplebucket.s3.amazonaws.com/test.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn header_auth_matches_aws_s3_get_object_example() {
        // https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html (GET Object with Range)
        let c = R2Client::new("https://examplebucket.s3.amazonaws.com", "AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "us-east-1");
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let h = c.sign_request("GET", "/test.txt", "", &hex::encode(Sha256::digest(b"")), &[("range", "bytes=0-9")], now);
        let auth = &h.iter().find(|(k, _)| k == "authorization").unwrap().1;
        assert!(auth.ends_with("Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"), "{auth}");
        assert!(auth.contains("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date"));
    }

    #[test]
    fn r2_shaped_url_is_path_style_with_region_auto_and_encoded_key() {
        let c = R2Client::new("https://acct123.r2.cloudflarestorage.com/", "AK", "SECRET", "auto");
        let url = c.presign_get("allternit-call-recordings", "calls/a b+c.ogg", Duration::from_secs(600)).unwrap();
        assert!(url.starts_with("https://acct123.r2.cloudflarestorage.com/allternit-call-recordings/calls/a%20b%2Bc.ogg?X-Amz-Algorithm=AWS4-HMAC-SHA256&"), "{url}");
        assert!(url.contains("%2Fauto%2Fs3%2Faws4_request"));
        assert!(url.contains("X-Amz-Expires=600&X-Amz-SignedHeaders=host&X-Amz-Signature="));
        // 7-day cap.
        let long = c.presign_get("b", "k", Duration::from_secs(30 * 86400)).unwrap();
        assert!(long.contains("X-Amz-Expires=604800"));
    }

    #[test]
    fn put_presign_signs_content_type_and_length() {
        let c = R2Client::new("https://x.r2.cloudflarestorage.com", "AK", "S", "auto");
        let url = c.presign_put("b", "k", Duration::from_secs(60), "audio/ogg", Some(10)).unwrap();
        assert!(url.contains("X-Amz-SignedHeaders=content-length%3Bcontent-type%3Bhost"), "{url}");
    }

    #[test]
    fn unconfigured_env_is_unavailable_and_debug_hides_secrets() {
        assert!(matches!(R2Client::from_lookup(|_| None), Err(R2Error::Unavailable)));
        assert!(matches!(R2Client::from_lookup(|k| (k != "ALLTERNIT_R2_SECRET").then(|| "x".to_string())), Err(R2Error::Unavailable)));
        let c = R2Client::from_lookup(|k| Some(if k.ends_with("ENDPOINT") { "https://e.example".into() } else { "topsecret".into() })).unwrap();
        assert!(!format!("{c:?}").contains("topsecret"));
    }

    #[test]
    fn list_response_is_summed_and_paged() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>tok</NextContinuationToken><Contents><Size>10</Size></Contents><Contents><Size>5</Size></Contents></ListBucketResult>";
        assert_eq!(parse_list(xml), (15, Some("tok".to_string())));
        assert_eq!(parse_list("<R><IsTruncated>false</IsTruncated><Contents><Size>7</Size></Contents></R>"), (7, None));
    }
}

/// Round trip against real R2 (put, head, presigned GET, delete). Opt-in:
/// `ALLTERNIT_R2_* cargo test -p allternit-cloud-api --lib r2_live -- --ignored`.
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn r2_live_round_trip() {
        let c = R2Client::from_env().expect("ALLTERNIT_R2_* not set");
        let (b, k) = ("allternit-call-recordings", "calls/live-check-r2-client.ogg");
        c.put(b, k, b"hello".to_vec(), "audio/ogg").await.unwrap();
        assert_eq!(c.head(b, k).await.unwrap(), Some(5));
        let url = c.presign_get(b, k, Duration::from_secs(60)).unwrap();
        let got = reqwest::get(&url).await.unwrap();
        assert!(got.status().is_success(), "presigned GET: {}", got.status());
        assert_eq!(&got.bytes().await.unwrap()[..], b"hello");
        c.delete(b, k).await.unwrap();
        assert_eq!(c.head(b, k).await.unwrap(), None);
    }
}
