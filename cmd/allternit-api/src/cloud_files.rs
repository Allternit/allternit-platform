//! Upload a bot-produced file to the owner's cloud storage and get a permanent link back.
//!
//! The runtime authenticates as itself (its device credential, see [`crate::phone_sync::runtime_bearer`]):
//! `POST <cloud>/api/v1/runtime-devices/me/files/uploads` (plan caps, charged to the device owner) →
//! `PUT` the bytes to the presigned URL → `POST …/files/:id/complete` → `linkUrl`, a capability link that
//! does not expire (until the owner deletes the file). Anything a bot makes that a person must open from a
//! message (an SMS, a pasted reply) can go through [`upload_file`] / [`upload_bytes`].

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// Where uploads go; tests substitute a fake.
#[async_trait]
pub trait FileUploader: Send + Sync {
    /// The permanent link for the uploaded bytes, or why it could not be made.
    async fn upload(&self, cloud_url: &str, bearer: &str, name: &str, mime: &str, data: Vec<u8>) -> Result<String, String>;
}

pub struct CloudUploader;

fn message(body: &Value, fallback: &str) -> String {
    body["message"].as_str().or_else(|| body["code"].as_str()).unwrap_or(fallback).to_string()
}

#[async_trait]
impl FileUploader for CloudUploader {
    async fn upload(&self, cloud_url: &str, bearer: &str, name: &str, mime: &str, data: Vec<u8>) -> Result<String, String> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().map_err(|e| e.to_string())?;
        let body = json!({ "name": name, "contentType": mime, "bytes": data.len() });
        let base = format!("{}/api/v1/runtime-devices/me/files", cloud_url.trim_end_matches('/'));

        let begin = client.post(format!("{base}/uploads")).bearer_auth(bearer).json(&body).send().await.map_err(|e| e.to_string())?;
        let status = begin.status();
        let begin: Value = begin.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(message(&begin, &format!("the cloud refused the upload ({})", status.as_u16())));
        }
        let (Some(file_id), Some(put_url)) = (begin["fileId"].as_str(), begin["putUrl"].as_str()) else {
            return Err("the cloud sent no upload URL".into());
        };

        let put = client.put(put_url).header("Content-Type", mime).body(data).send().await.map_err(|e| e.to_string())?;
        if !put.status().is_success() {
            return Err(format!("storage refused the file ({})", put.status().as_u16()));
        }

        let done = client.post(format!("{base}/{file_id}/complete")).bearer_auth(bearer).json(&body).send().await.map_err(|e| e.to_string())?;
        let status = done.status();
        let done: Value = done.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(message(&done, &format!("the upload could not be verified ({})", status.as_u16())));
        }
        done["linkUrl"].as_str().map(str::to_string).ok_or_else(|| "permanent file links are not enabled on this Allternit cloud".to_string())
    }
}

pub fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("txt" | "md") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Upload `data` as `name` with the runtime's own credential. `Err` says why (unpaired, over a plan cap, …).
pub async fn upload_bytes(uploader: &dyn FileUploader, name: &str, mime: &str, data: Vec<u8>) -> Result<String, String> {
    let bearer = crate::phone_sync::runtime_bearer().ok_or("this runtime is not paired with an Allternit account")?;
    upload_with(uploader, &crate::phone_sync::cloud_base(), &bearer, name, mime, data).await
}

pub async fn upload_with(uploader: &dyn FileUploader, cloud_url: &str, bearer: &str, name: &str, mime: &str, data: Vec<u8>) -> Result<String, String> {
    if data.is_empty() {
        return Err("the file is empty".into());
    }
    uploader.upload(cloud_url, bearer, name, mime, data).await
}

/// Read a local file and upload it; the content type comes from the extension.
pub async fn upload_file(uploader: &dyn FileUploader, path: &Path) -> Result<String, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let data = tokio::fs::read(path).await.map_err(|e| format!("could not read {name}: {e}"))?;
    upload_bytes(uploader, &name, mime_for(&name), data).await
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::sync::Mutex;

    /// Records uploads and answers with a link per file name.
    #[derive(Default)]
    pub struct FakeUploader {
        pub seen: Mutex<Vec<(String, String, String, usize)>>,
        pub fail: Mutex<Option<String>>,
    }

    #[async_trait]
    impl FileUploader for FakeUploader {
        async fn upload(&self, cloud_url: &str, bearer: &str, name: &str, _mime: &str, data: Vec<u8>) -> Result<String, String> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            self.seen.lock().unwrap().push((cloud_url.into(), bearer.into(), name.into(), data.len()));
            Ok(format!("https://api.test/api/v1/files/{name}/raw?k=t"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::FakeUploader;
    use super::*;

    #[tokio::test]
    async fn uploads_use_the_given_credential_and_refuse_empty_files() {
        let up = FakeUploader::default();
        let link = upload_with(&up, "https://cloud.test", "allternit_runtime_x", "r.pdf", "application/pdf", vec![1, 2, 3]).await.unwrap();
        assert_eq!(link, "https://api.test/api/v1/files/r.pdf/raw?k=t");
        assert_eq!(up.seen.lock().unwrap()[0], ("https://cloud.test".into(), "allternit_runtime_x".into(), "r.pdf".into(), 3));
        assert!(upload_with(&up, "https://cloud.test", "b", "e.txt", "text/plain", vec![]).await.is_err());
        *up.fail.lock().unwrap() = Some("over plan".into());
        assert_eq!(upload_with(&up, "https://cloud.test", "b", "x", "text/plain", vec![1]).await.unwrap_err(), "over plan");
    }

    #[test]
    fn mime_comes_from_the_extension() {
        assert_eq!((mime_for("a.PNG"), mime_for("b.pdf"), mime_for("noext")), ("image/png", "application/pdf", "application/octet-stream"));
    }
}
