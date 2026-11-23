//! Whole-disk files (Computers console step 2.11) on the legacy `/files/*`
//! routes: the real handlers on a temporary disk, signed the way the cloud
//! relay signs (`relay_auth::signed_headers`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::file_routes;
use crate::files_scope::{Access, Caller, FilesScope};
use crate::relay_auth::{secret_layer, signed_headers, RelaySecret, StaticRelaySecret};

const TOKEN: &str = "allternit_runtime_test_token";
const OWNER: &str = "user-a";

struct Disk {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    outside: PathBuf,
    app: Router,
}

fn disk() -> Disk {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let outside = root.join("srv");
    std::fs::create_dir_all(home.join("Allternit")).unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "HOME KEY").unwrap();
    std::fs::create_dir_all(outside.join("data")).unwrap();
    std::fs::write(outside.join("data/report.txt"), "disk file").unwrap();
    std::fs::write(outside.join("data/server.pem"), "PEM").unwrap();
    std::fs::create_dir_all(outside.join("other/.ssh")).unwrap();
    std::fs::write(outside.join("other/.ssh/id_rsa"), "OTHER KEY").unwrap();
    let scope = Arc::new(FilesScope::new(Some(home.clone()), vec![]));
    let secret: Arc<dyn RelaySecret> = Arc::new(StaticRelaySecret {
        token: TOKEN.into(),
        owner: OWNER.into(),
    });
    let app = file_routes::<()>(scope).layer(secret_layer(secret));
    Disk {
        _tmp: tmp,
        home,
        outside,
        app,
    }
}

/// How a request reaches the runtime.
#[derive(Clone, Copy)]
enum Via {
    /// Relayed, no whole-disk header.
    Relay,
    /// Relayed with `x-allternit-whole-disk: on` but no signature.
    RelayHeaderUnsigned,
    /// Relayed, the header, and the cloud's signature.
    RelaySigned,
    /// Relayed and signed with the header, but for another owner.
    RelaySignedWrongOwner,
    /// Loopback (the Desktop), with the header and a valid signature.
    LocalSigned,
}

fn q(p: &std::path::Path) -> String {
    url::form_urlencoded::byte_serialize(p.to_string_lossy().as_bytes()).collect()
}

async fn send(
    d: &Disk,
    via: Via,
    method: &str,
    route: &str,
    query: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let bytes = body
        .map(|b| serde_json::to_vec(&b).unwrap())
        .unwrap_or_default();
    let mut b = Request::builder()
        .method(method)
        .uri(format!("{route}?{query}"))
        .header("content-type", "application/json");
    let relayed = !matches!(via, Via::LocalSigned);
    if relayed {
        b = b.header("x-allternit-relayed", "1");
    }
    if !matches!(via, Via::Relay) {
        b = b.header("x-allternit-whole-disk", "on");
    }
    let owner = if matches!(via, Via::RelaySignedWrongOwner) {
        "user-b"
    } else {
        OWNER
    };
    if matches!(
        via,
        Via::RelaySigned | Via::RelaySignedWrongOwner | Via::LocalSigned
    ) {
        for (k, v) in signed_headers(TOKEN, owner, method, route, &bytes) {
            b = b.header(k, v);
        }
    }
    let res = d
        .app
        .clone()
        .oneshot(b.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let raw = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&raw) })),
    )
}

async fn read(d: &Disk, via: Via, p: &std::path::Path) -> (StatusCode, Value) {
    send(
        d,
        via,
        "GET",
        "/files/read",
        &format!("path={}", q(p)),
        None,
    )
    .await
}

#[tokio::test]
async fn without_the_signed_header_paths_outside_home_answer_whole_disk_off() {
    let d = disk();
    let file = d.outside.join("data/report.txt");
    for via in [
        Via::Relay,
        Via::RelayHeaderUnsigned,
        Via::RelaySignedWrongOwner,
    ] {
        let (status, body) = read(&d, via, &file).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"], "whole_disk_off", "{body}");
        assert_eq!(body["message"], crate::files_scope::WHOLE_DISK_OFF_MESSAGE);
    }
    // Inside the Files drive nothing changed.
    std::fs::write(d.home.join("Allternit/a.txt"), "drive").unwrap();
    let (status, body) = read(&d, Via::Relay, &d.home.join("Allternit/a.txt")).await;
    assert_eq!(
        (status, body["content"].as_str()),
        (StatusCode::OK, Some("drive"))
    );
}

#[tokio::test]
async fn a_local_caller_sending_the_header_gets_no_extra_access() {
    let d = disk();
    let (status, body) = read(&d, Via::LocalSigned, &d.outside.join("data/report.txt")).await;
    assert_eq!(
        (status, body["reason"].as_str()),
        (StatusCode::FORBIDDEN, Some("outside_home")),
        "{body}"
    );
}

#[tokio::test]
async fn the_signed_header_opens_the_whole_disk() {
    let d = disk();
    let (status, body) = read(&d, Via::RelaySigned, &d.outside.join("data/report.txt")).await;
    assert_eq!(
        (status, body["content"].as_str()),
        (StatusCode::OK, Some("disk file")),
        "{body}"
    );

    let target = d.outside.join("data/new.txt");
    let (status, body) = send(
        &d,
        Via::RelaySigned,
        "POST",
        "/files/write",
        "",
        Some(json!({"path": target.to_string_lossy(), "content": "written"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "written");

    // Home outside the Allternit folders is open too, minus secrets.
    std::fs::create_dir_all(d.home.join("Documents")).unwrap();
    std::fs::write(d.home.join("Documents/notes.md"), "notes").unwrap();
    let (status, _) = read(&d, Via::RelaySigned, &d.home.join("Documents/notes.md")).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &d,
        Via::RelaySigned,
        "GET",
        "/files/list",
        &format!("path={}&recursive=true", q(&d.outside)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(names.contains(&"report.txt"), "{names:?}");
    for hidden in [".ssh", "id_rsa", "server.pem"] {
        assert!(!names.contains(&hidden), "{hidden} was listed: {names:?}");
    }
}

#[tokio::test]
async fn secrets_stay_refused_with_the_whole_disk() {
    let d = disk();
    for secret in [
        d.home.join(".ssh/id_ed25519"),
        d.outside.join("other/.ssh/id_rsa"),
        d.outside.join("data/server.pem"),
        d.home.join(".config/allternit/runtime-identity.json"),
    ] {
        let (status, body) = read(&d, Via::RelaySigned, &secret).await;
        assert_eq!(
            (status, body["reason"].as_str()),
            (StatusCode::FORBIDDEN, Some("protected")),
            "{secret:?}: {body}"
        );
    }
}

#[tokio::test]
async fn parent_steps_and_symlink_escapes_are_refused_with_the_whole_disk() {
    let d = disk();
    let dotdot = format!("{}/data/../other/.ssh/id_rsa", d.outside.display());
    let (status, body) = send(
        &d,
        Via::RelaySigned,
        "GET",
        "/files/read",
        &format!("path={}", q(std::path::Path::new(&dotdot))),
        None,
    )
    .await;
    assert_eq!(
        (status, body["reason"].as_str()),
        (StatusCode::FORBIDDEN, Some("parent_dir")),
        "{body}"
    );

    #[cfg(unix)]
    {
        // A link on the open disk that points into a secret store.
        std::os::unix::fs::symlink(d.home.join(".ssh"), d.outside.join("data/keys")).unwrap();
        let (status, body) = read(
            &d,
            Via::RelaySigned,
            &d.outside.join("data/keys/id_ed25519"),
        )
        .await;
        assert_eq!(
            (status, body["reason"].as_str()),
            (StatusCode::FORBIDDEN, Some("protected")),
            "{body}"
        );
        // A link inside the drive that points out of home: no grant, no escape.
        std::os::unix::fs::symlink(d.outside.join("data"), d.home.join("Allternit/out")).unwrap();
        let (status, body) = read(&d, Via::Relay, &d.home.join("Allternit/out/report.txt")).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("whole_disk_off")),
            "{body}"
        );
    }
}

/// The disk root itself is never written or deleted. Checked on the scope
/// (no I/O happens before the check, and a test must never risk `/`).
#[test]
fn the_disk_root_itself_cannot_be_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(tmp.path()).unwrap();
    let scope = FilesScope::new(Some(home), vec![]);
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    for access in [Access::Write, Access::Delete] {
        assert_eq!(
            scope
                .authorize(root, Caller::WholeDisk, access)
                .unwrap_err()
                .0,
            "disk_root_itself"
        );
    }
    assert!(scope
        .authorize(root, Caller::WholeDisk, Access::Read)
        .is_ok());
    assert_eq!(
        scope
            .authorize(root, Caller::Remote, Access::Read)
            .unwrap_err()
            .0,
        "whole_disk_off"
    );
}
