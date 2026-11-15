use allternit_studio_spec::fake_engine::{FakeConfig, FakeEngine};
use allternit_studio_spec::*;
use std::time::Duration;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn codes(name: &str) -> Vec<String> {
    JobSpec::parse(&fixture(name)).unwrap_err().into_iter().map(|e| e.code).collect()
}

#[test]
fn valid_fixture_round_trips() {
    let spec = JobSpec::parse(&fixture("lora-valid")).unwrap();
    let back: JobSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
    assert_eq!(spec, back);
    assert_eq!(spec.method, "lora");
}

#[test]
fn local_fixture_takes_absolute_paths_and_base_needs_one_source() {
    let spec = JobSpec::parse(&fixture("lora-valid-local")).unwrap();
    assert!(spec.base.repo.is_empty());
    assert!(std::path::Path::new(spec.base.path.as_deref().unwrap()).is_absolute());
    assert!(std::path::Path::new(spec.dataset.path.as_deref().unwrap()).is_absolute());
    assert_eq!(spec.resources.backend, Some(BackendChoice::Auto));

    let mut v: serde_json::Value = serde_json::from_str(&fixture("lora-valid-local")).unwrap();
    v["base"]["repo"] = "freeweights/qwen2.5-0.5b-instruct".into();
    assert_eq!(JobSpec::parse(&v.to_string()).unwrap_err()[0].code, "invalid_base");
    v["base"] = serde_json::json!({});
    assert_eq!(JobSpec::parse(&v.to_string()).unwrap_err()[0].code, "invalid_base");
}

#[test]
fn refusals_have_stable_codes() {
    assert_eq!(codes("invalid-rented"), ["rented_not_available"]);
    assert_eq!(codes("invalid-publish"), ["publish_not_available"]);
    assert_eq!(codes("invalid-engine"), ["unknown_engine"]);
    assert_eq!(codes("invalid-method"), ["unknown_method"]);
    assert_eq!(codes("invalid-method-engine"), ["unknown_method"]);
    assert_eq!(codes("invalid-dataset"), ["invalid_dataset"]);
}

#[test]
fn unknown_fields_and_bad_version_are_refused() {
    let mut v: serde_json::Value = serde_json::from_str(&fixture("lora-valid")).unwrap();
    v["spec_version"] = 2.into();
    assert_eq!(JobSpec::parse(&v.to_string()).unwrap_err()[0].code, "unsupported_spec_version");
    v["spec_version"] = 1.into();
    v["surprise"] = true.into();
    assert_eq!(JobSpec::parse(&v.to_string()).unwrap_err()[0].code, "invalid_spec");
}

#[test]
fn state_transitions() {
    use JobState::*;
    let path = [Queued, Preparing, Running, Exporting, Succeeded];
    for w in path.windows(2) {
        assert!(w[0].can_transition_to(w[1]), "{:?} -> {:?}", w[0], w[1]);
    }
    assert!(Queued.transition(Running).is_err(), "no skipping");
    assert!(Running.transition(Queued).is_err(), "no going back");
    for s in [Queued, Preparing, Running, Exporting] {
        assert!(s.can_transition_to(Failed) && s.can_transition_to(Cancelled));
    }
    for t in [Succeeded, Failed, Cancelled] {
        for to in [Queued, Preparing, Running, Exporting, Succeeded, Failed, Cancelled] {
            assert!(!t.can_transition_to(to), "terminal {t:?} must not move to {to:?}");
        }
    }
}

#[test]
fn catalog_matches_decisions() {
    let c = catalog();
    let get = |id: &str| c.iter().find(|e| e.id == id).unwrap();
    assert_eq!(get("allternit-tune").license, "Apache-2.0");
    assert!(get("allternit-tune").default);
    let mlx = get("allternit-tune").backends.iter().find(|b| b.id == "mlx").unwrap();
    assert_eq!(mlx.status, "available");
    assert_eq!(get("unsloth-upstream").license, "AGPL-3.0");
    // `unsloth train` has no DPO.
    assert!(!get("unsloth-upstream").methods.iter().any(|m| m == "dpo"));
    // allternit-tune covers every hardware id; every id in the catalog is a real one.
    assert_eq!(get("allternit-tune").hardware.len(), Hardware::ALL.len());
    for e in &c {
        for h in &e.hardware {
            assert!(Hardware::from_id(h).is_some(), "{}: unknown hardware {h}", e.id);
        }
    }
    assert!(!get("unsloth-upstream").published);
    // No adapter ships for these yet; they stay hidden until their engine kit is published.
    for id in ["axolotl", "llama-factory", "torchtune"] {
        assert!(!get(id).published);
    }
}

#[test]
fn yaml_files_parse_and_name_the_routes() {
    let dir = env!("CARGO_MANIFEST_DIR");
    for (f, paths) in [
        ("studio-api", vec!["/engines", "/engines/{id}/install", "/jobs", "/jobs/{id}", "/jobs/{id}/events", "/jobs/{id}/cancel", "/jobs/{id}/outputs", "/jobs/{id}/outputs/register"]),
        ("engine-api", vec!["/v1/engine", "/v1/jobs", "/v1/jobs/{id}", "/v1/jobs/{id}/events", "/v1/jobs/{id}/cancel"]),
    ] {
        let y: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(format!("{dir}/openapi/{f}.yaml")).unwrap()).unwrap();
        for p in paths {
            assert!(y["paths"][p].is_mapping(), "{f} lacks {p}");
        }
    }
}

struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    async fn get(&self, p: &str) -> reqwest::Response {
        self.http.get(format!("{}{p}", self.base)).bearer_auth(&self.token).send().await.unwrap()
    }
    async fn post(&self, p: &str, body: String) -> reqwest::Response {
        self.http.post(format!("{}{p}", self.base)).bearer_auth(&self.token).body(body).send().await.unwrap()
    }
}

async fn wait_terminal(c: &Client, id: &str) -> Vec<JobEvent> {
    let mut all: Vec<JobEvent> = vec![];
    for _ in 0..200 {
        let after = all.last().map_or(0, |e| e.seq);
        let v: serde_json::Value = c.get(&format!("/v1/jobs/{id}/events?after={after}")).await.json().await.unwrap();
        let evs: Vec<JobEvent> = serde_json::from_value(v["events"].clone()).unwrap();
        all.extend(evs);
        let st: JobState = serde_json::from_value(v["state"].clone()).unwrap();
        if st.is_terminal() { return all; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job never finished");
}

#[tokio::test]
async fn fake_engine_runs_fixture_job_to_succeeded() {
    let e = FakeEngine::start("tok", Duration::from_millis(5)).await.unwrap();
    let c = Client { http: reqwest::Client::new(), base: e.base_url(), token: "tok".into() };

    let info: EngineInfo = c.get("/v1/engine").await.json().await.unwrap();
    assert_eq!(info.install_state, InstallState::Installed);

    let r = c.post("/v1/jobs", fixture("lora-valid")).await;
    assert_eq!(r.status(), 201);
    let job: Job = r.json().await.unwrap();
    assert_eq!(job.state, JobState::Queued);

    // a second job while one runs is refused
    let r = c.post("/v1/jobs", fixture("lora-valid")).await;
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<ApiError>().await.unwrap().error, "job_running");

    let events = wait_terminal(&c, &job.id).await;
    assert!(events.windows(2).all(|w| w[1].seq == w[0].seq + 1), "seq strictly increasing by 1");
    assert_eq!(events[0].seq, 1);
    let states: Vec<&str> = events.iter().filter(|e| e.kind == "state").map(|e| e.message.as_str()).collect();
    assert_eq!(states, ["queued", "preparing", "running", "exporting", "succeeded"]);

    let done: Job = c.get(&format!("/v1/jobs/{}", job.id)).await.json().await.unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert_eq!(done.outputs[0].kind, "adapter");

    // `after` returns only newer events
    let v: serde_json::Value = c.get(&format!("/v1/jobs/{}/events?after={}", job.id, done.last_seq)).await.json().await.unwrap();
    assert!(v["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn fake_engine_refuses_bad_token_bad_spec_and_late_cancel() {
    let e = FakeEngine::start("tok", Duration::from_millis(5)).await.unwrap();
    let bad = Client { http: reqwest::Client::new(), base: e.base_url(), token: "nope".into() };
    assert_eq!(bad.get("/v1/engine").await.status(), 401);

    let c = Client { http: reqwest::Client::new(), base: e.base_url(), token: "tok".into() };
    let r = c.post("/v1/jobs", fixture("invalid-rented")).await;
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<ApiError>().await.unwrap().error, "rented_not_available");

    let job: Job = c.post("/v1/jobs", fixture("lora-valid")).await.json().await.unwrap();
    wait_terminal(&c, &job.id).await;
    assert_eq!(c.post(&format!("/v1/jobs/{}/cancel", job.id), String::new()).await.status(), 409);
}

#[tokio::test]
async fn fake_engine_cancel_stops_the_job() {
    let e = FakeEngine::start("tok", Duration::from_millis(200)).await.unwrap();
    let c = Client { http: reqwest::Client::new(), base: e.base_url(), token: "tok".into() };
    let job: Job = c.post("/v1/jobs", fixture("lora-valid")).await.json().await.unwrap();
    let r = c.post(&format!("/v1/jobs/{}/cancel", job.id), String::new()).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Job>().await.unwrap().state, JobState::Cancelled);
    let events = wait_terminal(&c, &job.id).await;
    assert_eq!(events.last().unwrap().message, "cancelled");
    // a new job may start once the old one is terminal
    assert_eq!(c.post("/v1/jobs", fixture("lora-valid")).await.status(), 201);
}

fn fit_table() -> serde_json::Value {
    serde_json::from_str(&fixture("fit-table")).unwrap()
}

/// engines.json + the fit logic must give exactly what fixtures/fit-table.json says.
#[test]
fn fit_matches_the_fit_table() {
    let t = fit_table();
    let ids: Vec<&str> = Hardware::ALL.iter().map(|h| h.id()).collect();
    assert_eq!(t["hardware"], serde_json::json!(ids));
    let table = t["engines"].as_object().unwrap();
    let cat = catalog();
    assert_eq!(table.len(), cat.len(), "fit table covers every engine");
    for engine in &cat {
        let row = &table[&engine.id];
        for hw in Hardware::ALL {
            let cell = &row[hw.id()];
            let f = fit(engine, &HostHardware::plain(hw));
            let at = format!("{} on {}", engine.id, hw.id());
            if let Some(reason) = cell.as_str() {
                assert!(!f.ok, "{at}: table refuses the engine");
                assert_eq!(f.reason, reason, "{at}");
                continue;
            }
            assert!(f.ok, "{at}: table allows the engine, fit said {}", f.reason);
            let methods = cell["methods"].as_object().unwrap();
            let mut listed: Vec<&String> = methods.keys().collect();
            listed.sort();
            let mut want: Vec<&String> = engine.methods.iter().collect();
            want.sort();
            assert_eq!(listed, want, "{at}: table lists the engine's methods");
            for (m, v) in methods {
                match v.as_str().unwrap() {
                    "ok" => {
                        assert!(!f.methods_unavailable.contains_key(m), "{at}: {m} should run");
                        let b = allternit_studio_spec::fit::backend_for(engine, hw, m, BackendChoice::Auto).unwrap();
                        assert_eq!(cell["backend"][m], b.as_str(), "{at}: backend for {m}");
                    }
                    reason => assert_eq!(f.methods_unavailable.get(m).map(String::as_str), Some(reason), "{at}: {m}"),
                }
            }
            match cell["outputs"]["mlx"].as_str().unwrap() {
                "ok" => assert!(!f.outputs_unavailable.contains_key("mlx"), "{at}: mlx output"),
                reason => assert_eq!(f.outputs_unavailable.get("mlx").map(String::as_str), Some(reason), "{at}: mlx output"),
            }
            assert_eq!(cell["warning"].as_str(), f.warning.as_deref(), "{at}: warning");
        }
    }
}

#[test]
fn classify_matches_the_fit_table() {
    for case in fit_table()["classify"].as_array().unwrap() {
        let gpus: Vec<GpuVendor> = serde_json::from_value(case["gpus"].clone()).unwrap();
        let got = classify(case["os"].as_str().unwrap(), case["arch"].as_str().unwrap(), &gpus);
        assert_eq!(got.hardware.id(), case["hardware"], "{case}");
        assert_eq!(got.reason.is_some(), case["cpu_reason"].as_bool().unwrap(), "{case}");
    }
}

fn spec_with(f: impl FnOnce(&mut serde_json::Value)) -> JobSpec {
    let mut v: serde_json::Value = serde_json::from_str(&fixture("lora-valid")).unwrap();
    f(&mut v);
    JobSpec::parse(&v.to_string()).unwrap()
}

fn job_codes(spec: &JobSpec, hw: Hardware) -> Vec<String> {
    check_job(spec, &HostHardware::plain(hw)).unwrap_err().into_iter().map(|e| e.code).collect()
}

#[test]
fn check_job_picks_the_backend_and_refuses_with_codes() {
    let mac = HostHardware::plain(Hardware::AppleSilicon);
    assert_eq!(check_job(&spec_with(|_| {}), &mac).unwrap().backend, "mlx");
    let full = spec_with(|v| v["method"] = "full".into());
    assert_eq!(check_job(&full, &mac).unwrap().backend, "torch-mps");
    let torch = spec_with(|v| v["resources"]["backend"] = "torch".into());
    assert_eq!(check_job(&torch, &mac).unwrap().backend, "torch-mps");

    let qlora = spec_with(|v| v["method"] = "qlora".into());
    assert_eq!(job_codes(&qlora, Hardware::Cpu), ["method_not_supported"]);
    assert_eq!(check_job(&qlora, &HostHardware::plain(Hardware::XpuWindows)).unwrap().backend, "torch-xpu");

    let mlx_out = spec_with(|v| v["outputs"]["mlx"] = true.into());
    assert!(check_job(&mlx_out, &mac).is_ok());
    assert_eq!(job_codes(&mlx_out, Hardware::CudaLinux), ["output_not_supported"]);

    let mlx_backend = spec_with(|v| v["resources"]["backend"] = "mlx".into());
    assert_eq!(job_codes(&mlx_backend, Hardware::RocmLinux), ["backend_not_supported"]);
    let mlx_full = spec_with(|v| {
        v["resources"]["backend"] = "mlx".into();
        v["method"] = "dpo".into();
    });
    assert_eq!(job_codes(&mlx_full, Hardware::AppleSilicon), ["backend_not_supported"]);

    let axolotl = spec_with(|v| v["engine"] = "axolotl".into());
    assert_eq!(job_codes(&axolotl, Hardware::AppleSilicon), ["hardware_not_supported"]);

    // An AMD GPU on Windows falls back to the CPU, and the reason says why.
    let amd_win = classify("windows", "x86_64", &[GpuVendor::Amd]);
    let errs = check_job(&qlora, &amd_win).unwrap_err();
    assert_eq!(errs[0].code, "method_not_supported");
    let card = fit(&allternit_studio_spec::engines::find("allternit-tune").unwrap(), &amd_win);
    assert!(card.ok && card.warning.is_some());
    assert!(card.reason.contains("AMD"), "{}", card.reason);
}

#[tokio::test]
async fn fake_engine_honours_per_method_fit() {
    let out = std::env::temp_dir().join("studio-spec-fit-test");
    let cfg = FakeConfig { hardware: HostHardware::plain(Hardware::Cpu), output_dir: Some(out.clone()) };
    let e = FakeEngine::start_with("tok", Duration::from_millis(5), cfg).await.unwrap();
    let c = Client { http: reqwest::Client::new(), base: e.base_url(), token: "tok".into() };

    let card: serde_json::Value = c.get("/v1/engine").await.json().await.unwrap();
    assert_eq!(card["fit"]["hardware"], "cpu");
    assert_eq!(card["fit"]["backend"], "torch-cpu");
    assert!(card["fit"]["warning"].is_string());
    let want = &fit_table()["engines"]["allternit-tune"]["cpu"]["methods"]["qlora"];
    assert_eq!(&card["fit"]["methods_unavailable"]["qlora"], want);

    let mut v: serde_json::Value = serde_json::from_str(&fixture("lora-valid")).unwrap();
    v["method"] = "qlora".into();
    let r = c.post("/v1/jobs", v.to_string()).await;
    assert_eq!(r.status(), 422);
    let body = r.json::<ApiError>().await.unwrap();
    assert_eq!(body.error, "method_not_supported");
    assert_eq!(serde_json::Value::from(body.message), *want);

    let job: Job = c.post("/v1/jobs", fixture("lora-valid")).await.json().await.unwrap();
    wait_terminal(&c, &job.id).await;
    let done: Job = c.get(&format!("/v1/jobs/{}", job.id)).await.json().await.unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    let kinds: Vec<&str> = done.outputs.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, ["adapter", "gguf"]);
    for o in &done.outputs {
        let p = std::path::Path::new(&o.path);
        assert!(p.is_absolute() && p.starts_with(&out), "{}", o.path);
    }
}
