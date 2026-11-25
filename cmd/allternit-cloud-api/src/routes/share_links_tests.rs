//! Share-link registry (plan v3 lane 7): the name checks first, then the
//! routes against a test schema with migrations 085 and 161 (each applied
//! twice: both are idempotent). The dev token is the caller (DEV_USER);
//! another user's rows are inserted directly. No network.

use axum::http::StatusCode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{
    authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER,
};

#[test]
fn link_names_are_dns_labels_and_minted_names_are_random() {
    assert_eq!(check_subdomain("  My-Demo "), Ok("my-demo".into()));
    for bad in ["ab", "-demo", "demo-", "has space", "dots.not.ok", "ünï", &"a".repeat(64)] {
        assert!(check_subdomain(bad).is_err(), "{bad} must be refused");
    }
    let a = mint_label(MINTED_SUBDOMAIN_LEN);
    let b = mint_label(MINTED_SUBDOMAIN_LEN);
    assert_eq!(a.len(), 16);
    assert_ne!(a, b);
    assert!(a.as_bytes()[0].is_ascii_lowercase());
    assert!(check_subdomain(&a).is_ok() || reserved_name(&a), "{a} is a DNS label");
}

#[test]
fn share_links_reserved_names_refuse_our_brands_vendor_brands_and_infrastructure() {
    for name in [
        "allternit", "my-allternit-demo", "gizzi", "gizziapp", "claude", "claudeai", "my-claude",
        "openai-demo", "chatgpt", "google", "apple-pay", "microsoft", "github", "admin", "api",
        "www", "mail", "live", "meta",
    ] {
        assert_eq!(check_subdomain(name).map_err(|e| e.0), Err("reserved_subdomain"), "{name}");
    }
    for name in ["pineapple", "my-demo", "admin-panel", "metal-band", "tip-calc"] {
        assert_eq!(check_subdomain(name), Ok(name.to_string()), "{name} is allowed");
    }
    assert_eq!(check_subdomain("-bad").map_err(|e| e.0), Err("invalid_subdomain"));
}

#[test]
#[serial_test::serial]
fn share_links_url_follows_app_domain() {
    std::env::remove_var("APP_DOMAIN");
    assert_eq!(link_url("hosted", "tips"), Some("https://ai.allternit.com/app/tips".into()));
    assert_eq!(link_url("live", "tips"), None, "live links need their own domain");
    std::env::set_var("APP_DOMAIN", " https://AllternitUGC.app/ ");
    assert_eq!(link_url("hosted", "tips"), Some("https://tips.allternitugc.app".into()));
    assert_eq!(link_url("live", "tips"), Some("https://tips.live.allternitugc.app".into()));
    std::env::set_var("APP_DOMAIN", "not a domain");
    assert_eq!(link_url("hosted", "tips"), Some("https://ai.allternit.com/app/tips".into()));
    std::env::remove_var("APP_DOMAIN");
}

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/085_artifacts_v2.sql"),
        include_str!("../../migrations_pg/161_apps_mcp_2026.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.expect("migration");
        sqlx::raw_sql(sql).execute(&state.db).await.expect("migration twice");
    }
    sqlx::query("DELETE FROM share_links").execute(&state.db).await.unwrap();
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

async fn call(state: &Arc<ApiState>, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let body = if body.is_null() { String::new() } else { body.to_string() };
    let res = routes()
        .with_state(state.clone())
        .oneshot(authed_request(method, path, &body))
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn seed_artifact(db: &PgPool, owner: &str) -> String {
    let id = unique("art");
    sqlx::query("INSERT INTO artifacts (id, owner_id, kind, title) VALUES ($1, $2, 'page', 'Demo')")
        .bind(&id)
        .bind(owner)
        .execute(db)
        .await
        .unwrap();
    id
}

#[tokio::test]
#[serial_test::serial]
async fn share_links_create_list_pause_heartbeat_and_delete() {
    let _g = dev_token_on();
    let state = setup().await;
    let art = seed_artifact(&state.db, DEV_USER).await;

    let (status, hosted) = call(
        &state,
        "POST",
        "/api/v1/share-links",
        json!({ "kind": "hosted", "subject": "artifact", "artifact_id": art, "label": "Demo", "app_kind": "artifact" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{hosted}");
    assert_eq!(hosted["status"], "active");
    assert_eq!(hosted["auth_mode"], "public");
    assert_eq!(hosted["app_kind"], "artifact");
    assert_eq!(hosted["subdomain"].as_str().unwrap().len(), MINTED_SUBDOMAIN_LEN);
    assert!(hosted.get("abuse_note").is_none(), "abuse_note is admin-only");
    let id = hosted["id"].as_str().unwrap().to_string();

    let device = unique("rt");
    seed_runtime_device(&state.db, &device, DEV_USER).await;
    let name = unique("dev").replace('_', "-")[..20].to_string();
    let (status, live) = call(
        &state,
        "POST",
        "/api/v1/share-links",
        json!({ "kind": "live", "subject": "port", "runtime_device_id": device, "target_port": 3000, "subdomain": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{live}");
    assert_eq!(live["subdomain"], json!(name));
    assert_eq!(live["target_port"], 3000);

    let (status, list) = call(&state, "GET", "/api/v1/share-links", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["links"].as_array().unwrap().len(), 2);
    assert_eq!(list["links"][0]["id"], live["id"], "newest first");

    let pause = format!("/api/v1/share-links/{id}/pause");
    let (status, paused) = call(&state, "POST", &pause, Value::Null).await;
    assert_eq!((status, paused["status"].as_str()), (StatusCode::OK, Some("paused")));
    let (status, resumed) = call(&state, "POST", &pause, json!({ "paused": false })).await;
    assert_eq!((status, resumed["status"].as_str()), (StatusCode::OK, Some("active")));

    let beat = format!("/api/v1/share-links/{id}/heartbeat");
    assert!(hosted["last_seen_at"].is_null());
    let (status, seen) = call(&state, "POST", &beat, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(seen["last_seen_at"].is_string());

    let (status, gone) = call(&state, "DELETE", &format!("/api/v1/share-links/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gone["link"]["status"], "revoked");
    let (_, list) = call(&state, "GET", "/api/v1/share-links", Value::Null).await;
    assert_eq!(list["links"].as_array().unwrap().len(), 1, "revoked links leave the list");
    let (_, all) = call(&state, "GET", "/api/v1/share-links?include_revoked=true", Value::Null).await;
    assert_eq!(all["links"].as_array().unwrap().len(), 2);
    let (status, body) = call(&state, "POST", &beat, Value::Null).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::GONE, Some("revoked")));
    let (status, _) = call(&state, "POST", &pause, json!({ "paused": false })).await;
    assert_eq!(status, StatusCode::GONE, "a deleted link can't be resumed");

    // A chosen name is unique across every link.
    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/share-links",
        json!({ "kind": "live", "subject": "port", "runtime_device_id": device, "target_port": 8080, "subdomain": name }),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("subdomain_taken")));
}

#[tokio::test]
#[serial_test::serial]
async fn share_links_are_owner_scoped() {
    let _g = dev_token_on();
    let state = setup().await;
    let other = unique("someone_else");
    let foreign = unique("shl");
    sqlx::query(
        "INSERT INTO share_links (id, user_id, kind, subject, subdomain) VALUES ($1, $2, 'hosted', 'mcp', $3)",
    )
    .bind(&foreign)
    .bind(&other)
    .bind(mint_label(16))
    .execute(&state.db)
    .await
    .unwrap();

    let (_, list) = call(&state, "GET", "/api/v1/share-links?include_revoked=true", Value::Null).await;
    assert!(list["links"].as_array().unwrap().is_empty());
    for (method, path) in [
        ("POST", format!("/api/v1/share-links/{foreign}/pause")),
        ("POST", format!("/api/v1/share-links/{foreign}/heartbeat")),
        ("DELETE", format!("/api/v1/share-links/{foreign}")),
    ] {
        let (status, _) = call(&state, method, &path, Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let status: String = sqlx::query_scalar("SELECT status FROM share_links WHERE id = $1")
        .bind(&foreign)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "active", "someone else's link is untouched");

    // Linking someone else's artifact or computer is 404 too.
    let their_art = seed_artifact(&state.db, &other).await;
    let (status, _) = call(
        &state,
        "POST",
        "/api/v1/share-links",
        json!({ "kind": "hosted", "subject": "artifact", "artifact_id": their_art }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let their_device = unique("rt");
    seed_runtime_device(&state.db, &their_device, &other).await;
    let (status, _) = call(
        &state,
        "POST",
        "/api/v1/share-links",
        json!({ "kind": "live", "subject": "port", "runtime_device_id": their_device, "target_port": 3000 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial_test::serial]
async fn share_links_refuse_bad_shapes_and_killed_links_stay_dead() {
    let _g = dev_token_on();
    let state = setup().await;
    let device = unique("rt");
    seed_runtime_device(&state.db, &device, DEV_USER).await;
    for (body, error) in [
        (json!({ "kind": "tunnel", "subject": "port" }), "invalid_kind"),
        (json!({ "kind": "hosted", "subject": "folder" }), "invalid_subject"),
        (json!({ "kind": "hosted", "subject": "port", "target_port": 3000 }), "invalid_port"),
        (json!({ "kind": "live", "subject": "port", "runtime_device_id": device }), "invalid_port"),
        (json!({ "kind": "live", "subject": "mcp" }), "device_required"),
        (json!({ "kind": "hosted", "subject": "artifact" }), "artifact_required"),
        (json!({ "kind": "hosted", "subject": "mcp", "auth_mode": "anyone" }), "invalid_auth_mode"),
        (json!({ "kind": "hosted", "subject": "mcp", "app_kind": "vendor" }), "vendor_app_required"),
        (json!({ "kind": "hosted", "subject": "mcp", "subdomain": "-bad" }), "invalid_subdomain"),
        (json!({ "kind": "hosted", "subject": "mcp", "owner": "x" }), "invalid_body"),
    ] {
        let (status, got) = call(&state, "POST", "/api/v1/share-links", body.clone()).await;
        assert_eq!((status, got["error"].as_str()), (StatusCode::BAD_REQUEST, Some(error)), "{body}");
    }

    let (status, made) = call(
        &state,
        "POST",
        "/api/v1/share-links",
        json!({ "kind": "live", "subject": "mcp", "runtime_device_id": device }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE share_links SET status = 'killed' WHERE id = $1")
        .bind(&id)
        .execute(&state.db)
        .await
        .unwrap();
    let (status, body) = call(&state, "POST", &format!("/api/v1/share-links/{id}/heartbeat"), Value::Null).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::GONE, Some("killed")));
    let (status, _) = call(&state, "POST", &format!("/api/v1/share-links/{id}/pause"), json!({ "paused": false })).await;
    assert_eq!(status, StatusCode::GONE);
    let (status, body) = call(&state, "DELETE", &format!("/api/v1/share-links/{id}"), Value::Null).await;
    assert_eq!((status, body["link"]["status"].as_str()), (StatusCode::OK, Some("killed")), "delete never un-kills");
}

#[tokio::test]
#[serial_test::serial]
async fn share_links_need_a_signed_in_caller() {
    let state = setup().await;
    let res = routes()
        .with_state(state)
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/v1/share-links")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[serial_test::serial]
async fn share_links_over_cap_answer_429_and_killed_subjects_cannot_be_shared_again() {
    let _g = dev_token_on();
    let state = setup().await;
    let device = unique("rt");
    seed_runtime_device(&state.db, &device, DEV_USER).await;
    let live = json!({ "kind": "live", "subject": "mcp", "runtime_device_id": device });

    let mut ids = Vec::new();
    for _ in 0..MAX_ACTIVE_LIVE_LINKS {
        let (status, made) = call(&state, "POST", "/api/v1/share-links", live.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        assert!(made.get("url").is_some(), "every link carries url");
        ids.push(made["id"].as_str().unwrap().to_string());
    }
    let (status, body) = call(&state, "POST", "/api/v1/share-links", live.clone()).await;
    assert_eq!((status, body["error"].as_str(), body["cap"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("cap_reached"), Some("links")), "{body}");
    assert_eq!((body["used"].as_i64(), body["limit"].as_i64()), (Some(3), Some(3)));
    // Hosted links don't count against the live cap.
    let (status, _) = call(&state, "POST", "/api/v1/share-links", json!({ "kind": "hosted", "subject": "mcp" })).await;
    assert_eq!(status, StatusCode::CREATED);

    // Pausing frees a slot; resuming over the cap is refused.
    let pause = format!("/api/v1/share-links/{}/pause", ids[0]);
    call(&state, "POST", &pause, Value::Null).await;
    let (status, _) = call(&state, "POST", "/api/v1/share-links", live.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(&state, "POST", &pause, json!({ "paused": false })).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("cap_reached")));

    // 20 GB served this month stops new live links, deleted ones included.
    sqlx::query("UPDATE share_links SET status = 'revoked' WHERE user_id = $1").bind(DEV_USER).execute(&state.db).await.unwrap();
    sqlx::query("UPDATE share_links SET bytes_month = $2 WHERE id = $1")
        .bind(&ids[1])
        .bind(MAX_LIVE_BYTES_MONTH)
        .execute(&state.db)
        .await
        .unwrap();
    let (status, body) = call(&state, "POST", "/api/v1/share-links", live.clone()).await;
    assert_eq!((status, body["cap"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("bytes")), "{body}");

    // An artifact whose link was killed can't be shared again.
    let art = seed_artifact(&state.db, DEV_USER).await;
    let hosted = json!({ "kind": "hosted", "subject": "artifact", "artifact_id": art });
    let (status, made) = call(&state, "POST", "/api/v1/share-links", hosted.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    sqlx::query("UPDATE share_links SET status = 'killed' WHERE id = $1").bind(made["id"].as_str().unwrap()).execute(&state.db).await.unwrap();
    let (status, body) = call(&state, "POST", "/api/v1/share-links", hosted).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::GONE, Some("killed")));

    // A reserved name is its own error.
    let (status, body) = call(&state, "POST", "/api/v1/share-links", json!({ "kind": "hosted", "subject": "mcp", "subdomain": "claude" })).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("reserved_subdomain")));
}
