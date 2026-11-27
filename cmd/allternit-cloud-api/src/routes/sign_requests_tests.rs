//! State-machine table tests for sign requests (plan C1): every transition
//! from every state, document hash mismatch, foreign file ids, lazy expiry,
//! and `/sign` refusing every credential that is not an interactive session.
//! Store tests run against a real Postgres schema (`test_support::test_pool`)
//! with migrations 042 + 167.

use super::*;
use base64::Engine as _;
use crate::routes::test_support::test_pool;
use chrono::TimeZone;

const OWNER: &str = "user_owner";
const OTHER: &str = "user_other";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SIGNED_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 26, 12, 0, 0).unwrap()
}

fn code(result: Result<impl std::fmt::Debug, Fail>) -> (StatusCode, &'static str, Option<Status>) {
    match result {
        Err(Fail::Refused(r)) => (r.status, r.code, r.current),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A 1x1 PNG.
fn png_b64() -> String {
    let mut bytes = PNG_MAGIC.to_vec();
    bytes.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    STANDARD.encode(bytes)
}

fn slot(id: &str) -> SignSlot {
    SignSlot { id: id.to_string(), label: "Client".to_string(), page: 0, x: 380.0, y: 72.0, w: 180.0, h: 56.0 }
}

fn create_body(file: Uuid, slots: Vec<SignSlot>) -> CreateBody {
    CreateBody {
        title: "Quote Q-1042".to_string(),
        summary: Some("Website rebuild".to_string()),
        details: vec![Detail { label: "Total".to_string(), value: "$4,800".to_string() }],
        document_file_id: file,
        document_sha256: SHA.to_string(),
        slots,
        send_to: None,
        requester_kind: "agent".to_string(),
        requester_name: Some("Gizzi".to_string()),
        source_surface: "gizzi".to_string(),
        thread_id: Some("thread_1".to_string()),
        expires_in_days: None,
    }
}

fn sign_body(signed_file: Uuid) -> SignBody {
    SignBody {
        consent: true,
        consent_version: CONSENT_VERSION.to_string(),
        method: "draw".to_string(),
        signature_png_base64: png_b64(),
        slot: slot("client"),
        document_sha256: SHA.to_string(),
        signed_file_id: signed_file,
        signed_sha256: SIGNED_SHA.to_string(),
        surface: "web".to_string(),
    }
}

fn signer(id: &str) -> Signer {
    Signer { id: id.to_string(), name: Some("Pat Owner".to_string()), email: Some("pat@example.com".to_string()) }
}

async fn store() -> PgPool {
    let db = test_pool().await;
    for (name, sql) in [
        ("042", include_str!("../../migrations_pg/042_user_files.sql")),
        ("167", include_str!("../../migrations_pg/167_sign_requests.sql")),
    ] {
        sqlx::raw_sql(sql).execute(&db).await.unwrap_or_else(|e| panic!("{name} applies: {e}"));
    }
    db
}

async fn add_file(db: &PgPool, user: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO user_files (id, user_id, key, name, content_type, bytes) VALUES ($1, $2, $3, 'quote.pdf', 'application/pdf', 1000)")
        .bind(id)
        .bind(user)
        .bind(format!("u/{user}/{id}/quote.pdf"))
        .execute(db)
        .await
        .unwrap();
    id
}

async fn stored_status(db: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM sign_requests WHERE id = $1").bind(id).fetch_one(db).await.unwrap()
}

/// A request owned by OWNER, moved into `state` at t0 + 1 minute.
async fn request_in(db: &PgPool, state: Status) -> (Uuid, Uuid) {
    let doc = add_file(db, OWNER).await;
    let signed = add_file(db, OWNER).await;
    let row = insert_request(db, OWNER, false, &create_body(doc, vec![slot("client")]), t0()).await.unwrap();
    let at = t0() + Duration::minutes(1);
    match state {
        Status::Pending => {}
        Status::Signed => {
            apply_sign(db, &signer(OWNER), row.id, &sign_body(signed), None, None, at).await.unwrap();
        }
        Status::Declined => {
            apply_end(db, OWNER, row.id, Action::Decline, Some("Wrong total"), at).await.unwrap();
        }
        Status::Cancelled => {
            apply_end(db, OWNER, row.id, Action::Cancel, None, at).await.unwrap();
        }
        Status::Expired => {
            expire_due(db, OWNER, t0() + Duration::days(DEFAULT_EXPIRY_DAYS + 1)).await.unwrap();
        }
    }
    assert_eq!(stored_status(db, row.id).await, state.as_str());
    (row.id, signed)
}

/// The expected outcome of `action` on a request in `state`.
fn expected(state: Status, action: Action) -> Result<Status, (StatusCode, &'static str)> {
    use Action::*;
    use Status::*;
    match (state, action) {
        (Pending, Sign) => Ok(Signed),
        (Pending, Decline) => Ok(Declined),
        (Pending, Cancel) => Ok(Cancelled),
        (Pending, Sent) => Err((StatusCode::CONFLICT, "not-signed")),
        (Signed, Sent) => Ok(Signed),
        (Expired, Sign | Decline | Cancel) => Err((StatusCode::GONE, "expired")),
        (_, _) => Err((StatusCode::CONFLICT, "not-pending")),
    }
}

#[test]
fn transition_table_every_state_every_action() {
    let now = t0();
    let live = now + Duration::days(1);
    for state in Status::ALL {
        for action in Action::ALL {
            let got = transition(state, live, now, action).map_err(|r| {
                assert_eq!(r.current, Some(state), "{state:?} {action:?} reports the current status");
                (r.status, r.code)
            });
            assert_eq!(got, expected(state, action), "{state:?} + {action:?}");
        }
    }
}

#[test]
fn a_pending_request_past_expiry_reads_and_acts_as_expired() {
    let now = t0();
    assert_eq!(effective_status(Status::Pending, now, now), Status::Expired, "expiry instant counts");
    assert_eq!(effective_status(Status::Pending, now + Duration::seconds(1), now), Status::Pending);
    assert_eq!(effective_status(Status::Signed, now - Duration::days(1), now), Status::Signed, "end states keep");
    let gone = transition(Status::Pending, now - Duration::seconds(1), now, Action::Sign).unwrap_err();
    assert_eq!((gone.status, gone.code), (StatusCode::GONE, "expired"));
}

#[test]
fn sign_checks_run_state_then_hash_then_slot() {
    let now = t0();
    let live = now + Duration::days(1);
    let slots = vec![slot("client")];
    assert!(check_sign_against(Status::Pending, live, now, SHA, &slots, SHA, &slot("client")).is_ok());

    let changed = check_sign_against(Status::Pending, live, now, SHA, &slots, SIGNED_SHA, &slot("client")).unwrap_err();
    assert_eq!((changed.status, changed.code), (StatusCode::CONFLICT, "document-changed"));

    let elsewhere = check_sign_against(Status::Pending, live, now, SHA, &slots, SHA, &slot("witness")).unwrap_err();
    assert_eq!(elsewhere.code, "bad-slot");
    assert!(slot_allowed(&[], &slot("tapped")), "no slots: the signer picks the spot");

    let done = check_sign_against(Status::Signed, live, now, SHA, &slots, SIGNED_SHA, &slot("witness")).unwrap_err();
    assert_eq!(done.code, "not-pending", "an end state wins over a hash mismatch");
}

#[test]
fn sign_body_needs_consent_a_png_and_hashes() {
    let file = Uuid::new_v4();
    assert!(check_sign(&sign_body(file)).is_ok());

    let no_consent = SignBody { consent: false, ..sign_body(file) };
    assert_eq!(check_sign(&no_consent).unwrap_err().code, "consent-required");
    let old_text = SignBody { consent_version: "v0".to_string(), ..sign_body(file) };
    assert_eq!(check_sign(&old_text).unwrap_err().code, "consent-required");

    let jpeg = SignBody { signature_png_base64: STANDARD.encode([0xFFu8, 0xD8, 0xFF, 0xE0]), ..sign_body(file) };
    assert_eq!(check_sign(&jpeg).unwrap_err().code, "bad-signature");
    let huge = SignBody { signature_png_base64: STANDARD.encode(vec![0u8; MAX_SIGNATURE_PNG_BYTES + 1]), ..sign_body(file) };
    assert_eq!(check_sign(&huge).unwrap_err().code, "bad-signature");

    let bad_slot = SignBody { slot: SignSlot { w: 0.0, ..slot("client") }, ..sign_body(file) };
    assert_eq!(check_sign(&bad_slot).unwrap_err().code, "bad-slot");
    let short_hash = SignBody { document_sha256: "abc".to_string(), ..sign_body(file) };
    assert_eq!(check_sign(&short_hash).unwrap_err().code, "bad-request");
}

#[test]
fn only_interactive_session_claims_may_sign() {
    let session = json!({ "sub": "user_1", "sid": "sess_1", "azp": "https://ai.allternit.com" });
    assert!(is_interactive_session(&session));
    for (label, claims) in [
        ("oauth access token (MCP connector)", json!({ "sub": "user_1", "scope": "openid email", "client_id": "c1" })),
        ("oauth token with scp", json!({ "sub": "user_1", "scp": ["bots:act"], "cid": "c1" })),
        ("session id plus a client", json!({ "sub": "user_1", "sid": "sess_1", "client_id": "c1" })),
        ("no session id", json!({ "sub": "user_1" })),
        ("empty session id", json!({ "sub": "user_1", "sid": "" })),
    ] {
        assert!(!is_interactive_session(&claims), "{label} must not sign");
    }
}

#[tokio::test]
async fn sign_with_a_non_session_credential_is_403() {
    for token in [
        "allternit_0123456789abcdef0123456789abcdef",
        "allternit_runtime_0123456789abcdef0123456789",
        "alt_live_0123456789abcdef",
        "alt_oat_0123456789abcdef",
        "dev-api-token",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(
            code(interactive_signer(&headers).await),
            (StatusCode::FORBIDDEN, "user-session-required", None),
            "{token}"
        );
    }
    match interactive_signer(&HeaderMap::new()).await {
        Err(Fail::Api(ApiError::Unauthorized(_))) => {}
        other => panic!("no credential stays 401, got {other:?}"),
    }
}

#[tokio::test]
async fn every_transition_from_every_state_against_the_store() {
    let db = store().await;
    let at = t0() + Duration::minutes(5);
    for state in Status::ALL {
        for action in Action::ALL {
            // Expired rows are acted on after their expiry; the rest well before it.
            let when = if state == Status::Expired { t0() + Duration::days(DEFAULT_EXPIRY_DAYS + 2) } else { at };
            let (id, signed_file) = request_in(&db, state).await;
            let result = match action {
                Action::Sign => apply_sign(&db, &signer(OWNER), id, &sign_body(signed_file), None, None, when).await,
                Action::Decline => apply_end(&db, OWNER, id, Action::Decline, None, when).await,
                Action::Cancel => apply_end(&db, OWNER, id, Action::Cancel, None, when).await,
                Action::Sent => apply_sent(&db, OWNER, id, "client@example.com", when).await,
            };
            match expected(state, action) {
                Ok(next) => {
                    let row = result.unwrap_or_else(|e| panic!("{state:?} + {action:?}: {e:?}"));
                    assert_eq!(row.status, next.as_str(), "{state:?} + {action:?}");
                }
                Err((status, want)) => {
                    let (got_status, got_code, _) = code(result);
                    assert_eq!((got_status, got_code), (status, want), "{state:?} + {action:?}");
                    assert_eq!(stored_status(&db, id).await, state.as_str(), "{state:?} + {action:?} leaves the row");
                }
            }
        }
    }
}

#[tokio::test]
async fn signing_stores_the_record_from_the_account() {
    let db = store().await;
    let (id, signed_file) = request_in(&db, Status::Pending).await;
    let row = apply_sign(
        &db,
        &signer(OWNER),
        id,
        &sign_body(signed_file),
        Some("203.0.113.7".to_string()),
        Some("Safari".to_string()),
        t0() + Duration::minutes(2),
    )
    .await
    .unwrap();
    assert_eq!(
        (row.signer_name.as_deref(), row.signer_email.as_deref(), row.signed_file_id, row.consent_version.as_deref()),
        (Some("Pat Owner"), Some("pat@example.com"), Some(signed_file), Some("v1"))
    );
    let (ip, ua, text, png_len): (Option<String>, Option<String>, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT signer_ip, signer_user_agent, consent_text, length(signature_png) FROM sign_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((ip.as_deref(), ua.as_deref(), text.as_deref()), (Some("203.0.113.7"), Some("Safari"), Some(CONSENT_TEXT_V1)));
    assert!(png_len.unwrap_or(0) > 8, "signature PNG stored");

    let again = apply_sign(&db, &signer(OWNER), id, &sign_body(signed_file), None, None, t0() + Duration::minutes(3)).await;
    assert_eq!(code(again), (StatusCode::CONFLICT, "not-pending", Some(Status::Signed)), "second sign = 409");
}

#[tokio::test]
async fn a_changed_document_hash_is_409_and_leaves_the_request_pending() {
    let db = store().await;
    let (id, signed_file) = request_in(&db, Status::Pending).await;
    let body = SignBody { document_sha256: SIGNED_SHA.to_string(), ..sign_body(signed_file) };
    let result = apply_sign(&db, &signer(OWNER), id, &body, None, None, t0() + Duration::minutes(2)).await;
    assert_eq!(code(result).1, "document-changed");
    assert_eq!(stored_status(&db, id).await, "pending");
}

#[tokio::test]
async fn foreign_file_ids_are_404() {
    let db = store().await;
    let foreign = add_file(&db, OTHER).await;
    let result = insert_request(&db, OWNER, false, &create_body(foreign, vec![]), t0()).await;
    assert_eq!(code(result), (StatusCode::NOT_FOUND, "file-not-found", None), "document from another account");

    let (id, _) = request_in(&db, Status::Pending).await;
    let result = apply_sign(&db, &signer(OWNER), id, &sign_body(foreign), None, None, t0() + Duration::minutes(2)).await;
    assert_eq!(code(result).1, "file-not-found", "signed copy from another account");
    assert_eq!(stored_status(&db, id).await, "pending");

    // Another user cannot see or act on OWNER's request at all.
    let result = apply_sign(&db, &signer(OTHER), id, &sign_body(foreign), None, None, t0() + Duration::minutes(2)).await;
    assert_eq!(code(result), (StatusCode::NOT_FOUND, "not-found", None));
}

#[tokio::test]
async fn lazy_expiry_writes_expired_on_the_next_read() {
    let db = store().await;
    let (id, signed_file) = request_in(&db, Status::Pending).await;
    let before = t0() + Duration::days(DEFAULT_EXPIRY_DAYS) - Duration::seconds(1);
    expire_due(&db, OWNER, before).await.unwrap();
    assert_eq!(stored_status(&db, id).await, "pending", "not due yet");

    let after = t0() + Duration::days(DEFAULT_EXPIRY_DAYS) + Duration::seconds(1);
    let result = apply_sign(&db, &signer(OWNER), id, &sign_body(signed_file), None, None, after).await;
    assert_eq!(code(result), (StatusCode::GONE, "expired", Some(Status::Expired)));
    let (status, resolved): (String, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT status, resolved_at FROM sign_requests WHERE id = $1").bind(id).fetch_one(&db).await.unwrap();
    assert_eq!(status, "expired");
    assert_eq!(resolved, Some(t0() + Duration::days(DEFAULT_EXPIRY_DAYS)), "resolved at the expiry time");
}

#[test]
fn create_rules() {
    let file = Uuid::new_v4();
    let ok = check_create(&create_body(file, vec![slot("client")]), true).unwrap();
    assert_eq!((ok.requester_kind, ok.expires_in_days), ("agent", DEFAULT_EXPIRY_DAYS));

    let self_by_key = CreateBody { requester_kind: "self".to_string(), ..create_body(file, vec![]) };
    assert_eq!(check_create(&self_by_key, true).unwrap_err().code, "bad-request", "an API key is never self");
    assert!(check_create(&CreateBody { requester_kind: "self".to_string(), ..create_body(file, vec![]) }, false).is_ok());

    let too_long = CreateBody { expires_in_days: Some(MAX_EXPIRY_DAYS + 1), ..create_body(file, vec![]) };
    assert_eq!(check_create(&too_long, false).unwrap_err().code, "bad-request");
    let dup_slots = create_body(file, vec![slot("a"), slot("a")]);
    assert_eq!(check_create(&dup_slots, false).unwrap_err().code, "bad-slot");
    let nine = CreateBody {
        details: (0..9).map(|i| Detail { label: format!("L{i}"), value: "v".to_string() }).collect(),
        ..create_body(file, vec![])
    };
    assert_eq!(check_create(&nine, false).unwrap_err().code, "bad-request");
}
