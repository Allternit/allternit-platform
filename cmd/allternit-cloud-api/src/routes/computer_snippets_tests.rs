//! Computers console snippets (step 2.9): the checks and the answer mapping
//! first, then CRUD, the approval gate and runs against a test schema with
//! migrations 153, 154 and 158 and a fake relay connection. No network.

use axum::http::StatusCode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::routes::computers_host::verify_approval;
use crate::routes::runtime_relay::{answer_test_request, register_test_connection, CloudMessage};
use crate::routes::test_support::{
    authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER,
};

// ----------------------------------------------------------- decisions ----

#[test]
fn fields_are_checked_with_plain_messages() {
    assert_eq!(check_name("  Disk report "), Ok("Disk report".into()));
    assert!(check_name("   ").is_err());
    assert!(check_name(&"x".repeat(81)).is_err());
    assert!(check_command("df -h").is_ok());
    assert!(check_command("  \n").is_err());
    assert!(check_command(&"x".repeat(MAX_COMMAND_BYTES + 1)).is_err());
    assert_eq!(check_description(Some("  ".into())), Ok(None));
    assert!(check_description(Some("x".repeat(281))).is_err());
    assert_eq!(check_timeout(None), Ok(None));
    assert_eq!(check_timeout(Some(30)), Ok(Some(30)));
    assert!(check_timeout(Some(0)).is_err());
    assert!(check_timeout(Some(1801)).is_err());
}

#[test]
fn the_nodes_answer_becomes_one_status() {
    let ok = outcome_of(200, &json!({ "changeId": "ch_1" }), "ap1.x");
    assert_eq!(
        (ok.status, ok.change_id.as_deref(), ok.error),
        ("done", Some("ch_1"), None)
    );
    let snake = outcome_of(200, &json!({ "change_id": "ch_2" }), "ap1.x");
    assert_eq!(snake.change_id.as_deref(), Some("ch_2"));
    let elevated = outcome_of(403, &json!({ "error": "needs_elevation" }), "ap1.x");
    assert_eq!(elevated.status, "needs_elevation");
    assert!(elevated.error.unwrap().contains("admin"));
    for code in [404, 405, 501] {
        assert_eq!(
            outcome_of(code, &Value::Null, "ap1.x").status,
            "not_supported"
        );
    }
    let failed = outcome_of(
        500,
        &json!({ "error": "failed", "message": "The command exited with 2.", "changeId": "ch_3" }),
        "ap1.x",
    );
    assert_eq!(
        (failed.status, failed.change_id.as_deref()),
        ("failed", Some("ch_3"))
    );
    assert_eq!(failed.error.as_deref(), Some("The command exited with 2."));
    // A bare machine word is never shown as the sentence.
    assert_eq!(
        outcome_of(500, &json!({ "error": "failed" }), "ap1.x")
            .error
            .unwrap(),
        "The snippet did not finish on that computer."
    );
}

// --------------------------------------------------------- with a db ----

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(
        "ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS runtime_type TEXT;
         ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS hostname TEXT;
         ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS platform TEXT;
         ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS tailnet_ip TEXT;
         CREATE TABLE IF NOT EXISTS provisioned_hosts (id TEXT PRIMARY KEY, name TEXT NOT NULL, incus_endpoint TEXT NOT NULL, region TEXT);
         CREATE TABLE IF NOT EXISTS provisioned_instances (
             id TEXT PRIMARY KEY, user_id TEXT NOT NULL, host_id TEXT, incus_name TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'provisioning', device_id TEXT,
             created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP);
         CREATE TABLE IF NOT EXISTS paired_computers (
             id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, os TEXT, secret_hash TEXT NOT NULL,
             mesh_ip TEXT, vnc_ready BOOLEAN NOT NULL DEFAULT false, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
             last_seen_at TIMESTAMPTZ, organization_id TEXT, provisioned_instance_id TEXT);
         CREATE TABLE IF NOT EXISTS audit_log (
             id TEXT PRIMARY KEY, action TEXT, resource_type TEXT, resource_id TEXT, user_id TEXT,
             user_email TEXT, details TEXT, success BOOLEAN);",
    )
    .execute(&state.db)
    .await
    .unwrap();
    for sql in [
        include_str!("../../migrations_pg/153_computers_console.sql"),
        include_str!("../../migrations_pg/154_computer_metrics.sql"),
        include_str!("../../migrations_pg/158_computer_snippets.sql"),
        include_str!("../../migrations_pg/159_computer_machine_identity.sql"),
    ] {
        let sql = sql.replace("public.", "");
        sqlx::raw_sql(&sql)
            .execute(&state.db)
            .await
            .expect("migration");
        sqlx::raw_sql(&sql)
            .execute(&state.db)
            .await
            .expect("migration twice");
    }
    sqlx::query("DELETE FROM computer_snippets")
        .execute(&state.db)
        .await
        .unwrap();
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

async fn send(state: &Arc<ApiState>, method: &str, path: &str, body: &str) -> Response {
    routes()
        .with_state(state.clone())
        .oneshot(authed_request(method, path, body))
        .await
        .unwrap()
}

async fn json_of(res: Response) -> (StatusCode, Value) {
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[serial_test::serial]
async fn snippets_are_created_listed_changed_and_deleted_by_their_owner_only() {
    let _g = dev_token_on();
    let state = setup().await;
    let (status, made) = json_of(
        send(
            &state,
            "POST",
            "/api/v1/computers/snippets",
            r#"{"name":"Disk report","command":"df -h","description":"Space"}"#,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    assert_eq!(made["command"], "df -h");

    let (status, body) = json_of(
        send(
            &state,
            "POST",
            "/api/v1/computers/snippets",
            r#"{"name":"","command":"x"}"#,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("name"));

    let (_, listed) = json_of(send(&state, "GET", "/api/v1/computers/snippets", "").await).await;
    assert_eq!(listed["snippets"].as_array().unwrap().len(), 1);
    assert_eq!(listed["snippets"][0]["name"], "Disk report");

    let (status, changed) = json_of(
        send(
            &state,
            "PATCH",
            &format!("/api/v1/computers/snippets/{id}"),
            r#"{"command":"df -h /","description":""}"#,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(
        (changed["command"].as_str(), changed["name"].as_str()),
        (Some("df -h /"), Some("Disk report"))
    );
    assert!(
        changed["description"].is_null(),
        "an empty description clears it"
    );

    // Someone else's snippet is a 404 for every verb.
    sqlx::query("INSERT INTO computer_snippets (id, user_id, name, command) VALUES ('snp_theirs', 'someone-else', 'Theirs', 'id')")
        .execute(&state.db)
        .await
        .unwrap();
    for (method, body) in [
        ("GET", ""),
        ("PATCH", r#"{"name":"x"}"#),
        ("DELETE", ""),
        ("POST", r#"{"computers":["a"],"approvalConfirmed":true}"#),
    ] {
        let path = if method == "POST" {
            "/api/v1/computers/snippets/snp_theirs/run"
        } else {
            "/api/v1/computers/snippets/snp_theirs"
        };
        let (status, body) = json_of(send(&state, method, path, body).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
        assert!(body["message"].is_string());
    }

    let (status, body) = json_of(
        send(
            &state,
            "DELETE",
            &format!("/api/v1/computers/snippets/{id}"),
            "",
        )
        .await,
    )
    .await;
    assert_eq!(
        (status, body["removed"].clone()),
        (StatusCode::OK, json!(true))
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_run_needs_confirmation_and_sends_nothing_without_it() {
    let _g = dev_token_on();
    let state = setup().await;
    let rt = unique("rt_noconfirm");
    seed_runtime_device(&state.db, &rt, DEV_USER).await;
    let (_connection, mut outgoing) = register_test_connection(&rt).await;
    let (_, made) = json_of(
        send(
            &state,
            "POST",
            "/api/v1/computers/snippets",
            r#"{"name":"Up","command":"uptime"}"#,
        )
        .await,
    )
    .await;
    let path = format!(
        "/api/v1/computers/snippets/{}/run",
        made["id"].as_str().unwrap()
    );
    let (status, body) = json_of(
        send(
            &state,
            "POST",
            &path,
            &json!({ "computers": [rt] }).to_string(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "approval_required");
    let (status, body) = json_of(
        send(
            &state,
            "POST",
            &path,
            r#"{"computers":[],"approvalConfirmed":true}"#,
        )
        .await,
    )
    .await;
    assert_eq!(
        (status, body["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("no_computers"))
    );
    assert!(outgoing.try_recv().is_err(), "nothing was relayed");
}

#[tokio::test]
#[serial_test::serial]
async fn a_run_sends_the_command_with_an_approval_and_reports_each_computer() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    let rt = unique("rt_run");
    seed_runtime_device(db, &rt, DEV_USER).await;
    sqlx::query("UPDATE runtime_devices SET name = 'box', runtime_type = 'vps' WHERE id = $1")
        .bind(&rt)
        .execute(db)
        .await
        .unwrap();
    let sleepy_rt = unique("rt_sleepy");
    seed_runtime_device(db, &sleepy_rt, DEV_USER).await;
    sqlx::query(
        "UPDATE runtime_devices SET name = 'cloud', runtime_type = 'provisioned' WHERE id = $1",
    )
    .bind(&sleepy_rt)
    .execute(db)
    .await
    .unwrap();
    let pi = unique("pi");
    sqlx::query("INSERT INTO provisioned_instances (id, user_id, incus_name, status, device_id) VALUES ($1, $2, 'c-s', 'sleeping', $3)")
        .bind(&pi)
        .bind(DEV_USER)
        .bind(&sleepy_rt)
        .execute(db)
        .await
        .unwrap();
    let (connection, mut outgoing) = register_test_connection(&rt).await;
    let (sleepy_conn, mut sleepy_out) = register_test_connection(&sleepy_rt).await;
    let _ = sleepy_conn;

    let (_, made) = json_of(
        send(
            &state,
            "POST",
            "/api/v1/computers/snippets",
            r#"{"name":"Up","command":"uptime","timeoutSec":20}"#,
        )
        .await,
    )
    .await;
    let sid = made["id"].as_str().unwrap().to_string();
    let body = json!({ "computers": [rt, pi, "missing-computer", rt], "approvalConfirmed": true })
        .to_string();
    let call = tokio::spawn({
        let state = state.clone();
        let path = format!("/api/v1/computers/snippets/{sid}/run");
        async move { send(&state, "POST", &path, &body).await }
    });
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), outgoing.recv())
        .await
        .expect("a relayed request")
        .unwrap();
    let CloudMessage::Request {
        request_id,
        method,
        path,
        body,
        ..
    } = message
    else {
        panic!("expected a request")
    };
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("POST", "/v1/host/actions")
    );
    let sent: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (sent["type"].clone(), sent["target"].clone()),
        (json!("snippet.run"), json!(sid))
    );
    assert_eq!(
        sent["snippet"],
        json!({ "id": sid, "name": "Up", "script": "uptime", "elevated": false, "timeoutSec": 20 })
    );
    // The approval is bound to this user, this runtime, this action and this snippet.
    let key = crate::routes::runtime_pairing::sha256_hex(format!("token-of-{rt}").as_bytes());
    let approval = sent["approval_id"].as_str().unwrap();
    assert_eq!(
        verify_approval(
            &key,
            approval,
            Some(DEV_USER),
            &rt,
            "snippet.run",
            &sid,
            Utc::now().timestamp()
        ),
        Ok(())
    );
    assert!(verify_approval(
        &key,
        approval,
        Some(DEV_USER),
        &rt,
        "snippet.run",
        "snp_other",
        Utc::now().timestamp()
    )
    .is_err());
    answer_test_request(
        &connection,
        &request_id,
        200,
        r#"{"changeId":"ch_9","undo":null}"#,
    )
    .await;

    let (status, answer) = json_of(call.await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let runs = answer["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3, "a repeated computer runs once");
    assert_eq!(
        (
            runs[0]["computerId"].as_str(),
            runs[0]["status"].as_str(),
            runs[0]["changeId"].as_str()
        ),
        (Some(rt.as_str()), Some("done"), Some("ch_9"))
    );
    assert_eq!(runs[0]["approvalId"], approval);
    assert_eq!(
        (runs[1]["status"].as_str(), runs[2]["status"].as_str()),
        (Some("asleep"), Some("not_found"))
    );
    assert!(runs[1]["error"].as_str().unwrap().contains("asleep"));
    assert!(
        sleepy_out.try_recv().is_err(),
        "an asleep computer is never contacted"
    );

    // The Activity tab can list them.
    let (_, listed) = json_of(
        send(
            &state,
            "GET",
            &format!("/api/v1/computers/snippets/runs?computerId={rt}"),
            "",
        )
        .await,
    )
    .await;
    assert_eq!(listed["runs"].as_array().unwrap().len(), 1);
    assert_eq!(listed["runs"][0]["snippetName"], "Up");
    let (_, all) = json_of(send(&state, "GET", "/api/v1/computers/snippets/runs", "").await).await;
    assert_eq!(all["runs"].as_array().unwrap().len(), 3);
}
