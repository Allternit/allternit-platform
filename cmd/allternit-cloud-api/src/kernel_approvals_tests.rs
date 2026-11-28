//! Kernel adapter over `platform_approvals` (step 11b). The full text flow through a real tool
//! route is `platform_v1::agent_tools::tests::enforce_text_opens_a_kernel_request_approve_redeems_once_and_a_twin_hold_counts`.

use super::*;
use crate::kernel::tests::{receipts_schema, test_key};
use crate::kernel::{check_in, Mode, Tool};
use crate::routes::test_support::{platform_schema, test_state, MockGateway};
use allternit_authority::ActionClass;
use base64::Engine as _;
use std::sync::Arc;

async fn db_with_project() -> Arc<crate::ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    platform_schema(&state.db).await;
    receipts_schema(&state.db).await;
    sqlx::query("INSERT INTO platform_projects (id, owner_user_id, name, env) VALUES ('prj_k', 'owner_k', 'P', 'sandbox')").execute(&state.db).await.unwrap();
    state
}

fn tool<'a>(args: &'a Value, token: Option<&'a str>) -> Tool<'a> {
    Tool {
        surface: "test",
        name: "send_invoice",
        args,
        class: ActionClass::Money,
        known: true,
        approved_by: None,
        ask_to: Some(AskTo { project_id: "prj_k", assignee: None }),
        token,
    }
}

#[test]
fn a_token_verifies_once_minted_and_fails_when_tampered_or_expired() {
    let key = test_key();
    let now = Utc::now();
    let t = mint(&key, "apr_1", "money", "sha256:aa", "owner_k", "owner_k", now);
    let token = t["token"].as_str().unwrap();
    let c = verify(&key, token, now).unwrap();
    assert_eq!((c.request_id.as_str(), c.action_hash.as_str(), c.approved_by.as_str()), ("apr_1", "sha256:aa", "owner_k"));
    assert!(c.expires_at <= now + Duration::seconds(MAX_TTL_SECS));
    // Another kernel's key, a changed field, or past expiry: refused.
    let other = KernelKey::from_seed(&[9u8; 32]);
    assert_eq!(verify(&other, token, now).unwrap_err(), "approval_token_invalid");
    let mut obj: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token).unwrap()).unwrap();
    obj["action_hash"] = json!("sha256:bb");
    let forged = URL_SAFE_NO_PAD.encode(canonical(&obj));
    assert_eq!(verify(&key, &forged, now).unwrap_err(), "approval_token_invalid");
    assert_eq!(verify(&key, token, now + Duration::seconds(MAX_TTL_SECS + 1)).unwrap_err(), "approval_token_expired");
    // The token field never reaches the hash.
    let (clean, tok) = split_token(&json!({ "amount": 5, "kernel_approval_token": "abc" }));
    assert_eq!((clean, tok.as_deref()), (json!({ "amount": 5 }), Some("abc")));
}

#[tokio::test]
async fn enforce_without_a_token_opens_one_request_shadow_writes_only_a_dry_record() {
    let state = db_with_project().await;
    let db = &state.db;
    crate::kernel::set_test_kernel(Some(Mode::Enforce));
    let args = json!({ "amount": 500 });
    let t = tool(&args, None);

    let shadow = check_in(db, Ok(test_key()), Mode::Shadow, "owner_k", "chat_1", &t).await;
    assert!(shadow.proceed);
    let dry: (String, Option<String>) = sqlx::query_as("SELECT status, kernel_mode FROM platform_approvals WHERE id = $1")
        .bind(shadow.approval_request_id.as_deref().unwrap())
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!((dry.0.as_str(), dry.1.as_deref()), ("shadow", Some("shadow")));
    assert!(pending_items(db, "owner_k").await.unwrap().is_empty(), "a dry record is never listed");

    let enforce = check_in(db, Ok(test_key()), Mode::Enforce, "owner_k", "chat_1", &t).await;
    assert!(!enforce.proceed);
    let refusal = enforce.refusal().unwrap();
    assert_eq!(refusal["code"], "approval_required");
    let id = refusal["approval_request_id"].as_str().unwrap().to_string();
    let row: (String, String, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT source, status, approver_user_id, action_hash, kernel_check_receipt_id FROM platform_approvals WHERE id = $1")
            .bind(&id)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!((row.0.as_str(), row.1.as_str(), row.2.as_deref()), ("kernel", "pending", Some("owner_k")));
    assert_eq!(row.3.as_deref(), Some(enforce.action_hash.as_str()));
    assert_eq!(row.4, enforce.receipt_id);
    let items = pending_items(db, "owner_k").await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kernel"]["humanOnly"], true);

    // Approve (the owner) then the retry with no token sent redeems the stored one, once.
    let resp = resolve_cloud(db, "owner_k", &id, "approve", Some("ok")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ran = check_in(db, Ok(test_key()), Mode::Enforce, "owner_k", "chat_1", &t).await;
    assert!(ran.proceed && ran.refusal().is_none());
    let status: String = sqlx::query_scalar("SELECT status FROM platform_approvals WHERE id = $1").bind(&id).fetch_one(db).await.unwrap();
    assert_eq!(status, "consumed");
    // Doing it again asks again: a new request, never the spent approval.
    let again = check_in(db, Ok(test_key()), Mode::Enforce, "owner_k", "chat_1", &t).await;
    assert!(!again.proceed);
    assert_ne!(again.approval_request_id.as_deref(), Some(id.as_str()));
    // A decided row can't be decided again; a denied one never mints.
    let id2 = again.approval_request_id.unwrap();
    assert_eq!(resolve_cloud(db, "owner_k", &id2, "deny", None).await.status(), StatusCode::OK);
    assert_eq!(resolve_cloud(db, "owner_k", &id2, "approve", None).await.status(), StatusCode::CONFLICT);
    crate::kernel::set_test_kernel(None);
}

#[tokio::test]
async fn an_api_key_never_approves_a_kernel_row_and_the_refusal_is_receipted() {
    use crate::routes::platform_v1::approvals::{by_source, resolve, Decision, Hosts};
    use crate::routes::platform_v1::caller::{Plan, ProjectEnv};
    let state = db_with_project().await;
    let args = json!({ "amount": 7 });
    crate::kernel::set_test_kernel(Some(Mode::Enforce));
    let id = check_in(&state.db, Ok(test_key()), Mode::Enforce, "owner_k", "chat_1", &tool(&args, None)).await.approval_request_id.unwrap();
    let row = by_source(&state.db, "prj_k", "kernel", &id).await.unwrap().unwrap();
    let caller = crate::routes::platform_v1::PlatformCaller {
        project_id: "prj_k".into(),
        project_env: ProjectEnv::Sandbox,
        account_id: None,
        key_id: "ak_k".into(),
        scopes: vec!["twin".into(), "computers".into()],
        owner_user_id: "owner_k".into(),
        org_id: None,
        plan: Plan::Sandbox,
        rpm_override: None,
        call_cap_override: None,
    };
    let e = resolve(&state, Hosts { agent: None, computer: None }, &caller, row, Decision::Approve, None).await.unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (StatusCode::FORBIDDEN, "forbidden_caller"));
    let status: String = sqlx::query_scalar("SELECT status FROM platform_approvals WHERE id = $1").bind(&id).fetch_one(&state.db).await.unwrap();
    assert_eq!(status, "pending");
    let refused: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kernel_receipts WHERE principal = 'owner_k' AND kind = 'resolve_refused'").fetch_one(&state.db).await.unwrap();
    assert_eq!(refused, 1);
    crate::kernel::set_test_kernel(None);
}

#[tokio::test]
async fn a_phone_hold_resolution_is_written_through_once() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    receipts_schema(&state.db).await;
    crate::kernel::set_test_kernel(Some(Mode::Enforce));
    let data = json!({ "approval_id": "hold_1", "state": "approved", "run_id": "run_9", "effect": "phone.sms.send" });
    record_hold(&state.db, "owner_h", "phone_hold", &data).await;
    record_hold(&state.db, "owner_h", "phone_hold", &data).await;
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT kind, decision FROM kernel_receipts WHERE principal = 'owner_h'").fetch_all(&state.db).await.unwrap();
    assert_eq!(rows, vec![("hold_resolved".to_string(), "approved".to_string())]);
    crate::kernel::set_test_kernel(None);
}
