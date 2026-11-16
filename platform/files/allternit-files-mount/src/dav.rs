//! WebDAV frontend (RFC 4918 class 1, with class 2 locks answered but not
//! enforced), served on loopback only. It is the fallback mount where FUSE is
//! missing: `davfs2`, GNOME/KDE file managers (`dav://`), `rclone`, or any
//! WebDAV client.
//!
//! - **Loopback and a secret path.** The server binds 127.0.0.1 and every URL
//!   starts with a random secret (`http://127.0.0.1:PORT/<secret>/`), so other
//!   users on the same machine can't browse it by guessing the port.
//! - **Locks.** macOS and Windows clients refuse to write without LOCK. The
//!   Files drive has no locks; it protects edits with revisions instead (a
//!   change made elsewhere lands as a conflict copy). So LOCK hands out a token
//!   and UNLOCK accepts it; nothing is held.
//! - Backend calls block, so each runs in `spawn_blocking`.

use std::io::Write as _;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures::StreamExt;

use crate::backend::MountError;
use crate::vfs::{Entry, Vfs, BLOCK};

#[derive(Clone)]
struct Dav {
    vfs: Arc<Vfs>,
    /// URL prefix, `/<secret>`.
    prefix: Arc<str>,
}

/// Serve WebDAV for `vfs` on `listener` under `/<secret>/` until the process stops.
pub async fn serve(vfs: Arc<Vfs>, listener: tokio::net::TcpListener, secret: &str) -> std::io::Result<()> {
    let state = Dav { vfs, prefix: Arc::from(format!("/{secret}")) };
    let app = Router::new().fallback(handle).with_state(state);
    axum::serve(listener, app).await
}

/// Runs a blocking VFS call off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, MountError> + Send + 'static) -> Result<T, MountError> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|_| Err(MountError::new("internal", 500, "worker stopped")))
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY)
}

fn fail(e: MountError) -> Response {
    let mut res = (status(e.http_status()), e.message.clone()).into_response();
    if let Some(s) = e.retry_after_s {
        if let Ok(v) = HeaderValue::from_str(&s.to_string()) {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
    res
}

fn plain(code: StatusCode) -> Response {
    code.into_response()
}

/// Path segments after the secret prefix, percent-decoded. None when the
/// path is outside the prefix or a segment is `.`/`..`.
pub fn segments(path: &str, prefix: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix(prefix)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let mut out = Vec::new();
    for raw in rest.split('/').filter(|s| !s.is_empty()) {
        let s = urlencoding::decode(raw).ok()?.into_owned();
        if s == "." || s == ".." || s.contains('/') || s.contains('\0') {
            return None;
        }
        out.push(s);
    }
    Some(out)
}

/// The path part of a `Destination` header (absolute URL or path).
pub fn destination_path(dest: &str) -> Option<String> {
    let p = match dest.find("://") {
        Some(i) => {
            let after = &dest[i + 3..];
            &after[after.find('/')?..]
        }
        None => dest,
    };
    Some(p.split(['?', '#']).next().unwrap_or(p).to_string())
}

/// A single `bytes=a-b` / `bytes=a-` / `bytes=-n` range over `size` bytes,
/// as (start, end inclusive). None for no range or one we don't serve.
pub fn parse_range(h: &str, size: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (size.saturating_sub(n), size - 1)
        }
        (s, "") => (s.parse().ok()?, size - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(size - 1)),
    };
    (start <= end && start < size).then_some((start, end))
}

fn href(prefix: &str, segs: &[String], dir: bool) -> String {
    let mut s = prefix.to_string();
    for seg in segs {
        s.push('/');
        s.push_str(&urlencoding::encode(seg));
    }
    if dir || segs.is_empty() {
        s.push('/');
    }
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn http_date(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn prop_response(prefix: &str, segs: &[String], e: &Entry) -> String {
    let mut p = format!("<D:displayname>{}</D:displayname>", xml_escape(&e.name));
    if e.is_dir {
        p.push_str("<D:resourcetype><D:collection/></D:resourcetype>");
    } else {
        p.push_str("<D:resourcetype/>");
        p.push_str(&format!("<D:getcontentlength>{}</D:getcontentlength>", e.size));
        p.push_str(&format!("<D:getcontenttype>{}</D:getcontenttype>", xml_escape(e.mime.as_deref().unwrap_or("application/octet-stream"))));
    }
    p.push_str(&format!("<D:getlastmodified>{}</D:getlastmodified>", http_date(e.modified)));
    if let Some(item) = &e.item {
        p.push_str(&format!("<D:creationdate>{}</D:creationdate>", xml_escape(&item.created_at)));
        p.push_str(&format!("<D:getetag>\"{}\"</D:getetag>", xml_escape(&item.rev)));
    }
    p.push_str("<D:supportedlock><D:lockentry><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockentry></D:supportedlock>");
    format!(
        "<D:response><D:href>{}</D:href><D:propstat><D:prop>{p}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        xml_escape(&href(prefix, segs, e.is_dir))
    )
}

fn multistatus(body: String) -> Response {
    let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">{body}</D:multistatus>");
    Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(xml))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR))
}

async fn handle(State(dav): State<Dav>, req: Request) -> Response {
    let Some(segs) = segments(req.uri().path(), &dav.prefix) else {
        return plain(StatusCode::NOT_FOUND);
    };
    let method = req.method().clone();
    match method.as_str() {
        "OPTIONS" => Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 2")
            .header("MS-Author-Via", "DAV")
            .header(header::ALLOW, "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK")
            .body(Body::empty())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR)),
        "PROPFIND" => propfind(dav, segs, req.headers()).await,
        "PROPPATCH" => {
            // Clients set times and Finder flags; the drive keeps its own, so accept and ignore.
            let href = href(&dav.prefix, &segs, false);
            multistatus(format!(
                "<D:response><D:href>{}</D:href><D:propstat><D:prop/><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
                xml_escape(&href)
            ))
        }
        "GET" | "HEAD" => get(dav, segs, req.headers(), method == Method::HEAD).await,
        "PUT" => put(dav, segs, req).await,
        "DELETE" => delete(dav, segs).await,
        "MKCOL" => mkcol(dav, segs, req).await,
        "MOVE" | "COPY" => move_or_copy(dav, segs, req.headers(), method.as_str() == "MOVE").await,
        "LOCK" => lock(dav, segs, req.headers()),
        "UNLOCK" => plain(StatusCode::NO_CONTENT),
        _ => plain(StatusCode::METHOD_NOT_ALLOWED),
    }
}

async fn resolve(dav: &Dav, segs: &[String]) -> Result<Option<Entry>, MountError> {
    let vfs = dav.vfs.clone();
    let segs = segs.to_vec();
    blocking(move || vfs.resolve(&segs)).await
}

async fn propfind(dav: Dav, segs: Vec<String>, headers: &HeaderMap) -> Response {
    let depth0 = headers.get("depth").and_then(|v| v.to_str().ok()).map(|d| d.trim() == "0").unwrap_or(false);
    let entry = match resolve(&dav, &segs).await {
        Ok(Some(e)) => e,
        Ok(None) => return plain(StatusCode::NOT_FOUND),
        Err(e) => return fail(e),
    };
    let mut body = prop_response(&dav.prefix, &segs, &entry);
    if entry.is_dir && !depth0 {
        let vfs = dav.vfs.clone();
        let dir = entry.clone();
        let kids = match blocking(move || vfs.children(&dir)).await {
            Ok(k) => k,
            Err(e) => return fail(e),
        };
        for k in kids {
            let mut child = segs.clone();
            child.push(k.name.clone());
            body.push_str(&prop_response(&dav.prefix, &child, &k));
        }
    }
    multistatus(body)
}

async fn get(dav: Dav, segs: Vec<String>, headers: &HeaderMap, head: bool) -> Response {
    let entry = match resolve(&dav, &segs).await {
        Ok(Some(e)) => e,
        Ok(None) => return plain(StatusCode::NOT_FOUND),
        Err(e) => return fail(e),
    };
    if entry.is_dir {
        let vfs = dav.vfs.clone();
        let dir = entry.clone();
        let kids = match blocking(move || vfs.children(&dir)).await {
            Ok(k) => k,
            Err(e) => return fail(e),
        };
        let mut html = String::from("<!doctype html><meta charset=\"utf-8\"><title>Allternit Files</title><ul>");
        for k in kids {
            let mut child = segs.clone();
            child.push(k.name.clone());
            html.push_str(&format!("<li><a href=\"{}\">{}{}</a></li>", xml_escape(&href(&dav.prefix, &child, k.is_dir)), xml_escape(&k.name), if k.is_dir { "/" } else { "" }));
        }
        html.push_str("</ul>");
        return ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    }
    let size = entry.size;
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|r| parse_range(r, size));
    let (start, end, code) = match range {
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => (0, size.saturating_sub(1), StatusCode::OK),
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    let mut b = Response::builder()
        .status(code)
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, entry.mime.clone().unwrap_or_else(|| "application/octet-stream".into()))
        .header(header::LAST_MODIFIED, http_date(entry.modified));
    if let Some(tag) = entry.etag() {
        b = b.header(header::ETAG, format!("\"{tag}\""));
    }
    if code == StatusCode::PARTIAL_CONTENT {
        b = b.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if head || len == 0 {
        return b.body(Body::empty()).unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR));
    }
    // Stream in BLOCK pieces; a large file is never held in memory.
    let vfs = dav.vfs.clone();
    let stop = start + len;
    let stream = futures::stream::unfold(start, move |pos| {
        let vfs = vfs.clone();
        let entry = entry.clone();
        async move {
            if pos >= stop {
                return None;
            }
            let n = BLOCK.min(stop - pos);
            let piece = blocking(move || vfs.read(&entry, pos, n)).await;
            match piece {
                Ok(data) if !data.is_empty() => {
                    let next = pos + data.len() as u64;
                    Some((Ok::<Bytes, std::io::Error>(Bytes::from(data)), next))
                }
                Ok(_) => None,
                Err(e) => Some((Err(std::io::Error::other(e.to_string())), stop)),
            }
        }
    });
    b.body(Body::from_stream(stream)).unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Parent folder and leaf name of a path, both resolved.
async fn parent_and_name(dav: &Dav, segs: &[String]) -> Result<Option<(Entry, String, Option<Entry>)>, MountError> {
    let Some((name, parent)) = segs.split_last() else {
        return Ok(None);
    };
    let vfs = dav.vfs.clone();
    let parent = parent.to_vec();
    let name = name.clone();
    blocking(move || {
        let Some(dir) = vfs.resolve(&parent)? else { return Ok(None) };
        if !dir.is_dir {
            return Ok(None);
        }
        let existing = vfs.child(&dir, &name)?;
        Ok(Some((dir, name, existing)))
    })
    .await
}

async fn put(dav: Dav, segs: Vec<String>, req: Request) -> Response {
    let (dir, name, existing) = match parent_and_name(&dav, &segs).await {
        Ok(Some(t)) => t,
        Ok(None) => return plain(StatusCode::CONFLICT),
        Err(e) => return fail(e),
    };
    if existing.as_ref().is_some_and(|e| e.is_dir) {
        return plain(StatusCode::METHOD_NOT_ALLOWED);
    }
    if dav.vfs.read_only() {
        return fail(MountError::read_only());
    }
    // Spool the body to a temp file: uploads are chunked from disk, never held in memory.
    let mut tmp = match tempfile::NamedTempFile::new() {
        Ok(t) => t,
        Err(e) => return fail(MountError::io(e)),
    };
    let mut body = req.into_body().into_data_stream();
    while let Some(piece) = body.next().await {
        match piece {
            Ok(bytes) => {
                if let Err(e) = tmp.write_all(&bytes) {
                    return fail(MountError::io(e));
                }
            }
            Err(_) => return plain(StatusCode::BAD_REQUEST),
        }
    }
    if let Err(e) = tmp.flush() {
        return fail(MountError::io(e));
    }
    let created = existing.is_none();
    let vfs = dav.vfs.clone();
    match blocking(move || vfs.write(&dir, &name, existing.as_ref(), tmp.path())).await {
        Ok(_) => plain(if created { StatusCode::CREATED } else { StatusCode::NO_CONTENT }),
        Err(e) => fail(e),
    }
}

async fn delete(dav: Dav, segs: Vec<String>) -> Response {
    let (dir, _, existing) = match parent_and_name(&dav, &segs).await {
        Ok(Some(t)) => t,
        Ok(None) => return plain(StatusCode::FORBIDDEN),
        Err(e) => return fail(e),
    };
    let Some(entry) = existing else { return plain(StatusCode::NOT_FOUND) };
    let vfs = dav.vfs.clone();
    match blocking(move || vfs.delete(&entry, &dir)).await {
        Ok(()) => plain(StatusCode::NO_CONTENT),
        Err(e) => fail(e),
    }
}

async fn mkcol(dav: Dav, segs: Vec<String>, req: Request) -> Response {
    let has_body = req.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).is_some_and(|v| v != "0");
    if has_body {
        return plain(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let (dir, name, existing) = match parent_and_name(&dav, &segs).await {
        Ok(Some(t)) => t,
        Ok(None) => return plain(StatusCode::CONFLICT),
        Err(e) => return fail(e),
    };
    if existing.is_some() {
        return plain(StatusCode::METHOD_NOT_ALLOWED);
    }
    let vfs = dav.vfs.clone();
    match blocking(move || vfs.mkdir(&dir, &name)).await {
        Ok(_) => plain(StatusCode::CREATED),
        Err(e) => fail(e),
    }
}

async fn move_or_copy(dav: Dav, segs: Vec<String>, headers: &HeaderMap, is_move: bool) -> Response {
    let Some(dest) = headers.get("destination").and_then(|v| v.to_str().ok()).and_then(destination_path) else {
        return plain(StatusCode::BAD_REQUEST);
    };
    let Some(dest_segs) = segments(&dest, &dav.prefix) else {
        return plain(StatusCode::BAD_GATEWAY);
    };
    let overwrite = headers.get("overwrite").and_then(|v| v.to_str().ok()).map(|v| !v.trim().eq_ignore_ascii_case("F")).unwrap_or(true);
    let (from_dir, _, src) = match parent_and_name(&dav, &segs).await {
        Ok(Some(t)) => t,
        Ok(None) => return plain(StatusCode::FORBIDDEN),
        Err(e) => return fail(e),
    };
    let Some(src) = src else { return plain(StatusCode::NOT_FOUND) };
    let (to_dir, to_name, existing) = match parent_and_name(&dav, &dest_segs).await {
        Ok(Some(t)) => t,
        Ok(None) => return plain(StatusCode::CONFLICT),
        Err(e) => return fail(e),
    };
    if existing.is_some() && !overwrite {
        return plain(StatusCode::PRECONDITION_FAILED);
    }
    let replaced = existing.is_some();
    let vfs = dav.vfs.clone();
    let result = blocking(move || {
        if is_move {
            vfs.rename(&src, &from_dir, &to_dir, &to_name, overwrite)
        } else {
            vfs.copy(&src, &to_dir, &to_name, overwrite)
        }
    })
    .await;
    match result {
        Ok(_) => plain(if replaced { StatusCode::NO_CONTENT } else { StatusCode::CREATED }),
        Err(e) => fail(e),
    }
}

fn lock(dav: Dav, segs: Vec<String>, headers: &HeaderMap) -> Response {
    // A refresh names its token in `If`; otherwise hand out a new one.
    let token = headers
        .get("if")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split('<').nth(1).and_then(|t| t.split('>').next()).map(str::to_string))
        .unwrap_or_else(|| format!("opaquelocktoken:{:032x}", rand::random::<u128>()));
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock>\
         <D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>0</D:depth>\
         <D:timeout>Second-3600</D:timeout><D:locktoken><D:href>{}</D:href></D:locktoken>\
         <D:lockroot><D:href>{}</D:href></D:lockroot></D:activelock></D:lockdiscovery></D:prop>",
        xml_escape(&token),
        xml_escape(&href(&dav.prefix, &segs, false))
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .header("Lock-Token", format!("<{token}>"))
        .body(Body::from(xml))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_decode_and_refuse_escapes() {
        assert_eq!(segments("/s3cr3t/Inbox/Lease%20draft.pdf", "/s3cr3t"), Some(vec!["Inbox".to_string(), "Lease draft.pdf".to_string()]));
        assert_eq!(segments("/s3cr3t", "/s3cr3t"), Some(vec![]));
        // Not under the secret, or a prefix that only shares characters.
        assert_eq!(segments("/other/Inbox", "/s3cr3t"), None);
        assert_eq!(segments("/s3cr3tX/Inbox", "/s3cr3t"), None);
        // Dot segments and encoded slashes never reach the VFS.
        assert_eq!(segments("/s3cr3t/Inbox/../x", "/s3cr3t"), None);
        assert_eq!(segments("/s3cr3t/a%2Fb", "/s3cr3t"), None);
    }

    #[test]
    fn href_round_trips_through_segments() {
        let segs = vec!["Agent Output".to_string(), "r&d #1.md".to_string()];
        let h = href("/k", &segs, false);
        assert_eq!(segments(&h, "/k"), Some(segs));
    }

    #[test]
    fn destination_header_forms() {
        assert_eq!(destination_path("http://127.0.0.1:4711/k/Inbox/a.txt").as_deref(), Some("/k/Inbox/a.txt"));
        assert_eq!(destination_path("/k/Inbox/a.txt?x=1").as_deref(), Some("/k/Inbox/a.txt"));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        // End past the file is clamped; start past it is not served.
        assert_eq!(parse_range("bytes=990-2000", 1000), Some((990, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
    }
}
