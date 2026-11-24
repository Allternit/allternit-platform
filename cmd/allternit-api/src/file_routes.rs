use axum::{
    extract::{ConnectInfo, Query},
    http::{header::CONTENT_TYPE, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::files_scope::{denied_response, Access, Caller, FilesScope, OpenError, WholeDiskGrant};
use crate::AppState;

pub fn file_router() -> Router<Arc<AppState>> {
    file_routes(Arc::new(FilesScope::from_env()))
}

/// The `/files/*` routes, checked against `scope`. Generic over the router
/// state because no handler reads it, so tests can mount the routes on a
/// temporary home folder without building an `AppState`.
pub fn file_routes<S: Clone + Send + Sync + 'static>(scope: Arc<FilesScope>) -> Router<S> {
    Router::new()
        .route("/files/list", get(list_files))
        .route("/files/read", get(read_file))
        .route("/files/raw", get(raw_file))
        .route("/files/exists", get(file_exists_json).head(file_exists))
        .route("/files/mkdir", post(mkdir))
        .route("/files/delete", delete(delete_file))
        .route("/files/write", post(write_file))
        .route("/files/bot-writes", post(record_bot_write))
        // Whole-disk files (step 2.11): grants only a signed relayed request.
        .layer(axum::middleware::from_fn(crate::files_scope::whole_disk_gate))
        .layer(Extension(scope))
}

/// Shared scope plus who is asking, extracted once per request.
type Scope = Extension<Arc<FilesScope>>;
type Peer = Option<ConnectInfo<SocketAddr>>;
/// Set by `files_scope::whole_disk_gate` for a signed whole-disk request.
type Grant = Option<Extension<WholeDiskGrant>>;

fn caller_of(headers: &HeaderMap, peer: &Peer, grant: &Grant) -> Caller {
    crate::files_scope::caller_with_grant(headers, peer.as_ref().map(|c| c.0), grant.is_some())
}

/// Resolve `raw` for this request, or the `403` to answer with. Auth decides
/// who the caller is; this decides which paths that caller may touch (local
/// Desktop: home minus secrets; relayed or remote: registered roots only).
/// See `files_scope.rs`.
fn scoped(
    scope: &FilesScope,
    headers: &HeaderMap,
    peer: &Peer,
    grant: &Grant,
    raw: &str,
    access: Access,
) -> Result<(PathBuf, Caller), Response> {
    let caller = caller_of(headers, peer, grant);
    scope
        .authorize(raw, caller, access)
        .map(|path| (path, caller))
        .map_err(denied_response)
}

/// Open `path` (already authorized) and read it whole, checking after the
/// open that the file really is inside the scope (it can't have been swapped
/// for a symlink between the check and the open).
async fn read_scoped(scope: Arc<FilesScope>, path: PathBuf, caller: Caller) -> Result<Vec<u8>, Response> {
    let result = tokio::task::spawn_blocking(move || {
        let mut file = scope.open_verified(&path, caller, Access::Read)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).map_err(OpenError::Io)?;
        Ok::<_, OpenError>(bytes)
    })
    .await;
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(OpenError::Denied(denied))) => Err(denied_response(denied)),
        _ => Err((StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response()),
    }
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

#[derive(Deserialize)]
struct ListQuery {
    path: String,
    recursive: Option<String>,
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
    size: Option<u64>,
    modified_at: Option<String>,
}

/// Caps for recursive listings: agent workspaces are tiny, but `/files/list`
/// is a general endpoint and must not walk $HOME or node_modules forever.
const MAX_LIST_DEPTH: usize = 8;
const MAX_LIST_ENTRIES: usize = 1000;

async fn file_entry_for(path: &std::path::Path, name: String) -> FileEntry {
    let entry_type = if tokio::fs::metadata(path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        "directory"
    } else {
        "file"
    };
    let (size, modified_at) = match tokio::fs::metadata(path).await {
        Ok(m) => (
            Some(m.len()),
            m.modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
        ),
        Err(_) => (None, None),
    };
    FileEntry {
        name,
        path: path.to_string_lossy().to_string(),
        entry_type: entry_type.to_string(),
        size,
        modified_at,
    }
}

async fn list_files(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<ListQuery>,
) -> Response {
    let recursive = params.recursive.as_deref() == Some("true");
    let caller = caller_of(&headers, &peer, &grant);
    let resolved = match scope.authorize_list(&params.path, caller, recursive) {
        Ok(path) => path,
        Err(denied) => return denied_response(denied),
    };
    let _meta = match tokio::fs::metadata(&resolved).await {
        Ok(m) if m.is_dir() => m,
        _ => {
            // A missing/unreadable candidate dir is a normal outcome for the
            // capability scanner (it probes many optional locations). Answer
            // 200 with an empty listing — a 404 adds nothing but console noise.
            return (
                StatusCode::OK,
                Json(json!({"path": resolved.to_string_lossy().to_string(), "entries": []})),
            )
                .into_response();
        }
    };

    let mut entries = vec![];
    // (dir, depth) work stack; iterative so recursion depth stays bounded.
    let mut stack = vec![(resolved.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(d) => d,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            if entries.len() >= MAX_LIST_ENTRIES {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            // Keep recursive listings out of secret folders (and, for a
            // relayed caller, inside the allowed tree).
            if !scope.listable(&path, caller) {
                continue;
            }
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(file_entry_for(&path, name).await);
            if recursive && is_dir && depth + 1 < MAX_LIST_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }

    (
        StatusCode::OK,
        Json(json!({"path": resolved.to_string_lossy().to_string(), "entries": entries})),
    )
        .into_response()
}

async fn read_file(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<PathQuery>,
) -> Response {
    let (resolved, caller) = match scoped(&scope, &headers, &peer, &grant, &params.path, Access::Read) {
        Ok(v) => v,
        Err(denied) => return denied,
    };
    match read_scoped(scope, resolved, caller).await.map(String::from_utf8) {
        Ok(Ok(content)) => (StatusCode::OK, Json(json!({"content": content}))).into_response(),
        Err(denied) if denied.status() == StatusCode::FORBIDDEN => denied,
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Cannot read file"})),
        )
            .into_response(),
    }
}

async fn raw_file(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<PathQuery>,
) -> Response {
    let (resolved, caller) = match scoped(&scope, &headers, &peer, &grant, &params.path, Access::Read) {
        Ok(v) => v,
        Err(denied) => return denied,
    };
    let mime = mime_for(&resolved);
    match read_scoped(scope, resolved, caller).await {
        Ok(bytes) => (StatusCode::OK, [(CONTENT_TYPE, mime)], bytes).into_response(),
        Err(response) => response,
    }
}

async fn file_exists(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<PathQuery>,
) -> Response {
    let resolved = match scoped(&scope, &headers, &peer, &grant, &params.path, Access::Read) {
        Ok((path, _)) => path,
        Err(denied) => return denied,
    };
    if resolved.exists() {
        StatusCode::OK.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// JSON variant of the existence probe. A HEAD 404 is the only way to signal
/// "missing" over HEAD, and every such 404 shows up as a console error in the
/// browser — which made routine probe-then-create flows (agent bootstrap)
/// look broken. GET always answers 200 and carries the answer in the body.
async fn file_exists_json(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<PathQuery>,
) -> Response {
    let resolved = match scoped(&scope, &headers, &peer, &grant, &params.path, Access::Read) {
        Ok((path, _)) => path,
        Err(denied) => return denied,
    };
    Json(json!({"exists": resolved.exists()})).into_response()
}

async fn mkdir(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<PathQuery>,
) -> Response {
    let resolved = match scoped(&scope, &headers, &peer, &grant, &params.path, Access::Write) {
        Ok((path, _)) => path,
        Err(denied) => return denied,
    };
    match tokio::fs::create_dir_all(&resolved).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot create directory"})),
        )
            .into_response(),
    }
}

async fn delete_file(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<PathQuery>,
) -> Response {
    let resolved = match scoped(&scope, &headers, &peer, &grant, &params.path, Access::Delete) {
        Ok((path, _)) => path,
        Err(denied) => return denied,
    };
    // symlink_metadata: the scope left a symlink at the target unresolved,
    // so a link is removed as a link and never what it points at.
    let removed = match tokio::fs::symlink_metadata(&resolved).await {
        Ok(m) if m.is_dir() => tokio::fs::remove_dir_all(&resolved).await,
        Ok(_) => tokio::fs::remove_file(&resolved).await,
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response()
        }
    };
    match removed {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot delete"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct WriteQuery {
    path: Option<String>,
}

#[derive(Deserialize)]
struct WriteBody {
    path: Option<String>,
    content: String,
}

async fn write_file(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Query(params): Query<WriteQuery>,
    Json(body): Json<WriteBody>,
) -> Response {
    let target = match body.path.or(params.path) {
        Some(path) => path,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Missing path"})),
            )
                .into_response()
        }
    };
    let (resolved, caller) = match scoped(&scope, &headers, &peer, &grant, &target, Access::Write) {
        Ok(v) => v,
        Err(denied) => return denied,
    };

    let bytes_written = body.content.len();
    let path = resolved.clone();
    let written = tokio::task::spawn_blocking(move || {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Opened without truncating, checked, and only then emptied, so a
        // file swapped in under us is never cut short.
        let mut file = scope.open_verified(&path, caller, Access::Write)?;
        file.set_len(0).map_err(OpenError::Io)?;
        std::io::Write::write_all(&mut file, body.content.as_bytes()).map_err(OpenError::Io)
    })
    .await;
    match written {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "path": resolved.to_string_lossy().to_string(),
                "bytesWritten": bytes_written,
                "operation": "write",
            })),
        )
            .into_response(),
        Ok(Err(OpenError::Denied(denied))) => denied_response(denied),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot write file"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct BotWriteBody {
    path: String,
}

/// gizzi on this computer reports a file one of its tools just wrote
/// (`File.Event.Edited`). Local callers only: a relayed client can never
/// add a path. See `files_scope::BotWriteLedger`.
async fn record_bot_write(
    Extension(scope): Scope,
    peer: Peer,
    grant: Grant,
    headers: HeaderMap,
    Json(body): Json<BotWriteBody>,
) -> Response {
    let caller = caller_of(&headers, &peer, &grant);
    match scope.record_bot_write(&body.path, caller) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(denied) => denied_response(denied),
    }
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("pdf") => "application/pdf",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}
#[cfg(test)]
#[path = "file_routes_whole_disk_tests.rs"]
mod whole_disk_tests;

#[cfg(test)]
mod tests {
    //! Scoping of the legacy `/files/*` routes (Files drive plan, step 3).
    //! Every case drives the real routes over HTTP against a temporary home.
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        home: PathBuf,
        app: Router,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let write = |rel: &str, body: &str| {
            let path = home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write("Allternit/Inbox/note.txt", "hi");
        write(".agents/plugins/demo/plugin.json", "{}");
        write(".ssh/id_rsa", "SECRET");
        write(".claude/skills/s1/SKILL.md", "skill");
        write(".agents/skills/s2/SKILL.md", "skill");
        write(".agents/plugins/demo2/plugin.json", "{}");
        write(".config/gizzi/gizzi.json", "{}");
        write("AllternitX/secret.txt", "SECRET");
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/secret.txt"), "SECRET").unwrap();
        std::fs::create_dir_all(root.join("homeb")).unwrap();
        std::fs::write(root.join("homeb/secret.txt"), "SECRET").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside"), home.join("Allternit/escape")).unwrap();
            std::os::unix::fs::symlink(home.join(".agents/plugins/demo"), home.join(".claude/skills/linked")).unwrap();
        }
        let scope = Arc::new(FilesScope::new(Some(home.clone()), vec![home.join("Library/Application Support/allternit")]));
        Fixture { _tmp: tmp, root, home, app: file_routes(scope) }
    }

    fn q(path: &str) -> String {
        urlencoding::encode(path).into_owned()
    }

    async fn send(app: &Router, method: &str, uri: &str, relayed: bool, body: Option<serde_json::Value>) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(uri);
        if relayed {
            req = req.header("x-allternit-relayed", "1");
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn read(app: &Router, path: &str, relayed: bool) -> (StatusCode, String) {
        send(app, "GET", &format!("/files/read?path={}", q(path)), relayed, None).await
    }

    #[tokio::test]
    async fn relayed_callers_cannot_leave_the_registered_roots() {
        let f = fixture();
        let outside_prefix = f.root.join("homeb/secret.txt");
        for path in [
            "/etc/passwd".to_string(),
            "~/.ssh/id_rsa".to_string(),
            "~/Allternit/../.ssh/id_rsa".to_string(),
            "../outside/secret.txt".to_string(),
            "~/Allternit/escape/secret.txt".to_string(),
            "~/AllternitX/secret.txt".to_string(),
            outside_prefix.to_string_lossy().into_owned(),
            f.home.join(".ssh/id_rsa").to_string_lossy().into_owned(),
        ] {
            let (status, body) = read(&f.app, &path, true).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "relayed read of {path} answered {status}: {body}");
            assert!(!body.contains("SECRET"), "relayed read of {path} leaked the file");
            let (status, _) = send(&f.app, "GET", &format!("/files/raw?path={}", q(&path)), true, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "relayed raw of {path}");
        }
        let (status, _) = send(&f.app, "GET", &format!("/files/list?path={}", q("~/Allternit/escape")), true, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "relayed list through a symlink out of the root");
    }

    #[tokio::test]
    async fn relayed_case_tricks_do_not_unlock_read_only_or_secret_paths() {
        let f = fixture();
        let (status, _) = send(
            &f.app,
            "POST",
            "/files/write",
            true,
            Some(json!({"path": "~/.AGENTS/plugins/demo/evil.sh", "content": "pwned"})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(!f.home.join(".agents/plugins/demo/evil.sh").exists());
        let (status, _) = read(&f.app, "~/.SSH/id_rsa", true).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn relayed_callers_use_roots_plugins_and_heartbeat() {
        let f = fixture();
        let (status, body) = read(&f.app, "~/Allternit/Inbox/note.txt", true).await;
        assert_eq!((status, body.contains("\"hi\"")), (StatusCode::OK, true), "{body}");
        let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}", q("~/.agents/plugins")), true, None).await;
        assert_eq!((status, body.contains("\"demo\"")), (StatusCode::OK, true), "{body}");
        let (status, _) = read(&f.app, "~/.agents/plugins/demo/plugin.json", true).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(json!({"path": "~/Allternit/Inbox/new.txt", "content": "ok"}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(std::fs::read_to_string(f.home.join("Allternit/Inbox/new.txt")).unwrap(), "ok");
        let heartbeat = "agents/a1/.allternit/governance/HEARTBEAT.md";
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(json!({"path": heartbeat, "content": "# beat"}))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = read(&f.app, heartbeat, true).await;
        assert_eq!((status, body.contains("# beat")), (StatusCode::OK, true));
    }

    /// The relay is the authenticated owner: relayed callers keep the
    /// features the web, PWA and iOS use today (agent workspaces, skills,
    /// plugin enable/remove, the home probe), minus the secrets.
    #[tokio::test]
    async fn relayed_callers_keep_workspace_skill_and_plugin_features() {
        let f = fixture();
        let post = |path: &str| json!({"path": path, "content": "x"});
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(post("agents/a1/.allternit/manifest.json"))).await;
        assert_eq!(status, StatusCode::OK, "agent workspace write");
        let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}&recursive=true", q("agents/a1")), true, None).await;
        assert_eq!((status, body.contains("manifest.json")), (StatusCode::OK, true), "agent workspace list: {body}");
        let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}", q("~/.agents/skills")), true, None).await;
        assert_eq!((status, body.contains("\"s2\"")), (StatusCode::OK, true), "skills list: {body}");
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(post("~/.allternit/skills/new-skill/SKILL.md"))).await;
        assert_eq!(status, StatusCode::OK, "createSkill");
        let (status, _) = send(&f.app, "DELETE", &format!("/files/delete?path={}", q("~/.allternit/skills/new-skill")), true, None).await;
        assert_eq!(status, StatusCode::OK, "remove skill");
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(post("~/.agents/plugins/demo/config.json"))).await;
        assert_eq!(status, StatusCode::OK, "plugin setEnabled writes config.json");
        let (status, _) = send(&f.app, "DELETE", &format!("/files/delete?path={}", q("~/.agents/plugins/demo2")), true, None).await;
        assert_eq!(status, StatusCode::OK, "plugin remove deletes the plugin folder");
        assert!(!f.home.join(".agents/plugins/demo2").exists());
        let (status, _) = read(&f.app, "~/.config/gizzi/gizzi.json", true).await;
        assert_eq!(status, StatusCode::OK, "gizzi.json is readable");
        let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}", q("~")), true, None).await;
        assert_eq!(status, StatusCode::OK, "home probe: {body}");
        assert!(body.contains("\"Allternit\""), "{body}");
        assert!(!body.contains(".ssh"), "home probe showed .ssh: {body}");
    }

    #[tokio::test]
    async fn relayed_callers_still_cannot_reach_the_rest() {
        let f = fixture();
        for (method, uri, body) in [
            ("POST", "/files/write".to_string(), Some(json!({"path": "~/.agents/plugins/demo/evil.sh", "content": "x"}))),
            ("POST", "/files/write".to_string(), Some(json!({"path": "~/.config/gizzi/gizzi.json", "content": "x"}))),
            ("DELETE", format!("/files/delete?path={}", q("~/.agents/plugins")), None),
            ("GET", format!("/files/list?path={}&recursive=true", q("~")), None),
            ("GET", format!("/files/list?path={}", q("~/.ssh")), None),
            ("GET", format!("/files/read?path={}", q("/etc/passwd")), None),
        ] {
            let (status, text) = send(&f.app, method, &uri, true, body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {text}");
        }
    }

    #[tokio::test]
    async fn relayed_callers_open_shared_context_notes_and_nothing_else_there() {
        let f = fixture();
        let put = |rel: &str, body: &str| {
            let path = f.home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        put("Desktop/my-app/.allternit/shared-context.md", "notes");
        put("Desktop/my-app/.allternit/other.md", "SECRET");
        put("Desktop/my-app/package.json", "SECRET");
        put(".ssh/.allternit/shared-context.md", "SECRET");
        std::fs::create_dir_all(f.home.join("Desktop/linked/.allternit")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(f.root.join("outside/secret.txt"), f.home.join("Desktop/linked/.allternit/shared-context.md")).unwrap();

        let (status, body) = read(&f.app, "~/Desktop/my-app/.allternit/shared-context.md", true).await;
        assert_eq!((status, body.contains("notes")), (StatusCode::OK, true), "{body}");
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(json!({"path": "~/Desktop/my-app/.allternit/shared-context.md", "content": "edited"}))).await;
        assert_eq!(status, StatusCode::OK);
        for path in ["~/Desktop/my-app/.allternit/other.md", "~/.ssh/.allternit/shared-context.md", "~/Desktop/linked/.allternit/shared-context.md", "~/Desktop/my-app/package.json"] {
            let (status, body) = read(&f.app, path, true).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
            assert!(!body.contains("SECRET"));
        }
    }

    #[tokio::test]
    async fn relayed_callers_read_only_the_files_a_bot_wrote() {
        let f = fixture();
        std::fs::create_dir_all(f.home.join("Desktop")).unwrap();
        std::fs::write(f.home.join("Desktop/report.md"), "report").unwrap();
        std::fs::write(f.home.join("Desktop/other.md"), "SECRET").unwrap();
        let record = |path: &str| json!({"path": path, "session_id": "ses_1"});

        let (status, _) = send(&f.app, "POST", "/files/bot-writes", true, Some(record("~/Desktop/other.md"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a relayed client can't claim a path");
        let (status, _) = send(&f.app, "POST", "/files/bot-writes", false, Some(record("~/Desktop/report.md"))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&f.app, "POST", "/files/bot-writes", false, Some(record("~/.ssh/x"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a secret location is never recorded");

        std::fs::write(f.home.join("Desktop/report.md"), "changed later").unwrap();
        let (status, body) = read(&f.app, "~/Desktop/report.md", true).await;
        assert_eq!((status, body.contains("changed later")), (StatusCode::OK, true), "{body}");
        let (status, _) = send(&f.app, "GET", &format!("/files/raw?path={}", q("~/Desktop/report.md")), true, None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&f.app, "POST", "/files/write", true, Some(json!({"path": "~/Desktop/report.md", "content": "x"}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "bot-written files are read-only");
        let (status, _) = send(&f.app, "DELETE", &format!("/files/delete?path={}", q("~/Desktop/report.md")), true, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for path in ["~/Desktop/other.md", "~/.ssh/x"] {
            let (status, _) = read(&f.app, path, true).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        }
    }

    /// APFS treats NFC and NFD spellings as the same name: a denied folder
    /// written in the other form must still be denied.
    #[tokio::test]
    async fn unicode_normalization_does_not_bypass_the_deny_list() {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        let scope = Arc::new(FilesScope::new(Some(home.clone()), vec![home.join("Caf\u{e9}")]));
        let app: Router = file_routes(scope);
        for path in ["~/Caf\u{e9}/x.txt", "~/Cafe\u{301}/x.txt"] {
            let (status, body) = send(&app, "POST", "/files/write", false, Some(json!({"path": path, "content": "x"}))).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path:?}: {body}");
        }
    }

    #[tokio::test]
    async fn local_desktop_keeps_home_but_not_secrets() {
        let f = fixture();
        let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}", q("~/.claude/skills")), false, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"s1\""), "{body}");
        for path in ["~/.ssh", "~/.SSH/id_rsa", "/etc/passwd", "~/Library/Application Support/allternit/x.db"] {
            let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}", q(path)), false, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "local list of {path}: {body}");
            let (status, _) = read(&f.app, path, false).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "local read of {path}");
        }
        let (status, body) = send(&f.app, "GET", &format!("/files/list?path={}&recursive=true", q("~")), false, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("id_rsa"), "recursive home listing showed ~/.ssh contents: {body}");
    }

    #[tokio::test]
    async fn deleting_a_symlink_removes_the_link_not_its_target() {
        let f = fixture();
        let (status, _) = send(&f.app, "DELETE", &format!("/files/delete?path={}", q("~/.claude/skills/linked")), false, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(std::fs::symlink_metadata(f.home.join(".claude/skills/linked")).is_err());
        assert!(f.home.join(".agents/plugins/demo/plugin.json").exists(), "the link's target was deleted");
    }
}
