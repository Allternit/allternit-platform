//! Studio host tests through the HTTP routes, against the contract crate's
//! fake engine (engine API v1 over loopback with a bearer token).
//!
//! The fake engine serves in process; a `sleep` child stands in for the
//! engine's process so pid checks, kill and reattach run for real.

use super::engines::{EngineDirs, Hardware};
use super::runner::{pid_alive, EngineLauncher, LaunchRequest, Launched};
use super::*;
use allternit_studio_spec::fake_engine::FakeEngine;
use axum::body::Body;
use axum::http::Request;
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt;

const FIXTURE: &str = include_str!("../../../../services/studio-spec/fixtures/lora-valid.json");

struct FakeLauncher {
    delay: Duration,
    engines: Mutex<Vec<FakeEngine>>,
}

#[async_trait::async_trait]
impl EngineLauncher for FakeLauncher {
    fn installed(&self, engine_id: &str, _dirs: &EngineDirs) -> bool {
        engine_id == "allternit-tune"
    }

    async fn launch(&self, req: LaunchRequest) -> Result<Launched, String> {
        let engine = FakeEngine::start(&req.token, self.delay)
            .await
            .map_err(|e| e.to_string())?;
        let port = engine.addr.port();
        // Kept here, not in the host, so it outlives a simulated host crash.
        self.engines.lock().unwrap().push(engine);
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("600");
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let pid = child.id().unwrap();
        tokio::spawn(async move {
            let _ = child.wait().await; // reap, so a killed pid really disappears
        });
        Ok(Launched {
            pid,
            port,
            child: None,
            guard: None,
        })
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    dataset: PathBuf,
    launcher: Arc<FakeLauncher>,
}

fn env(step_delay: Duration) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("studio");
    // A base model already fetched (cache hit), so tests never touch the network.
    let base = dir
        .join("models")
        .join("freeweights__qwen2.5-0.5b-instruct")
        .join("main");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("config.json"), "{}").unwrap();
    std::fs::write(base.join(".studio-complete"), "main\n").unwrap();
    // Fetched with the signed manifest the fixture pins (base.manifest_sha256).
    let pinned: Value = serde_json::from_str(FIXTURE).unwrap();
    std::fs::write(
        base.join(super::runner::MANIFEST_SHA_FILE),
        pinned["base"]["manifest_sha256"].as_str().unwrap(),
    )
    .unwrap();
    let dataset = tmp.path().join("train.jsonl");
    std::fs::write(&dataset, "{\"messages\":[]}\n").unwrap();
    Env {
        _tmp: tmp,
        dir,
        dataset,
        launcher: Arc::new(FakeLauncher {
            delay: step_delay,
            engines: Mutex::default(),
        }),
    }
}

fn host(env: &Env, hardware: Hardware) -> StudioHost {
    StudioHost::new(StudioConfig {
        dir: env.dir.clone(),
        hardware,
        launcher: env.launcher.clone(),
        local_engine_url: "http://127.0.0.1:9".into(),
        poll_interval: Duration::from_millis(20),
        start_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_secs(2),
        // Nothing listens here: a test that reaches Freeweights fails fast.
        freeweights_url: Some("http://127.0.0.1:9".into()),
    })
}

fn fixture(env: &Env) -> Value {
    let mut spec: Value = serde_json::from_str(FIXTURE).unwrap();
    spec["dataset"]["path"] = json!(env.dataset.to_string_lossy());
    spec
}

async fn call(
    host: &StudioHost,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let app: Router = router(host.clone());
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-allternit-user-id", "user_test")
        .header("content-type", "application/json")
        .body(
            body.map(|b| Body::from(b.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

use axum::Router;

async fn wait_state(host: &StudioHost, id: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, job) = call(host, "GET", &format!("/studio/jobs/{id}"), None).await;
        if job["state"] == want {
            return job;
        }
        assert!(Instant::now() < deadline, "job never reached {want}: {job}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

use std::time::Instant;

fn engine_pid(dir: &Path, id: &str) -> u32 {
    let p: Value = serde_json::from_slice(
        &std::fs::read(dir.join("jobs").join(id).join("engine.json")).unwrap(),
    )
    .unwrap();
    p["pid"].as_u64().unwrap() as u32
}

async fn wait_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} still alive");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Simulates allternit-api dying: runner tasks stop, engine processes keep going.
fn crash(host: &StudioHost) {
    for (_, a) in host.0.active.lock().unwrap().drain() {
        if let Some(t) = a.task.lock().unwrap().take() {
            t.abort();
        }
        std::mem::forget(a.engine.lock().unwrap().take());
    }
}

#[tokio::test]
async fn studio_fixture_lora_job_succeeds_through_routes() {
    let env = env(Duration::from_millis(30));
    let host = host(&env, Hardware::AppleSilicon);

    let (status, job) = call(&host, "POST", "/studio/jobs", Some(fixture(&env))).await;
    assert_eq!(status, StatusCode::CREATED, "{job}");
    assert_eq!(job["state"], "queued");
    let id = job["id"].as_str().unwrap().to_string();

    let done = wait_state(&host, &id, "succeeded").await;
    assert!(done["error"].is_null());

    let (_, page) = call(
        &host,
        "GET",
        &format!("/studio/jobs/{id}/events?after=0"),
        None,
    )
    .await;
    let events = page["events"].as_array().unwrap();
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "seq must start at 1 and rise by 1"
    );
    let states: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "state")
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        ["queued", "preparing", "running", "exporting", "succeeded"]
    );
    assert!(
        events.iter().any(|e| e["kind"] == "progress"),
        "engine progress events are copied"
    );

    let after = seqs[seqs.len() - 3];
    let (_, tail) = call(
        &host,
        "GET",
        &format!("/studio/jobs/{id}/events?after={after}"),
        None,
    )
    .await;
    assert_eq!(tail["events"].as_array().unwrap().len(), 2);

    let (_, outs) = call(&host, "GET", &format!("/studio/jobs/{id}/outputs"), None).await;
    let adapter = &outs["outputs"][0];
    assert_eq!(adapter["kind"], "adapter");
    let job_dir = env.dir.join("jobs").join(&id);
    assert!(adapter["path"]
        .as_str()
        .unwrap()
        .starts_with(&*job_dir.join("outputs").to_string_lossy()));

    // Stored as files; the engine record is gone once the engine stopped.
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(job_dir.join("job.json")).unwrap()).unwrap();
    assert_eq!(on_disk["state"], "succeeded");
    assert!(!job_dir.join("engine.json").exists());
    let lines = std::fs::read_to_string(job_dir.join("events.jsonl")).unwrap();
    assert_eq!(lines.lines().count(), seqs.len());

    // A new host reading the same folder sees the finished job.
    let again = super::StudioHost::new(StudioConfig {
        dir: env.dir.clone(),
        ..host_cfg(&env)
    });
    assert_eq!(again.job(&id).unwrap().state, JobState::Succeeded);
}

fn host_cfg(env: &Env) -> StudioConfig {
    StudioConfig {
        dir: env.dir.clone(),
        hardware: Hardware::AppleSilicon,
        launcher: env.launcher.clone(),
        local_engine_url: "http://127.0.0.1:9".into(),
        poll_interval: Duration::from_millis(20),
        start_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_secs(2),
        freeweights_url: Some("http://127.0.0.1:9".into()),
    }
}

#[tokio::test]
async fn studio_second_job_while_one_runs_is_409() {
    let env = env(Duration::from_millis(300));
    let host = host(&env, Hardware::AppleSilicon);
    let (status, job) = call(&host, "POST", "/studio/jobs", Some(fixture(&env))).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, err) = call(&host, "POST", "/studio/jobs", Some(fixture(&env))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"], "job_running");
    let id = job["id"].as_str().unwrap();
    call(&host, "POST", &format!("/studio/jobs/{id}/cancel"), None).await;
    // Free again after the cancel.
    let (status, job2) = call(&host, "POST", "/studio/jobs", Some(fixture(&env))).await;
    assert_eq!(status, StatusCode::CREATED, "{job2}");
    call(
        &host,
        "POST",
        &format!("/studio/jobs/{}/cancel", job2["id"].as_str().unwrap()),
        None,
    )
    .await;
}

#[tokio::test]
async fn studio_cancel_stops_the_engine_process() {
    let env = env(Duration::from_millis(300));
    let host = host(&env, Hardware::AppleSilicon);
    let (_, job) = call(&host, "POST", "/studio/jobs", Some(fixture(&env))).await;
    let id = job["id"].as_str().unwrap().to_string();
    wait_state(&host, &id, "running").await;
    let pid = engine_pid(&env.dir, &id);
    assert!(pid_alive(pid));

    let (status, job) = call(&host, "POST", &format!("/studio/jobs/{id}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK, "{job}");
    assert_eq!(job["state"], "cancelled");
    wait_gone(pid).await;
    assert!(!env.dir.join("jobs").join(&id).join("engine.json").exists());

    let (status, err) = call(&host, "POST", &format!("/studio/jobs/{id}/cancel"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"], "already_finished");

    // Nothing moves the job after the cancel.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(host.job(&id).unwrap().state, JobState::Cancelled);
}

#[tokio::test]
async fn studio_restart_reattaches_a_live_engine() {
    let env = env(Duration::from_millis(250));
    let first = host(&env, Hardware::AppleSilicon);
    let (_, job) = call(&first, "POST", "/studio/jobs", Some(fixture(&env))).await;
    let id = job["id"].as_str().unwrap().to_string();
    wait_state(&first, &id, "running").await;
    crash(&first);

    let second = host(&env, Hardware::AppleSilicon);
    second.recover().await;
    let done = wait_state(&second, &id, "succeeded").await;
    assert_eq!(done["outputs"][0]["kind"], "adapter");
    let (_, page) = call(&second, "GET", &format!("/studio/jobs/{id}/events"), None).await;
    let events = page["events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|e| e["message"].as_str().unwrap().starts_with("Reattached")));
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
}

#[tokio::test]
async fn studio_restart_marks_job_failed_when_engine_died() {
    let env = env(Duration::from_millis(300));
    let first = host(&env, Hardware::AppleSilicon);
    let (_, job) = call(&first, "POST", "/studio/jobs", Some(fixture(&env))).await;
    let id = job["id"].as_str().unwrap().to_string();
    wait_state(&first, &id, "running").await;
    let pid = engine_pid(&env.dir, &id);
    crash(&first);
    // SAFETY: test-owned stand-in process.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    wait_gone(pid).await;

    let second = host(&env, Hardware::AppleSilicon);
    second.recover().await;
    let job = second.job(&id).unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert!(job
        .error
        .unwrap()
        .contains("stopped while Allternit was restarting"));
    // The computer is free for a new job.
    let (status, _) = call(&second, "POST", "/studio/jobs", Some(fixture(&env))).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn studio_refusals_and_engine_list() {
    let env = env(Duration::from_millis(30));
    let pc = host(&env, Hardware::Unsupported);
    let (status, err) = call(&pc, "POST", "/studio/jobs", Some(fixture(&env))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["error"], "hardware_not_supported");

    let mac = host(&env, Hardware::AppleSilicon);
    let mut rented = fixture(&env);
    rented["resources"]["place"] = json!("rented");
    let (status, err) = call(&mac, "POST", "/studio/jobs", Some(rented)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["error"], "rented_not_available");

    let mut missing = fixture(&env);
    missing["dataset"]["path"] = json!("/nonexistent/train.jsonl");
    let (_, job) = call(&mac, "POST", "/studio/jobs", Some(missing)).await;
    let failed = wait_state(&mac, job["id"].as_str().unwrap(), "failed").await;
    assert!(
        failed["error"].as_str().unwrap().contains("was not found"),
        "{failed}"
    );

    let (_, list) = call(&mac, "GET", "/studio/engines", None).await;
    let engines = list["engines"].as_array().unwrap();
    let tune = engines
        .iter()
        .find(|e| e["id"] == "allternit-tune")
        .unwrap();
    assert_eq!(tune["install_state"], "installed");
    assert_eq!(tune["fit"]["ok"], true);
    let unsloth = engines
        .iter()
        .find(|e| e["id"] == "unsloth-upstream")
        .unwrap();
    assert_eq!(unsloth["fit"]["ok"], false);

    let (status, err) = call(&mac, "POST", "/studio/engines/axolotl/install", None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(err["message"].as_str().unwrap().contains("not available"));
}

#[tokio::test]
async fn studio_register_output_calls_local_engine() {
    // local-engine is a separate service: stand it up as an HTTP edge.
    let seen = Arc::new(Mutex::new(None::<Value>));
    let seen2 = seen.clone();
    let le = axum::Router::new().route(
        "/models/studio-output",
        axum::routing::post(move |axum::Json(b): axum::Json<Value>| {
            let seen = seen2.clone();
            async move {
                *seen.lock().unwrap() = Some(b);
                axum::Json(json!({ "id": "studio--job--adapter", "source": "studio_output" }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, le).await.unwrap() });

    let env = env(Duration::from_millis(20));
    let host = StudioHost::new(StudioConfig {
        local_engine_url: format!("http://{addr}"),
        ..host_cfg(&env)
    });
    let (_, job) = call(&host, "POST", "/studio/jobs", Some(fixture(&env))).await;
    let id = job["id"].as_str().unwrap().to_string();

    let (status, err) = call(
        &host,
        "POST",
        &format!("/studio/jobs/{id}/outputs/register"),
        Some(json!({"kind": "adapter"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");

    wait_state(&host, &id, "succeeded").await;
    let (status, body) = call(
        &host,
        "POST",
        &format!("/studio/jobs/{id}/outputs/register"),
        Some(json!({"kind": "adapter"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["model_id"], "studio--job--adapter");
    let sent = seen.lock().unwrap().clone().unwrap();
    assert_eq!(sent["job_id"], id.as_str());
    assert!(sent["path"].as_str().unwrap().ends_with("adapter"));

    let (status, _) = call(
        &host,
        "POST",
        &format!("/studio/jobs/{id}/outputs/register"),
        Some(json!({"kind": "gguf"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Serves the real signed Qwen3-Embedding manifest from model-source testdata
/// the way Freeweights does (`/manifests/<org>/<repo>.json` + `.minisig`).
async fn serve_signed_manifest() -> (String, String) {
    const QWEN_MANIFEST: &[u8] = include_bytes!(
        "../../../../services/model-source/testdata/Qwen__Qwen3-Embedding-0.6B.json"
    );
    const QWEN_SIG: &[u8] = include_bytes!(
        "../../../../services/model-source/testdata/Qwen__Qwen3-Embedding-0.6B.json.minisig"
    );
    let app = axum::Router::new()
        .route(
            "/manifests/Qwen/Qwen3-Embedding-0.6B.json",
            axum::routing::get(|| async { QWEN_MANIFEST.to_vec() }),
        )
        .route(
            "/manifests/Qwen/Qwen3-Embedding-0.6B.json.minisig",
            axum::routing::get(|| async { QWEN_SIG.to_vec() }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    use sha2::{Digest, Sha256};
    (
        format!("http://{addr}"),
        hex::encode(Sha256::digest(QWEN_MANIFEST)),
    )
}

#[tokio::test]
async fn studio_manifest_pin_match_and_mismatch() {
    const QWEN: &str = "Qwen/Qwen3-Embedding-0.6B";
    let (base, manifest_sha) = serve_signed_manifest().await;
    let env = env(Duration::from_millis(20));
    let host = StudioHost::new(StudioConfig {
        freeweights_url: Some(base.clone()),
        ..host_cfg(&env)
    });
    let other = "ab".repeat(32);

    // Mismatch through the route: refused with 422 before any job exists.
    let mut wrong = fixture(&env);
    wrong["base"]["manifest_sha256"] = json!(other);
    let (status, err) = call(&host, "POST", "/studio/jobs", Some(wrong)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["error"], "manifest_mismatch");
    let (_, jobs) = call(&host, "GET", "/studio/jobs", None).await;
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 0);

    // Against the manifest Freeweights serves now (nothing cached yet).
    let spec_for = |pin: &str| {
        let mut v = fixture(&env);
        v["base"] = json!({ "repo": QWEN, "manifest_sha256": pin });
        JobSpec::parse(&v.to_string()).unwrap()
    };
    host.check_manifest_pin(&spec_for(&manifest_sha))
        .await
        .unwrap();
    let HostError(status, body) = host
        .check_manifest_pin(&spec_for(&other))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.error, "manifest_mismatch");
    assert!(body.message.contains(&manifest_sha), "{}", body.message);
    // A revision other than the manifest's pinned commit has no signed manifest.
    let mut off_rev = spec_for(&manifest_sha);
    off_rev.base.revision = Some("refs/pr/1".into());
    let HostError(_, body) = host.check_manifest_pin(&off_rev).await.unwrap_err();
    assert_eq!(body.error, "manifest_mismatch");

    // The runner refuses too (the manifest could change after the job was
    // accepted), before downloading any file.
    let client = host.model_source().unwrap();
    let root = env.dir.join("models");
    let err = super::runner::fetch_repo(&client, &root, QWEN, None, true, Some(&other))
        .await
        .unwrap_err();
    assert!(err.contains("not the one this job pins"), "{err}");
    assert!(!super::runner::repo_dir(&root, QWEN, None)
        .join(".studio-complete")
        .exists());

    // Match through the route: a base fetched with the pinned manifest is used.
    let mut ok = fixture(&env);
    ok["base"]["manifest_sha256"] = json!(manifest_sha);
    let cached = super::runner::repo_dir(&root, "freeweights/qwen2.5-0.5b-instruct", Some("main"));
    std::fs::write(cached.join(super::runner::MANIFEST_SHA_FILE), &manifest_sha).unwrap();
    let (status, job) = call(&host, "POST", "/studio/jobs", Some(ok)).await;
    assert_eq!(status, StatusCode::CREATED, "{job}");
    wait_state(&host, job["id"].as_str().unwrap(), "succeeded").await;
}
