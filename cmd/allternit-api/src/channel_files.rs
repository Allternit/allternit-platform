//! Attachments on outbound channel messages: the shape a client sends, the
//! per-platform size caps, and the multipart body builder for platforms that
//! take a file upload (Telegram).
//!
//! A client attaches `{ filename, mimeType, dataBase64 }` (`name`/`mime`/`type`
//! and `contentBase64`/`data` are accepted as well). Files are checked before
//! any thread or send exists, so a refused attachment leaves nothing behind.
//!
//! Caps (decoded bytes) are the smaller of what we carry and what the platform
//! allows: Telegram `sendDocument`/`sendPhoto` 50 MB / 10 MB, Slack
//! `files.getUploadURLExternal` 1 GB, mailflare 10 MB per file, 20 MB per
//! message, 10 files. Platforms with no entry here do not take attachments
//! from a bot yet and answer `attachments_unsupported`.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;

pub const MAX_EACH_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
/// Request bodies carrying base64 files: 20 MB of files is ~27 MB encoded.
pub const MAX_REQUEST_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelFile {
    pub filename: String,
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileError {
    /// This provider takes no attachments from a bot.
    Unsupported,
    Invalid(String),
    TooMany(usize),
    TooBig(String),
}

impl FileError {
    pub fn code(&self) -> &'static str {
        match self {
            FileError::Unsupported => "attachments_unsupported",
            FileError::Invalid(_) => "invalid_attachment",
            FileError::TooMany(_) | FileError::TooBig(_) => "attachment_too_large",
        }
    }
    pub fn sentence(&self, place: &str) -> String {
        match self {
            FileError::Unsupported => format!("Attachments can't be sent in a {place} conversation yet."),
            FileError::Invalid(why) => why.clone(),
            FileError::TooMany(max) => format!("{place} takes at most {max} attachments in one message."),
            FileError::TooBig(why) => why.clone(),
        }
    }
}

/// How many files a message to `provider` may carry; `None` when it takes none.
pub fn max_files(provider: &str) -> Option<usize> {
    match provider {
        "telegram" | "slack" => Some(5),
        "email" => Some(10),
        // No MMS: each file is uploaded to cloud storage and its permanent link is added to the text.
        "sms" => Some(3),
        _ => None,
    }
}

fn clean_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let name: String = base.chars().filter(|c| !c.is_control()).take(120).collect();
    let name = name.trim().trim_start_matches('.').to_string();
    if name.is_empty() { "attachment".into() } else { name }
}

fn clean_mime(raw: &str) -> String {
    let m = raw.trim().to_lowercase();
    let ok = m.len() <= 100 && m.split_once('/').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty()) && m.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-".contains(c));
    if ok { m } else { "application/octet-stream".into() }
}

fn text_of<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).filter(|s| !s.is_empty())
}

/// Decode and check `raw` for a message to `provider`. An empty list is fine.
pub fn parse(provider: &str, raw: &[Value]) -> Result<Vec<ChannelFile>, FileError> {
    if raw.is_empty() {
        return Ok(vec![]);
    }
    let max = max_files(provider).ok_or(FileError::Unsupported)?;
    if raw.len() > max {
        return Err(FileError::TooMany(max));
    }
    let mut files = Vec::with_capacity(raw.len());
    let mut total = 0usize;
    for (i, a) in raw.iter().enumerate() {
        let encoded = text_of(a, &["dataBase64", "contentBase64", "data"]).ok_or_else(|| FileError::Invalid(format!("Attachment {} has no file data.", i + 1)))?;
        // A data: URL's prefix is not part of the bytes.
        let encoded = encoded.split_once(";base64,").map_or(encoded, |(_, rest)| rest);
        let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
        // Bound the work before decoding: base64 is 4 bytes of text per 3 of file.
        if compact.len() / 4 * 3 > MAX_EACH_BYTES + 3 {
            return Err(FileError::TooBig(format!("Attachment {} is larger than {} MB.", i + 1, MAX_EACH_BYTES / 1024 / 1024)));
        }
        let data = STANDARD.decode(compact.as_bytes()).map_err(|_| FileError::Invalid(format!("Attachment {} isn't valid base64.", i + 1)))?;
        if data.is_empty() {
            return Err(FileError::Invalid(format!("Attachment {} is empty.", i + 1)));
        }
        if data.len() > MAX_EACH_BYTES {
            return Err(FileError::TooBig(format!("Attachment {} is larger than {} MB.", i + 1, MAX_EACH_BYTES / 1024 / 1024)));
        }
        total += data.len();
        if total > MAX_TOTAL_BYTES {
            return Err(FileError::TooBig(format!("Attachments together are larger than {} MB.", MAX_TOTAL_BYTES / 1024 / 1024)));
        }
        files.push(ChannelFile {
            filename: clean_filename(text_of(a, &["filename", "name"]).unwrap_or("attachment")),
            mime: clean_mime(text_of(a, &["mimeType", "mime", "type"]).unwrap_or("")),
            data,
        });
    }
    Ok(files)
}

/// A `multipart/form-data` body: the text `fields`, then one file part.
/// Returns `(content type with boundary, body)`.
pub fn multipart(fields: &[(String, String)], file_field: &str, file: &ChannelFile) -> (String, Vec<u8>) {
    let boundary = format!("allternit-{}", uuid::Uuid::new_v4().simple());
    let quote = |s: &str| s.replace(['"', '\r', '\n'], "_");
    let mut body = Vec::with_capacity(file.data.len() + 512);
    for (k, v) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{v}\r\n", quote(k)).as_bytes());
    }
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n", quote(file_field), quote(&file.filename), file.mime).as_bytes(),
    );
    body.extend_from_slice(&file.data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// `{ filename, mimeType, dataBase64 }` for each file: the cloud routes' shape.
pub fn to_json(files: &[ChannelFile]) -> Vec<Value> {
    files.iter().map(|f| serde_json::json!({ "filename": f.filename, "mimeType": f.mime, "dataBase64": STANDARD.encode(&f.data) })).collect()
}

pub fn names(files: &[ChannelFile]) -> Vec<String> {
    files.iter().map(|f| f.filename.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(name: &str, bytes: &[u8]) -> Value {
        json!({ "filename": name, "mimeType": "image/png", "dataBase64": STANDARD.encode(bytes) })
    }

    #[test]
    fn files_decode_and_names_are_cleaned() {
        let f = parse("telegram", &[one("../../etc/pass wd.png", b"abc"), json!({ "name": "n.txt", "type": "text/plain", "contentBase64": "aGk=" })]).unwrap();
        assert_eq!(f[0], ChannelFile { filename: "pass wd.png".into(), mime: "image/png".into(), data: b"abc".to_vec() });
        assert_eq!((f[1].filename.as_str(), f[1].mime.as_str(), f[1].data.as_slice()), ("n.txt", "text/plain", b"hi".as_slice()));
        assert_eq!(parse("telegram", &[json!({ "dataBase64": "data:image/png;base64,YWJj", "mimeType": "bad mime" })]).unwrap()[0].mime, "application/octet-stream");
        assert!(parse("sms", &[]).unwrap().is_empty());
    }

    #[test]
    fn limits_and_unsupported_providers_say_why() {
        assert_eq!(parse("sms", &[one("a.png", b"x")]), Err(FileError::Unsupported));
        assert_eq!(parse("discord", &[one("a.png", b"x")]), Err(FileError::Unsupported));
        let six: Vec<Value> = (0..6).map(|i| one(&format!("{i}.png"), b"x")).collect();
        assert_eq!(parse("slack", &six), Err(FileError::TooMany(5)));
        assert!(parse("email", &six).is_ok());
        assert!(matches!(parse("slack", &[json!({ "filename": "a" })]), Err(FileError::Invalid(_))));
        assert!(matches!(parse("slack", &[json!({ "dataBase64": "!!!" })]), Err(FileError::Invalid(_))));
        assert!(matches!(parse("slack", &[json!({ "dataBase64": "" })]), Err(FileError::Invalid(_))));
        let big = vec![0u8; MAX_EACH_BYTES + 1];
        assert!(matches!(parse("telegram", &[one("big.bin", &big)]), Err(FileError::TooBig(_))));
        let ten = vec![1u8; MAX_EACH_BYTES];
        let err = parse("email", &[one("a", &ten), one("b", &ten), one("c", &ten)]).unwrap_err();
        assert_eq!(err.code(), "attachment_too_large");
        assert!(err.sentence("email").contains("together"));
    }

    #[test]
    fn multipart_carries_fields_then_the_file() {
        let (ct, body) = multipart(&[("chat_id".into(), "42".into())], "document", &ChannelFile { filename: "a\"b.txt".into(), mime: "text/plain".into(), data: b"HELLO".to_vec() });
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n42\r\n")));
        assert!(text.contains("name=\"document\"; filename=\"a_b.txt\"\r\nContent-Type: text/plain\r\n\r\nHELLO\r\n"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }
}
