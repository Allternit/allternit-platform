use allternit_studio_spec::fake_engine::FakeEngine;
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
    assert_eq!(mlx.status, "pending_decision");
    assert_eq!(get("unsloth-upstream").license, "AGPL-3.0");
    assert!(!get("unsloth-upstream").published);
    for id in ["axolotl", "llama-factory", "torchtune"] {
        assert!(get(id).published);
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
