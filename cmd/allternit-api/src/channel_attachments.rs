//! Files that travel with channel messages, kept so a thread can offer them as downloads.
//!
//! The runtime does not hold the bytes. Sent and received attachments go to the
//! user's file store at the cloud (the R2 bucket behind Settings > Files, under
//! the plan's caps) through the runtime upload in [`crate::cloud_files`], and
//! the thread message carries `files: [{name, mime, size, url}]`. `url` is the
//! file's permanent link (`/api/v1/files/:id/raw?k=…`), so an old message's
//! download keeps working. A file that could not be kept (no cloud pairing, over
//! the plan cap, the platform refused the download) still shows by name: its
//! entry has `name` and a short `error` instead of `url`.
//!
//! Inbound:
//! * Telegram: `getFile` then the file endpoint, with the connection's bot
//!   token (https://core.telegram.org/bots/api#getfile; bots download up to 20 MB).
//! * Slack: the file's `url_private_download` needs the bot token and the
//!   `files:read` scope (https://docs.slack.dev/messaging/working-with-files/#downloading).
//!   A shared-app token never leaves the cloud, so the cloud downloads it
//!   (`POST /api/v1/channels/slack/file`) and stores it in one step.
//!
//! The Telegram and Slack payloads are read here, not in the transports, so no
//! other module's `Inbound` shape changes: [`inbound_files`] reads the raw
//! payload and [`crate::channel_transports::dispatch_events_with_files`] keeps
//! what it fetched until `record_inbound` writes the event.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{json, Value};

use crate::channel_files::{ChannelFile, MAX_EACH_BYTES};
use crate::channel_transports::{HttpReq, HttpSend};

/// At most this many files are kept from one inbound message.
pub const MAX_INBOUND_FILES: usize = 5;

/// A file the cloud holds for the user, with its permanent link.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredFile {
    pub url: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

impl StoredFile {
    pub fn to_json(&self) -> Value {
        json!({ "name": self.name, "mime": self.mime, "size": self.size, "url": self.url })
    }
}

/// The entry for a file that could not be kept: shown by name, with why.
fn unkept(name: &str, mime: &str, size: Option<u64>, why: &str) -> Value {
    let mut v = json!({ "name": name, "mime": mime, "error": why });
    if let Some(s) = size {
        v["size"] = json!(s);
    }
    v
}

// ---------------------------------------------------------------- cloud client

/// The user's file store at the cloud, reached with the runtime's own bearer.
pub struct CloudFiles {
    http: Arc<dyn HttpSend>,
    uploader: Arc<dyn crate::cloud_files::FileUploader>,
    cloud: String,
    token: Option<String>,
}

impl CloudFiles {
    pub fn new(http: Arc<dyn HttpSend>, uploader: Arc<dyn crate::cloud_files::FileUploader>, cloud: String, token: Option<String>) -> Self {
        Self { http, uploader, cloud: cloud.trim_end_matches('/').to_string(), token }
    }

    /// The paired runtime's cloud and bearer; unpaired (no token) keeps nothing.
    pub fn from_env(http: Arc<dyn HttpSend>) -> Self {
        Self::new(http, Arc::new(crate::cloud_files::CloudUploader), crate::phone_sync::cloud_base(), crate::phone_sync::runtime_bearer())
    }

    fn token(&self) -> Result<&str, String> {
        self.token.as_deref().ok_or_else(|| "not_paired".to_string())
    }

    /// Short reason a refused request gives, in words an entry can carry.
    fn refusal(status: u16, body: &Value) -> String {
        let code = body.get("code").or_else(|| body.get("error")).and_then(Value::as_str).unwrap_or("");
        match (status, code) {
            (_, c) if !c.is_empty() => c.to_string(),
            (401 | 403, _) => "not_signed_in".into(),
            (s, _) => format!("cloud_{s}"),
        }
    }

    /// Keep `data` as the user's file `name` (runtime upload, plan caps) and return its permanent link.
    pub async fn ingest(&self, name: &str, mime: &str, data: &[u8]) -> Result<StoredFile, String> {
        let token = self.token()?;
        let url = crate::cloud_files::upload_with(self.uploader.as_ref(), &self.cloud, token, name, mime, data.to_vec()).await?;
        Ok(StoredFile { url, name: name.to_string(), mime: mime.to_string(), size: data.len() as u64 })
    }

    /// Have the cloud download a Slack-hosted file with the install's token and keep it.
    pub async fn ingest_slack(&self, url: &str, name: &str, mime: &str) -> Result<StoredFile, String> {
        let resp = self
            .http
            .post_json(HttpReq {
                url: format!("{}/api/v1/channels/slack/file", self.cloud),
                headers: vec![("authorization".into(), format!("Bearer {}", self.token()?))],
                body: json!({ "url": url, "name": name, "mimeType": mime }),
            })
            .await?;
        match resp.status {
            200..=299 => Ok(StoredFile {
                url: resp.body.get("linkUrl").and_then(Value::as_str).ok_or("no_link")?.to_string(),
                name: resp.body.get("name").and_then(Value::as_str).unwrap_or(name).to_string(),
                mime: resp.body.get("contentType").and_then(Value::as_str).unwrap_or(mime).to_string(),
                size: resp.body.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            }),
            s => Err(Self::refusal(s, &resp.body)),
        }
    }
}

// ---------------------------------------------------------------- sent

/// Keep the files a bot just sent. One entry per file, in order. Never fails the
/// send: a file that couldn't be kept is listed by name with an `error`.
pub async fn sent_entries(cloud: &CloudFiles, files: &[ChannelFile]) -> Vec<Value> {
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        out.push(match cloud.ingest(&f.filename, &f.mime, &f.data).await {
            Ok(s) => s.to_json(),
            Err(why) => {
                if why != "not_paired" {
                    tracing::warn!(file = %f.filename, %why, "sent attachment was not kept in the user's files");
                }
                unkept(&f.filename, &f.mime, Some(f.data.len() as u64), &why)
            }
        });
    }
    out
}

/// [`sent_entries`] with the production client. Empty in, empty out, no network.
pub async fn sent_entries_prod(files: &[ChannelFile]) -> Vec<Value> {
    if files.is_empty() {
        return vec![];
    }
    sent_entries(&CloudFiles::from_env(Arc::new(crate::channel_transports::ReqwestSend)), files).await
}

// ---------------------------------------------------------------- inbound

/// Where an inbound file comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Telegram { file_id: String },
    Slack { url: String },
}

/// A file named in an inbound platform message, not yet downloaded.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFile {
    pub name: String,
    pub mime: String,
    pub size: Option<u64>,
    pub source: Source,
}

/// Files by the `remote_id` of the message that carried them.
pub type InboundFiles = HashMap<String, Vec<RemoteFile>>;

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Telegram message media: document, photo (the largest size), video, audio,
/// voice, animation, video note. https://core.telegram.org/bots/api#message
fn telegram_message_files(m: &Value) -> Vec<RemoteFile> {
    let mut out = vec![];
    let mut one = |obj: &Value, fallback_name: &str, fallback_mime: &str| {
        let Some(file_id) = str_at(obj, "file_id") else { return };
        out.push(RemoteFile {
            name: str_at(obj, "file_name").unwrap_or(fallback_name).to_string(),
            mime: str_at(obj, "mime_type").unwrap_or(fallback_mime).to_string(),
            size: obj.get("file_size").and_then(Value::as_u64),
            source: Source::Telegram { file_id: file_id.to_string() },
        });
    };
    for (key, name, mime) in [
        ("document", "document", "application/octet-stream"),
        ("video", "video.mp4", "video/mp4"),
        ("audio", "audio.mp3", "audio/mpeg"),
        ("voice", "voice.ogg", "audio/ogg"),
        ("animation", "animation.mp4", "video/mp4"),
        ("video_note", "video-note.mp4", "video/mp4"),
    ] {
        if let Some(obj) = m.get(key) {
            one(obj, name, mime);
        }
    }
    if let Some(sizes) = m.get("photo").and_then(Value::as_array) {
        // Several sizes of one photo: keep the largest.
        if let Some(best) = sizes.iter().max_by_key(|p| p.get("file_size").and_then(Value::as_u64).unwrap_or(0)) {
            one(best, "photo.jpg", "image/jpeg");
        }
    }
    out
}

/// Slack message `files[]`: `name`, `mimetype`, `size`, `url_private_download`.
/// https://docs.slack.dev/reference/objects/file-object
fn slack_message_files(ev: &Value) -> Vec<RemoteFile> {
    ev.get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| {
            // A tombstoned or externally hosted file has no private download.
            let url = str_at(f, "url_private_download").or_else(|| str_at(f, "url_private"))?;
            Some(RemoteFile {
                name: str_at(f, "name").or_else(|| str_at(f, "title")).unwrap_or("file").to_string(),
                mime: str_at(f, "mimetype").unwrap_or("application/octet-stream").to_string(),
                size: f.get("size").and_then(Value::as_u64),
                source: Source::Slack { url: url.to_string() },
            })
        })
        .collect()
}

/// The files in a raw webhook payload, keyed by the `remote_id` the transport's
/// `normalize` gives that message. Providers other than Telegram and Slack, and
/// payloads with no files, give an empty map.
pub fn inbound_files(provider: &str, payload: &Value) -> InboundFiles {
    let mut map = InboundFiles::new();
    match provider {
        "telegram" => {
            if let Some(m) = payload.get("message").or_else(|| payload.get("channel_post")) {
                let files = telegram_message_files(m);
                let ids = (m.pointer("/chat/id").and_then(Value::as_i64), m.get("message_id").and_then(Value::as_i64));
                if let (false, (Some(chat), Some(mid))) = (files.is_empty(), ids) {
                    map.insert(format!("{chat}:{mid}"), files);
                }
            }
        }
        "slack" => {
            let ev = payload.get("event").unwrap_or(payload);
            let files = slack_message_files(ev);
            if let (false, Some(ts)) = (files.is_empty(), str_at(ev, "ts")) {
                map.insert(ts.to_string(), files);
            }
        }
        _ => {}
    }
    map
}

/// What was kept of an inbound message's files, until `record_inbound` writes the event.
static KEPT: LazyLock<Mutex<HashMap<String, Vec<Value>>>> = LazyLock::new(Default::default);

fn kept_key(provider: &str, remote_id: &str) -> String {
    format!("{provider}:{remote_id}")
}

/// Take what was kept for a message (the event and the log row carry it).
pub fn take_kept(provider: &str, remote_id: &str) -> Option<Vec<Value>> {
    KEPT.lock().ok()?.remove(&kept_key(provider, remote_id))
}

/// Drop what was kept when the message turned out not to be recorded (a replay).
pub fn discard_kept(provider: &str, remote_id: &str) {
    let _ = take_kept(provider, remote_id);
}

/// Download one Telegram file with the bot token. `getFile` answers `result.file_path`,
/// the bytes are at `https://api.telegram.org/file/bot<token>/<file_path>`.
async fn telegram_download(http: &dyn HttpSend, token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let got = http
        .post_json(HttpReq { url: format!("https://api.telegram.org/bot{token}/getFile"), headers: vec![], body: json!({ "file_id": file_id }) })
        .await?;
    // Bots can download up to 20 MB; a larger file is answered "file is too big".
    if got.status == 400 {
        return Err("too_large".into());
    }
    let path = got.body.pointer("/result/file_path").and_then(Value::as_str).filter(|p| !p.is_empty() && !p.contains("..")).ok_or("no_file_path")?;
    let (status, bytes) = http.get_bytes(&format!("https://api.telegram.org/file/bot{token}/{path}"), vec![], MAX_EACH_BYTES).await?;
    match status {
        200 if !bytes.is_empty() => Ok(bytes),
        200 => Err("empty".into()),
        s => Err(format!("telegram_{s}")),
    }
}

/// How a connection's inbound files are fetched.
pub enum Fetcher {
    /// Telegram: this bot token.
    Telegram { token: String },
    /// Slack shared app: the cloud downloads with the install's token.
    SlackCloud,
    /// Slack custom (legacy) app: this bot token, from the runtime's env.
    SlackToken { token: String },
    /// Nothing can be fetched for this connection.
    None,
}

impl Fetcher {
    /// The fetcher for a provider account's sealed secret.
    pub fn for_account(provider: &str, secret: &str) -> Self {
        match provider {
            "telegram" => Some(crate::channel_transports::pick(secret, "botToken")).filter(|t| !t.is_empty()).map_or(Fetcher::None, |token| Fetcher::Telegram { token }),
            "slack" if crate::channel_slack_app::is_shared_secret(secret) => Fetcher::SlackCloud,
            "slack" => crate::config::AppConfig::load().slack_bot_token().map_or(Fetcher::None, |token| Fetcher::SlackToken { token }),
            _ => Fetcher::None,
        }
    }
}

/// Download and keep a message's files. Returns one entry per file (at most
/// [`MAX_INBOUND_FILES`]); a file that can't be kept is listed by name with an `error`.
pub async fn fetch_and_keep(http: Arc<dyn HttpSend>, cloud: &CloudFiles, fetcher: &Fetcher, files: &[RemoteFile]) -> Vec<Value> {
    let mut out = Vec::new();
    for f in files.iter().take(MAX_INBOUND_FILES) {
        if f.size.is_some_and(|s| s as usize > MAX_EACH_BYTES) {
            out.push(unkept(&f.name, &f.mime, f.size, "too_large"));
            continue;
        }
        let kept = match (&f.source, fetcher) {
            (Source::Telegram { file_id }, Fetcher::Telegram { token }) => match telegram_download(http.as_ref(), token, file_id).await {
                Ok(bytes) => cloud.ingest(&f.name, &f.mime, &bytes).await,
                Err(why) => Err(why),
            },
            (Source::Slack { url }, Fetcher::SlackCloud) => cloud.ingest_slack(url, &f.name, &f.mime).await,
            (Source::Slack { url }, Fetcher::SlackToken { token }) => {
                let host_ok = reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("files.slack.com"));
                if !host_ok {
                    Err("not_a_slack_file_url".into())
                } else {
                    match http.get_bytes(url, vec![("authorization".into(), format!("Bearer {token}"))], MAX_EACH_BYTES).await {
                        Ok((200, bytes)) if !bytes.is_empty() && !bytes.starts_with(b"<!DOCTYPE html") => cloud.ingest(&f.name, &f.mime, &bytes).await,
                        Ok((200, _)) | Ok((401 | 403, _)) => Err("slack_reauthorize_required".into()),
                        Ok((s, _)) => Err(format!("slack_{s}")),
                        Err(why) => Err(why),
                    }
                }
            }
            _ => Err("no_way_to_fetch".into()),
        };
        out.push(match kept {
            Ok(s) => s.to_json(),
            Err(why) => {
                if why != "not_paired" {
                    tracing::warn!(file = %f.name, %why, "inbound attachment was not kept");
                }
                unkept(&f.name, &f.mime, f.size, &why)
            }
        });
    }
    out
}

/// The line a message with no text of its own gets, so the thread and the bot
/// see that a file came: `[file: report.pdf, photo.jpg]`.
pub fn placeholder_text(entries: &[Value]) -> String {
    let names: Vec<&str> = entries.iter().filter_map(|e| e.get("name").and_then(Value::as_str)).collect();
    format!("[file: {}]", names.join(", "))
}

/// Fetch and keep the files for one inbound message, remember them for
/// `record_inbound`, and give a message without text a placeholder so it still
/// reaches a thread. `already_logged` skips the download on a platform retry.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_for_event(
    http: Arc<dyn HttpSend>,
    cloud: &CloudFiles,
    fetcher: &Fetcher,
    provider: &str,
    ev: &mut crate::channel_gateway::Inbound,
    files: &[RemoteFile],
    already_logged: bool,
) {
    if files.is_empty() || already_logged || ev.kind != crate::channel_gateway::InboundKind::Message || ev.own {
        return;
    }
    let entries = fetch_and_keep(http, cloud, fetcher, files).await;
    if ev.text.as_deref().map_or(true, |t| t.trim().is_empty()) {
        ev.text = Some(placeholder_text(&entries));
    }
    if let Ok(mut kept) = KEPT.lock() {
        // A bounded table: a stale entry from a message that never recorded is dropped first.
        if kept.len() > 256 {
            kept.clear();
        }
        kept.insert(kept_key(provider, &ev.remote_id), entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::HttpResp;
    use crate::cloud_files::fakes::FakeUploader;
    use async_trait::async_trait;

    #[derive(Default)]
    struct Fake {
        posts: Mutex<Vec<HttpReq>>,
        gets: Mutex<Vec<(String, Vec<(String, String)>)>>,
        get_reply: Mutex<Option<(u16, Vec<u8>)>>,
    }

    #[async_trait]
    impl HttpSend for Fake {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.posts.lock().unwrap().push(req.clone());
            Ok(if req.url.ends_with("/getFile") {
                HttpResp { status: 200, body: json!({ "ok": true, "result": { "file_path": "documents/file_7.pdf" } }) }
            } else if req.url.ends_with("/api/v1/channels/slack/file") {
                HttpResp { status: 201, body: json!({ "fileId": "f-2", "name": req.body["name"], "contentType": req.body["mimeType"], "bytes": 5, "linkUrl": "https://api.test/api/v1/files/f-2/raw?k=t" }) }
            } else {
                HttpResp { status: 500, body: Value::Null }
            })
        }
        async fn get_bytes(&self, url: &str, headers: Vec<(String, String)>, _limit: usize) -> Result<(u16, Vec<u8>), String> {
            self.gets.lock().unwrap().push((url.into(), headers));
            Ok(self.get_reply.lock().unwrap().clone().unwrap_or((200, b"%PDF-1".to_vec())))
        }
    }

    fn cloud(http: &Arc<Fake>, token: Option<&str>) -> CloudFiles {
        cloud_with(http, Arc::new(FakeUploader::default()), token)
    }

    fn cloud_with(http: &Arc<Fake>, up: Arc<FakeUploader>, token: Option<&str>) -> CloudFiles {
        CloudFiles::new(http.clone(), up, "https://api.test/".into(), token.map(str::to_string))
    }

    #[test]
    fn telegram_media_is_found_and_the_largest_photo_wins() {
        let update = json!({ "update_id": 1, "message": { "message_id": 9, "chat": { "id": -42 }, "caption": "q3", "document": { "file_id": "D1", "file_name": "q3.pdf", "mime_type": "application/pdf", "file_size": 2048 },
            "photo": [{ "file_id": "P1", "file_size": 100 }, { "file_id": "P3", "file_size": 900 }, { "file_id": "P2", "file_size": 400 }] } });
        let map = inbound_files("telegram", &update);
        let files = &map["-42:9"];
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].name.as_str(), files[0].mime.as_str(), files[0].size), ("q3.pdf", "application/pdf", Some(2048)));
        assert_eq!(files[1].source, Source::Telegram { file_id: "P3".into() });
        assert_eq!(files[1].name, "photo.jpg");
        // No media, or not a message: nothing.
        assert!(inbound_files("telegram", &json!({ "message": { "message_id": 1, "chat": { "id": 5 }, "text": "hi" } })).is_empty());
        assert!(inbound_files("telegram", &json!({ "message_reaction": {} })).is_empty());
        assert!(inbound_files("sms", &update).is_empty());
    }

    #[test]
    fn slack_files_are_keyed_by_the_message_ts() {
        let ev = json!({ "type": "message", "subtype": "file_share", "ts": "1700.01", "channel": "C1", "files": [
            { "name": "plan.xlsx", "mimetype": "application/vnd.ms-excel", "size": 5, "url_private_download": "https://files.slack.com/files-pri/T-F/download/plan.xlsx" },
            { "name": "gone", "mode": "tombstone" }] });
        let map = inbound_files("slack", &json!({ "event": ev, "team_id": "T1" }));
        assert_eq!(map["1700.01"].len(), 1);
        assert_eq!(map["1700.01"][0].name, "plan.xlsx");
    }

    #[tokio::test]
    async fn a_telegram_file_is_downloaded_with_the_bot_token_and_kept() {
        let http = Arc::new(Fake::default());
        let files = vec![RemoteFile { name: "q3.pdf".into(), mime: "application/pdf".into(), size: Some(6), source: Source::Telegram { file_id: "D1".into() } }];
        let up = Arc::new(FakeUploader::default());
        let out = fetch_and_keep(http.clone(), &cloud_with(&http, up.clone(), Some("rt-token")), &Fetcher::Telegram { token: "123:abc".into() }, &files).await;
        assert_eq!(out, vec![json!({ "name": "q3.pdf", "mime": "application/pdf", "size": 6, "url": "https://api.test/api/v1/files/q3.pdf/raw?k=t" })]);
        let posts = http.posts.lock().unwrap();
        assert_eq!(posts[0].url, "https://api.telegram.org/bot123:abc/getFile");
        assert_eq!(up.seen.lock().unwrap()[0], ("https://api.test".to_string(), "rt-token".to_string(), "q3.pdf".to_string(), 6), "kept with the runtime upload and its own credential");
        assert_eq!(http.gets.lock().unwrap()[0].0, "https://api.telegram.org/file/bot123:abc/documents/file_7.pdf");
    }

    #[tokio::test]
    async fn a_shared_slack_file_is_kept_by_the_cloud_and_a_big_or_unkept_one_still_shows_by_name() {
        let http = Arc::new(Fake::default());
        let url = "https://files.slack.com/files-pri/T-F/download/plan.xlsx";
        let files = vec![
            RemoteFile { name: "plan.xlsx".into(), mime: "application/vnd.ms-excel".into(), size: Some(5), source: Source::Slack { url: url.into() } },
            RemoteFile { name: "movie.mov".into(), mime: "video/quicktime".into(), size: Some(50_000_000), source: Source::Slack { url: url.into() } },
        ];
        let out = fetch_and_keep(http.clone(), &cloud(&http, Some("t")), &Fetcher::SlackCloud, &files).await;
        assert_eq!(out[0]["url"], "https://api.test/api/v1/files/f-2/raw?k=t");
        assert_eq!((out[1]["name"].as_str(), out[1]["error"].as_str(), out[1].get("url")), (Some("movie.mov"), Some("too_large"), None));
        assert_eq!(http.posts.lock().unwrap().len(), 1, "the big file is never downloaded");
        // An unpaired runtime keeps nothing but still names the file, with no network.
        let http = Arc::new(Fake::default());
        let out = fetch_and_keep(http.clone(), &cloud(&http, None), &Fetcher::SlackCloud, &files[..1]).await;
        assert_eq!(out[0]["error"], "not_paired");
        assert!(http.posts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_legacy_slack_token_only_goes_to_slacks_file_host_and_a_sign_in_page_means_reauthorize() {
        let http = Arc::new(Fake::default());
        let fetcher = Fetcher::SlackToken { token: "xoxb-1".into() };
        let ok = RemoteFile { name: "a.pdf".into(), mime: "application/pdf".into(), size: None, source: Source::Slack { url: "https://files.slack.com/f/a.pdf".into() } };
        let evil = RemoteFile { source: Source::Slack { url: "https://evil.test/a.pdf".into() }, ..ok.clone() };
        let out = fetch_and_keep(http.clone(), &cloud(&http, Some("t")), &fetcher, &[ok.clone(), evil]).await;
        assert!(out[0]["url"].as_str().is_some_and(|u| u.ends_with("/a.pdf/raw?k=t")));
        assert_eq!(out[1]["error"], "not_a_slack_file_url");
        let gets = http.gets.lock().unwrap();
        assert_eq!(gets.len(), 1);
        assert_eq!(gets[0].1, vec![("authorization".to_string(), "Bearer xoxb-1".to_string())]);
        drop(gets);
        *http.get_reply.lock().unwrap() = Some((200, b"<!DOCTYPE html><html>".to_vec()));
        let out = fetch_and_keep(http.clone(), &cloud(&http, Some("t")), &fetcher, &[ok]).await;
        assert_eq!(out[0]["error"], "slack_reauthorize_required");
    }

    #[tokio::test]
    async fn sent_files_are_kept_and_a_refusal_keeps_the_name() {
        let http = Arc::new(Fake::default());
        let file = ChannelFile { filename: "r.pdf".into(), mime: "application/pdf".into(), data: b"%PDF-1.7".to_vec() };
        let out = sent_entries(&cloud(&http, Some("t")), std::slice::from_ref(&file)).await;
        assert_eq!(out, vec![json!({ "name": "r.pdf", "mime": "application/pdf", "size": 8, "url": "https://api.test/api/v1/files/r.pdf/raw?k=t" })]);
        let up = Arc::new(FakeUploader::default());
        *up.fail.lock().unwrap() = Some("storage-quota-exceeded".into());
        let out = sent_entries(&cloud_with(&http, up, Some("t")), std::slice::from_ref(&file)).await;
        assert_eq!((out[0]["error"].as_str(), out[0].get("url")), (Some("storage-quota-exceeded"), None));
        let out = sent_entries(&cloud(&http, None), std::slice::from_ref(&file)).await;
        assert_eq!((out[0]["name"].as_str(), out[0]["error"].as_str(), out[0]["size"].as_u64()), (Some("r.pdf"), Some("not_paired"), Some(8)));
        assert!(sent_entries_prod(&[]).await.is_empty());
    }

    #[tokio::test]
    async fn a_file_only_message_gets_a_placeholder_and_the_kept_files_are_taken_once() {
        let http = Arc::new(Fake::default());
        let mut ev = crate::channel_gateway::Inbound {
            kind: crate::channel_gateway::InboundKind::Message, workspace: None, channel: "9".into(), conversation: "telegram:9".into(), thread: None,
            remote_id: "9:1".into(), message_id: "1".into(), text: None, user: Some("7".into()), reaction: None, added: None, cursor: None, own: false,
        };
        let files = vec![RemoteFile { name: "q3.pdf".into(), mime: "application/pdf".into(), size: None, source: Source::Telegram { file_id: "D1".into() } }];
        // A platform retry of a message we already logged is not downloaded again.
        ingest_for_event(http.clone(), &cloud(&http, Some("t")), &Fetcher::Telegram { token: "1:a".into() }, "telegram", &mut ev, &files, true).await;
        assert!(ev.text.is_none() && take_kept("telegram", "9:1").is_none() && http.posts.lock().unwrap().is_empty());
        ingest_for_event(http.clone(), &cloud(&http, Some("t")), &Fetcher::Telegram { token: "1:a".into() }, "telegram", &mut ev, &files, false).await;
        assert_eq!(ev.text.as_deref(), Some("[file: q3.pdf]"));
        assert_eq!(take_kept("telegram", "9:1").unwrap()[0]["name"], "q3.pdf");
        assert!(take_kept("telegram", "9:1").is_none());
        // A message with its own text keeps it.
        let mut with_text = ev.clone();
        with_text.text = Some("see attached".into());
        with_text.remote_id = "9:2".into();
        ingest_for_event(http.clone(), &cloud(&http, Some("t")), &Fetcher::Telegram { token: "1:a".into() }, "telegram", &mut with_text, &files, false).await;
        assert_eq!(with_text.text.as_deref(), Some("see attached"));
        discard_kept("telegram", "9:2");
        assert!(take_kept("telegram", "9:2").is_none());
    }
}
