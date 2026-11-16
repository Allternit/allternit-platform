//! Route tests for Files drive sharing (step 14). Each test gets its own
//! Postgres schema (routes::test_support) with migration 144 applied. Owners
//! are stood in for by the test-only `x-test-caller` header; an awake
//! computer is a test relay connection answered by a fake runtime.

use std::sync::Mutex;

use super::*;
use crate::routes::runtime_relay::{answer_test_request, register_test_connection, CloudMessage};
use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway};
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const OWNER: &str = "user_owner";
const SEED: [u8; 32] = [42u8; 32];

struct T {
    router: Router,
    db: PgPool,
    computer: String,
}

/// Every test sets the same values before the first key read, so the
/// process-wide key is the same whichever test reads it first.
fn env() {
    std::env::set_var(files_tokens::SEED_ENV, STANDARD.encode(SEED));
    std::env::set_var("ALLTERNIT_FILES_DRIVE_ENABLED", "1");
}

async fn setup() -> T {
    env();
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/144_files_shares.sql"))
        .execute(&state.db)
        .await
        .expect("144 applies");
    let computer = format!("rd_{}", uuid::Uuid::new_v4().simple());
    seed_runtime_device(&state.db, &computer, OWNER).await;
    sqlx::query("UPDATE runtime_devices SET capabilities = $2 WHERE id = $1")
        .bind(&computer)
        .bind(r#"["runtime:connect","runtime:files"]"#)
        .execute(&state.db)
        .await
        .unwrap();
    T { router: routes().with_state(state.clone()), db: state.db.clone(), computer }
}

struct Share {
    id: String,
    link: String,
}

/// A share of `rt_1/it_shared` on `computer`.
async fn share_on(db: &PgPool, computer: &str, role: ShareRole, passcode: Option<&str>, guest_wake: bool) -> Share {
    let id = random_id("sh_", 6);
    let link = random_id("lk", 10);
    sqlx::query(
        "INSERT INTO files_shares (share_id, owner_user_id, owner_display_name, computer_id, root_id, item_id, item_name, \
         role, link_token, passcode_hash, guest_wake, created_by) \
         VALUES ($1, $2, 'Eoj', $3, 'rt_1', 'it_shared', 'Closing docs', $4, $5, $6, $7, '{\"kind\":\"owner\",\"id\":\"user_owner\"}')",
    )
    .bind(&id)
    .bind(OWNER)
    .bind(computer)
    .bind(role.as_str())
    .bind(&link)
    .bind(passcode.map(hash_passcode))
    .bind(guest_wake)
    .execute(db)
    .await
    .unwrap();
    Share { id, link }
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    fn code(&self) -> ErrorCode {
        let e: ErrorBody = serde_json::from_value(self.body.clone()).unwrap_or_else(|_| panic!("not an ErrorBody: {}", self.body));
        assert_eq!(self.status, status_for(e.code), "status follows SPEC section 16");
        e.code
    }
}

async fn send(router: &Router, method: &str, path: &str, headers: &[(&str, String)], body: Option<Value>) -> Reply {
    let mut b = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        b = b.header(*k, v);
    }
    let req = match body {
        Some(body) => b.header("content-type", "application/json").body(Body::from(body.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    Reply { status, body }
}

fn owner() -> (&'static str, String) {
    ("x-test-caller", json!({ "id": OWNER }).to_string())
}

async fn redeem_with(t: &T, link: &str, passcode: Option<&str>) -> Reply {
    let mut body = json!({ "link_token": link });
    if let Some(p) = passcode {
        body["passcode"] = json!(p);
    }
    send(&t.router, "POST", "/api/v1/files-gateway/redeem", &[], Some(body)).await
}

async fn token_for(t: &T, s: &Share) -> String {
    let r = redeem_with(t, &s.link, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.body["capability_token"].as_str().unwrap().to_string()
}

async fn guest(t: &T, method: &str, rest: &str, token: &str) -> Reply {
    send(&t.router, method, &format!("/api/v1/files-gateway/v2/{rest}"), &[("authorization", format!("Files-Guest {token}"))], None).await
}

async fn audit_rows(db: &PgPool, share_id: &str) -> Vec<(String, String, String, Option<String>)> {
    sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT guest_id, action, result, error FROM files_share_access WHERE share_id = $1 ORDER BY id",
    )
    .bind(share_id)
    .fetch_all(db)
    .await
    .unwrap()
}

/// What the fake runtime saw: `(path, authorization)` per relayed request.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// An awake computer: answers every relayed request with `answer(path)`.
async fn fake_runtime(computer: &str, answer: fn(&str) -> (u16, String)) -> Seen {
    let (connection, mut outgoing) = register_test_connection(computer).await;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if let CloudMessage::Request { request_id, path, headers, .. } = message {
                log.lock().unwrap().push((path.clone(), headers.get("authorization").cloned().unwrap_or_default()));
                let (status, body) = answer(&path);
                answer_test_request(&connection, &request_id, status, &body).await;
            }
        }
    });
    seen
}

fn runtime_ok_in_shared_folder(path: &str) -> (u16, String) {
    if path.contains("parent_id=it_shared") || path.ends_with("/whoami") {
        (200, json!({ "ok": true }).to_string())
    } else {
        // What the runtime's second check answers outside the share.
        (403, json!({ "error": "scope_violation", "code": "scope_violation", "message": "That item is not in the shared folder." }).to_string())
    }
}

fn claims_for(t: &T, s: &Share, role: ShareRole) -> GuestCapabilityClaims {
    let now = now_secs();
    GuestCapabilityClaims {
        v: 1,
        typ: "files_guest".into(),
        share_id: s.id.clone(),
        guest_id: "gst_minted".into(),
        owner_user_id: OWNER.into(),
        computer_id: t.computer.clone(),
        root_id: "rt_1".into(),
        item_id: "it_shared".into(),
        role,
        guest_wake: false,
        passcode_ok: true,
        iat: now,
        exp: now + 600,
        kid: files_tokens::token_key().unwrap().kid().to_string(),
    }
}

fn sign(c: &GuestCapabilityClaims) -> String {
    files_tokens::token_key().unwrap().sign(GUEST_TOKEN_PREFIX, c)
}

#[tokio::test]
async fn guest_reads_only_its_folder() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let seen = fake_runtime(&t.computer, runtime_ok_in_shared_folder).await;
    let token = token_for(&t, &s).await;

    let ok = guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &token).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    // The sibling folder is refused by the runtime's second check, and the
    // guest gets that refusal.
    let sibling = guest(&t, "GET", "roots/rt_1/list?parent_id=it_private", &token).await;
    assert_eq!(sibling.code(), ErrorCode::ScopeViolation);

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, "/api/v1/files/v2/roots/rt_1/list?parent_id=it_shared");
    assert_eq!(seen[0].1, format!("Files-Guest {token}"), "the runtime gets the guest token, never the owner's session");
    let rows = audit_rows(&t.db, &s.id).await;
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!((rows[1].1.as_str(), rows[1].2.as_str()), ("list", "allowed"));
    assert_eq!((rows[2].1.as_str(), rows[2].2.as_str(), rows[2].3.as_deref()), ("list", "denied", Some("scope_violation")));
}

#[tokio::test]
async fn expired_token_fails() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let seen = fake_runtime(&t.computer, runtime_ok_in_shared_folder).await;
    let mut c = claims_for(&t, &s, ShareRole::ViewDownload);
    c.iat = now_secs() - 700;
    c.exp = now_secs() - 100;
    let r = guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &sign(&c)).await;
    assert_eq!(r.code(), ErrorCode::Unauthorized);
    assert!(seen.lock().unwrap().is_empty(), "nothing reaches the runtime");
    let rows = audit_rows(&t.db, &s.id).await;
    assert_eq!(rows, vec![("gst_minted".into(), "list".into(), "denied".into(), Some("unauthorized".into()))]);
}

#[tokio::test]
async fn wrong_passcode_fails() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, Some("4417"), false).await;
    let stored: String = sqlx::query_scalar("SELECT passcode_hash FROM files_shares WHERE share_id = $1")
        .bind(&s.id)
        .fetch_one(&t.db)
        .await
        .unwrap();
    assert!(stored.starts_with("pbkdf2-sha256$") && !stored.contains("4417"), "never plaintext");

    assert_eq!(redeem_with(&t, &s.link, None).await.code(), ErrorCode::PasscodeRequired);
    let wrong = redeem_with(&t, &s.link, Some("0000")).await;
    assert_eq!(wrong.code(), ErrorCode::PasscodeInvalid);
    assert_eq!(wrong.body["retry_after_s"], 30);
    let right = redeem_with(&t, &s.link, Some("4417")).await;
    assert_eq!(right.status, StatusCode::OK, "{}", right.body);
    let session: GuestSessionResponse = serde_json::from_value(right.body.clone()).unwrap();
    assert!(session.capability_token.starts_with("fgt1."));
    assert_eq!(session.share.folder_name, "Closing docs");

    let rows = audit_rows(&t.db, &s.id).await;
    let errors: Vec<_> = rows.iter().map(|r| r.3.as_deref()).collect();
    assert_eq!(errors, vec![Some("passcode_required"), Some("passcode_invalid"), None]);
}

#[tokio::test]
async fn revoked_share_fails_on_the_next_request() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let seen = fake_runtime(&t.computer, runtime_ok_in_shared_folder).await;
    let token = token_for(&t, &s).await;
    assert_eq!(guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &token).await.status, StatusCode::OK);

    // Another user cannot revoke it.
    let stranger = ("x-test-caller", json!({ "id": "user_other" }).to_string());
    let r = send(&t.router, "DELETE", &format!("/api/v1/computer-files/shares/{}", s.id), &[stranger], None).await;
    assert_eq!(r.code(), ErrorCode::NotFound);
    let r = send(&t.router, "DELETE", &format!("/api/v1/computer-files/shares/{}", s.id), &[owner()], None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);

    // The same, still-unexpired token fails on its very next request.
    assert_eq!(guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &token).await.code(), ErrorCode::ShareRevoked);
    assert_eq!(seen.lock().unwrap().len(), 1, "the revoked request never reached the runtime");
    assert_eq!(redeem_with(&t, &s.link, None).await.code(), ErrorCode::ShareRevoked);

    // Revoking one guest stops that guest only.
    let s2 = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let a = token_for(&t, &s2).await;
    let b = token_for(&t, &s2).await;
    let guest_a: String = sqlx::query_scalar("SELECT guest_id FROM files_share_guests WHERE share_id = $1 ORDER BY created_at, guest_id LIMIT 1")
        .bind(&s2.id)
        .fetch_one(&t.db)
        .await
        .unwrap();
    let a_is_first = files_tokens::token_key().unwrap().verify_guest(&a, now_secs()).unwrap().guest_id == guest_a;
    let (revoked, kept) = if a_is_first { (a, b) } else { (b, a) };
    let r = send(&t.router, "DELETE", &format!("/api/v1/computer-files/shares/{}/guests/{guest_a}", s2.id), &[owner()], None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &revoked).await.code(), ErrorCode::ShareRevoked);
    assert_eq!(guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &kept).await.status, StatusCode::OK);
}

#[tokio::test]
async fn upload_only_cannot_list() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::UploadOnly, None, false).await;
    let seen = fake_runtime(&t.computer, |_| (200, "{}".to_string())).await;
    let token = token_for(&t, &s).await;
    for rest in ["roots/rt_1/list?parent_id=it_shared", "items/it_shared", "items/it_doc/content"] {
        let r = guest(&t, "GET", rest, &token).await;
        assert_eq!(r.code(), ErrorCode::Forbidden, "{rest}");
        assert_eq!(r.body["message"], "This link only allows uploads.");
    }
    assert!(seen.lock().unwrap().is_empty(), "the gateway refuses before relaying");
}

#[tokio::test]
async fn tampered_signature_fails() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let seen = fake_runtime(&t.computer, runtime_ok_in_shared_folder).await;
    let good = token_for(&t, &s).await;
    // Upgrade the payload to edit, keep the view token's signature.
    let edit = sign(&claims_for(&t, &s, ShareRole::Edit));
    let forged = format!("fgt1.{}.{}", edit.split('.').nth(1).unwrap(), good.split('.').nth(2).unwrap());
    assert_eq!(guest(&t, "POST", "mkdir", &forged).await.code(), ErrorCode::Unauthorized);
    // A token signed with another key.
    let other = FilesTokenKey::from_seed(&[1u8; 32]);
    let foreign = other.sign(GUEST_TOKEN_PREFIX, &claims_for(&t, &s, ShareRole::ViewDownload));
    assert_eq!(guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &foreign).await.code(), ErrorCode::Unauthorized);
    assert!(seen.lock().unwrap().is_empty());
    // An unverifiable token names nobody: only the redeem row exists.
    assert_eq!(audit_rows(&t.db, &s.id).await.len(), 1);
}

#[tokio::test]
async fn token_for_computer_a_fails_on_computer_b() {
    let t = setup().await;
    let b = format!("rd_b_{}", uuid::Uuid::new_v4().simple());
    seed_runtime_device(&t.db, &b, OWNER).await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let seen_a = fake_runtime(&t.computer, runtime_ok_in_shared_folder).await;
    let seen_b = fake_runtime(&b, runtime_ok_in_shared_folder).await;
    // A correctly signed token for this share that claims computer B.
    let mut c = claims_for(&t, &s, ShareRole::ViewDownload);
    c.computer_id = b.clone();
    let r = guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &sign(&c)).await;
    assert_eq!(r.code(), ErrorCode::ScopeViolation);
    assert!(seen_a.lock().unwrap().is_empty() && seen_b.lock().unwrap().is_empty());
    // The real token only ever reaches computer A.
    let token = token_for(&t, &s).await;
    assert_eq!(guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &token).await.status, StatusCode::OK);
    assert_eq!((seen_a.lock().unwrap().len(), seen_b.lock().unwrap().len()), (1, 0));
}

#[tokio::test]
async fn every_access_writes_an_audit_row() {
    let t = setup().await;
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    fake_runtime(&t.computer, runtime_ok_in_shared_folder).await;
    let token = token_for(&t, &s).await;
    guest(&t, "GET", "roots/rt_1/list?parent_id=it_shared", &token).await; // allowed
    guest(&t, "GET", "items/it_private", &token).await; // runtime: scope_violation
    guest(&t, "GET", "roots", &token).await; // not a guest route
    guest(&t, "PUT", "roots/rt_1/content?parent_id=it_shared&name=a.txt", &token).await; // role: forbidden

    let r = send(&t.router, "GET", &format!("/api/v1/computer-files/shares/{}/access", s.id), &[owner()], None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let rows: Vec<ShareAccess> = serde_json::from_value(r.body).unwrap();
    assert_eq!(rows.len(), 5, "redeem plus one row per guest request: {rows:?}");
    let got: Vec<_> = rows.iter().rev().map(|a| (a.action, a.result, a.error)).collect();
    assert_eq!(
        got,
        vec![
            (ShareAccessAction::Redeem, ShareAccessResult::Allowed, None),
            (ShareAccessAction::List, ShareAccessResult::Allowed, None),
            (ShareAccessAction::List, ShareAccessResult::Denied, Some(ErrorCode::ScopeViolation)),
            (ShareAccessAction::List, ShareAccessResult::Denied, Some(ErrorCode::Forbidden)),
            (ShareAccessAction::Upload, ShareAccessResult::Denied, Some(ErrorCode::Forbidden)),
        ]
    );
    assert_eq!(rows[2].item_id.as_deref(), Some("it_private"));
    // The audit is the owner's only.
    let stranger = ("x-test-caller", json!({ "id": "user_other" }).to_string());
    let r = send(&t.router, "GET", &format!("/api/v1/computer-files/shares/{}/access", s.id), &[stranger], None).await;
    assert_eq!(r.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn guest_never_wakes_an_asleep_computer_without_guest_wake() {
    let t = setup().await;
    // No relay connection: the computer is asleep.
    let s = share_on(&t.db, &t.computer, ShareRole::ViewDownload, None, false).await;
    let token = token_for(&t, &s).await;
    let summary = send(&t.router, "GET", &format!("/api/v1/files-gateway/summary?link={}", s.link), &[], None).await;
    assert_eq!((summary.body["computer_state"].as_str(), summary.body["can_wake"].as_bool()), (Some("asleep"), Some(false)));

    for rest in ["roots/rt_1/list?parent_id=it_shared", "items/it_doc/content"] {
        let r = guest(&t, "GET", rest, &token).await;
        assert_eq!(r.code(), ErrorCode::GuestWakeNotAllowed, "{rest}");
        assert_eq!(r.body["computer_state"], "asleep");
    }
    assert!(!runtime_relay::connected_runtime_ids().await.contains(&t.computer));
    let rows = audit_rows(&t.db, &s.id).await;
    assert!(rows.iter().all(|r| r.1 != "wake"), "no wake was attempted: {rows:?}");
    assert_eq!(rows.iter().filter(|r| r.3.as_deref() == Some("guest_wake_not_allowed")).count(), 2);
}

#[tokio::test]
async fn token_keys_without_a_seed_is_feature_disabled() {
    let off = token_keys_response(None);
    assert_eq!(off.status(), StatusCode::FORBIDDEN);
    let body: ErrorBody = serde_json::from_slice(&axum::body::to_bytes(off.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body.code, ErrorCode::FeatureDisabled);
    let key = FilesTokenKey::from_seed(&SEED);
    let on = token_keys_response(Some(&key));
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(on.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(v["keys"][0]["kid"], key.kid());
    assert_eq!(v["keys"][0]["alg"], "Ed25519");
}

#[test]
fn passcode_hash_checks_and_rejects() {
    let h = hash_passcode("4417");
    assert!(check_passcode(&h, "4417"));
    assert!(!check_passcode(&h, "4418"));
    assert!(!check_passcode("4417", "4417"), "a plaintext value is never accepted");
    assert_ne!(hash_passcode("4417"), h, "salted");
}

#[test]
fn upload_session_and_unknown_routes_classify() {
    let get = Method::GET;
    assert_eq!(classify(&get, "roots/rt_1/list"), Some(Op::List));
    assert_eq!(classify(&Method::PUT, "uploads/s1/chunks/abc"), Some(Op::UploadSession));
    assert_eq!(classify(&get, "roots"), None);
    assert_eq!(classify(&Method::POST, "delete"), None);
    assert_eq!(classify(&get, "chunks/abc"), None);
}
