//! A fresh workspace has no template folder. The API must still list and
//! serve the built-in templates, or a new install shows an empty Workflows
//! view and can't start a run.

use std::sync::Arc;

use allternit_factory_engine::api::service::{create_router, ServiceState};
use serde_json::Value;
use tempfile::TempDir;

#[tokio::test]
async fn fresh_workspace_lists_and_serves_builtin_templates() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(ServiceState::new(dir.path().to_path_buf()).await.unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, create_router(state)).await.unwrap() });

    let list: Value = reqwest::get(format!("{base}/api/factory/templates")).await.unwrap().json().await.unwrap();
    let ids: Vec<&str> = list["templates"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
    assert!(ids.contains(&"build-check-prove"), "built-ins missing: {list}");
    assert!(ids.contains(&"fact-check"), "built-ins missing: {list}");

    let one = reqwest::get(format!("{base}/api/factory/templates/fact-check")).await.unwrap();
    assert_eq!(one.status(), 200);

    let missing = reqwest::get(format!("{base}/api/factory/templates/no-such-template")).await.unwrap();
    assert_eq!(missing.status(), 404);
}

/// A run started from a template saves back as a template
/// (`POST /api/factory/templates { fromRun }`), with its steps, order,
/// route-back, wait-gate evidence and closure.
#[tokio::test]
async fn a_run_saves_back_as_a_template() {
    let dir = TempDir::new().unwrap();
    // build-check-prove assigns steps by role; the workspace's only team fills them.
    let team = dir.path().join(".allternit/teams/docs");
    std::fs::create_dir_all(&team).unwrap();
    std::fs::write(
        team.join("team.yaml"),
        "bots:\n  - { bot: builder, role: build, binding: terminal, harness: bash }\n  - { bot: checker, role: check, binding: terminal, harness: bash }\n",
    )
    .unwrap();
    let state = Arc::new(ServiceState::new(dir.path().to_path_buf()).await.unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, create_router(state)).await.unwrap() });
    let http = reqwest::Client::new();
    let post = |path: &str, body: Value| http.post(format!("{base}{path}")).json(&body).send();

    let run = post("/api/factory/runs", serde_json::json!({ "template": "build-check-prove", "intent": "Ship the docs", "projectId": "p1" }))
        .await
        .unwrap();
    let status = run.status();
    let run: Value = run.json().await.unwrap();
    assert_eq!(status, 200, "{run}");
    let dag = run["dagId"].as_str().unwrap().to_string();

    let dry: Value = post("/api/factory/templates", serde_json::json!({ "fromRun": dag, "id": "docs-run", "dryRun": true }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(dry["plan"]["steps"], 3, "{dry}");
    assert_eq!(dry["plan"]["fromRun"]["finished"], false, "{dry}");
    assert!(!dir.path().join(".allternit/rails/templates/docs-run.json").exists(), "a dry run wrote");

    let saved = post("/api/factory/templates", serde_json::json!({ "fromRun": dag, "id": "docs-run" })).await.unwrap();
    assert_eq!(saved.status(), 200);

    let t: Value = reqwest::get(format!("{base}/api/factory/templates/docs-run")).await.unwrap().json().await.unwrap();
    let ids: Vec<&str> = t["steps"].as_array().unwrap().iter().filter_map(|s| s["id"].as_str()).collect();
    assert_eq!(ids, ["build", "check", "prove"], "{t}");
    assert_eq!(t["steps"][1]["blockedBy"], serde_json::json!(["build"]), "{t}");
    assert_eq!(t["steps"][0]["executor"], "bot:builder", "{t}");
    assert_eq!(t["steps"][1]["onFail"], "build", "{t}");
    assert_eq!(t["steps"][2]["waitGate"]["evidence"], "PROOF.md and proof/ files", "{t}");
    assert_eq!(t["maxRounds"], 3, "{t}");
    assert!(t["closure"]["success"].as_str().unwrap().starts_with("Built, checked"), "{t}");

    // Saving again without force is a conflict; an unknown run is not found.
    let again = post("/api/factory/templates", serde_json::json!({ "fromRun": dag, "id": "docs-run" })).await.unwrap();
    assert_eq!(again.status(), 403);
    let missing = post("/api/factory/templates", serde_json::json!({ "fromRun": "dag_nope" })).await.unwrap();
    assert_eq!(missing.status(), 404);
}

/// A role template with two teams and none named is refused with the choice,
/// and naming the team works.
#[tokio::test]
async fn a_role_template_run_names_its_team() {
    let dir = TempDir::new().unwrap();
    for name in ["alpha", "beta"] {
        let team = dir.path().join(format!(".allternit/teams/{name}"));
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(
            team.join("team.yaml"),
            "bots:\n  - { bot: builder, role: build, binding: terminal, harness: bash }\n  - { bot: checker, role: check, binding: terminal, harness: bash }\n",
        )
        .unwrap();
    }
    let state = Arc::new(ServiceState::new(dir.path().to_path_buf()).await.unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, create_router(state)).await.unwrap() });
    let http = reqwest::Client::new();

    let body = serde_json::json!({ "template": "build-check-prove", "intent": "x", "projectId": "p1", "dryRun": true });
    let r = http.post(format!("{base}/api/factory/runs")).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 400);
    let err: Value = r.json().await.unwrap();
    assert!(err["error"]["fact"].as_str().unwrap().contains("alpha, beta"), "{err}");

    let mut named = body.clone();
    named["team"] = "beta".into();
    let r = http.post(format!("{base}/api/factory/runs")).json(&named).send().await.unwrap();
    let plan: Value = r.json().await.unwrap();
    assert_eq!(plan["plan"]["nodes"][0]["executor"], "bot:builder", "{plan}");
}
