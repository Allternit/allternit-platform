//! Whole-disk files (Computers console step 2.11) on the Files drive v2
//! routes: disk roots, through the real router, signed the way the cloud
//! relay signs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{routes, Config, FilesV2, GuestAuth};
use crate::auth::AuthUser;
use crate::relay_auth::{secret_layer, signed_headers, RelaySecret, StaticRelaySecret};

const TOKEN: &str = "allternit_runtime_test_token";
const OWNER: &str = "u1";

struct Env {
    _tmp: tempfile::TempDir,
    outside: PathBuf,
    app: Router,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    std::fs::create_dir_all(base.join("srv/data")).unwrap();
    std::fs::write(base.join("srv/data/report.txt"), "disk file").unwrap();
    let cfg = Config {
        enabled: true,
        data_dir: base.join("data"),
        home: Some(base.join("home")),
        computer_id: "rd_test".into(),
        memory_db: None,
        models_dir: None,
        code_dir: None,
        watch: false,
        clock: Some(Arc::new(|| "2026-11-23T16:42:00Z".parse().unwrap())),
    };
    let user = AuthUser {
        user_id: OWNER.into(),
        email: None,
        name: Some("Owner".into()),
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    };
    let secret: Arc<dyn RelaySecret> = Arc::new(StaticRelaySecret {
        token: TOKEN.into(),
        owner: OWNER.into(),
    });
    let app = routes::<()>(Arc::new(FilesV2::with_guests(
        cfg,
        GuestAuth::with_keys(vec![]),
    )))
    .layer(Extension(user))
    .layer(secret_layer(secret));
    Env {
        outside: base.join("srv"),
        _tmp: tmp,
        app,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Via {
    Local,
    Relay,
    RelayHeaderUnsigned,
    RelaySigned,
}

async fn call(
    e: &Env,
    via: Via,
    method: &str,
    route: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let path = format!("/files/v2{route}");
    let bytes = body
        .map(|b| serde_json::to_vec(&b).unwrap())
        .unwrap_or_default();
    let mut b = Request::builder()
        .method(method)
        .uri(&path)
        .header("content-type", "application/json");
    if via != Via::Local {
        b = b.header("x-allternit-relayed", "1");
    }
    if matches!(via, Via::RelayHeaderUnsigned | Via::RelaySigned) {
        b = b.header("x-allternit-whole-disk", "on");
    }
    if via == Via::RelaySigned {
        for (k, v) in signed_headers(TOKEN, OWNER, method, path.split('?').next().unwrap(), &bytes) {
            b = b.header(k, v);
        }
    }
    let res = e
        .app
        .clone()
        .oneshot(b.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let raw = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn add_body(p: &Path, open: bool) -> Value {
    json!({
        "name": "Data",
        "kind": "disk",
        "local_path": p.to_string_lossy(),
        "policy": {"read": true, "write": true, "shareable": open, "agent_process": open, "protected": false, "offline": true,
                   "replica": false, "versions": {"keep_count": 10, "keep_days": 30}, "trash_days": 30},
    })
}

fn root_ids(body: &Value) -> Vec<String> {
    body["roots"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str().map(String::from))
        .collect()
}

#[tokio::test]
async fn a_disk_root_needs_the_signed_header() {
    let e = env();
    let data = e.outside.join("data");
    for via in [Via::Relay, Via::RelayHeaderUnsigned] {
        let (status, body) = call(&e, via, "POST", "/roots", Some(add_body(&data, false))).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("whole_disk_off")),
            "{body}"
        );
        assert_eq!(body["message"], crate::files_scope::WHOLE_DISK_OFF_MESSAGE);
    }
    // A local caller gets no disk root either (the old scope refusal).
    let (status, body) = call(
        &e,
        Via::Local,
        "POST",
        "/roots",
        Some(add_body(&data, false)),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("scope_violation")),
        "{body}"
    );
}

#[tokio::test]
async fn a_signed_disk_root_is_never_shared_or_given_to_agents_and_stays_hidden_without_the_grant()
{
    let e = env();
    let (status, body) = call(
        &e,
        Via::RelaySigned,
        "POST",
        "/roots",
        Some(add_body(&e.outside.join("data"), true)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["root"]["id"].as_str().unwrap().to_string();
    let top = body["root"]["root_item_id"].as_str().unwrap().to_string();
    assert_eq!(body["root"]["policy"]["shareable"], false, "{body}");
    assert_eq!(body["root"]["policy"]["agent_process"], false, "{body}");

    let (_, signed) = call(&e, Via::RelaySigned, "GET", "/roots", None).await;
    assert!(root_ids(&signed).contains(&id), "{signed}");
    let (_, plain) = call(&e, Via::Relay, "GET", "/roots", None).await;
    assert!(
        !root_ids(&plain).contains(&id),
        "a disk root showed without the grant: {plain}"
    );

    let (status, body) = call(&e, Via::Relay, "GET", &format!("/roots/{id}/list?parent_id={top}"), None).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("whole_disk_off")),
        "{body}"
    );
    let (status, body) = call(
        &e,
        Via::RelaySigned,
        "GET",
        &format!("/roots/{id}/list?parent_id={top}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Share refusal: sharing (and agents) can't be switched on, even signed or local.
    let open = json!({"policy": add_body(&e.outside, true)["policy"]});
    for via in [Via::RelaySigned, Via::Local] {
        let (status, body) = call(
            &e,
            via,
            "PATCH",
            &format!("/roots/{id}"),
            Some(open.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
}

#[tokio::test]
async fn the_disk_root_and_folders_holding_home_or_system_secrets_are_refused() {
    let e = env();
    let home_parent = e.outside.parent().unwrap().to_path_buf();
    let fs_root = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    for p in [fs_root, home_parent] {
        let (status, body) = call(
            &e,
            Via::RelaySigned,
            "POST",
            "/roots",
            Some(add_body(&p, false)),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("scope_violation")),
            "{p:?}: {body}"
        );
    }
    let dotdot = PathBuf::from(format!("{}/data/..", e.outside.display()));
    let (status, body) = call(
        &e,
        Via::RelaySigned,
        "POST",
        "/roots",
        Some(add_body(&dotdot, false)),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("scope_violation")),
        "{body}"
    );
}
