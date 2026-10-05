//! CommRails bridge: scoped identities, forbidden capabilities, bind policy,
//! ledger provenance, rate limiting, and the box-side client's mirror output.
//! Every listener here binds 127.0.0.1:0 — nothing is exposed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use allternit_factory_engine::bridge::{
    serve_listener, BridgeState, IdentityStore, Scope, GRANTABLE_SCOPES,
};
use allternit_factory_engine::gate::gate::DagMutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::{
    ActorType, AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("commrails-bridge-")
        .tempdir_in(base)
        .unwrap()
}

struct Bridge {
    _tmp: TempDir,
    root: PathBuf,
    url: String,
    store: IdentityStore,
    ledger: Arc<Ledger>,
}

async fn start(rate_limit_per_min: u32) -> Bridge {
    let tmp = test_root();
    let root = tmp.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let store = IdentityStore::new(tmp.path().join("identities.json"));
    let state = BridgeState::new(root.clone(), store.clone(), rate_limit_per_min)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(addr.ip().is_loopback());
    tokio::spawn(serve_listener(listener, state));
    let ledger = Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.clone()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }));
    Bridge {
        _tmp: tmp,
        root,
        url: format!("http://{addr}"),
        store,
        ledger,
    }
}

fn chief(b: &Bridge, scopes: &[Scope]) -> String {
    b.store.add("bot:chief", scopes, None).unwrap().token
}

async fn call(
    b: &Bridge,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let client = reqwest::Client::new();
    let mut req = client.request(method, format!("{}{}", b.url, path));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn events(b: &Bridge) -> Vec<AllternitEvent> {
    b.ledger.query(LedgerQuery::default()).await.unwrap()
}

async fn local_gate(root: &Path, ledger: Arc<Ledger>) -> Gate {
    let leases = Arc::new(
        Leases::new(LeasesOptions {
            root_dir: Some(root.to_path_buf()),
            leases_dir: Some(PathBuf::from(".allternit/leases")),
            event_sink: Some(ledger.clone()),
            actor_id: Some("gate".to_string()),
            auto_renewal_enabled: false,
            auto_renewal_threshold_seconds: 300,
            auto_renewal_interval_seconds: 60,
            auto_renewal_extend_seconds: 600,
        })
        .await
        .unwrap(),
    );
    let receipts = Arc::new(
        ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.to_path_buf()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })
        .unwrap(),
    );
    Gate::new(GateOptions {
        ledger,
        leases,
        receipts,
        index: None,
        vault: None,
        oauth_vault: None,
        root_dir: Some(root.to_path_buf()),
        actor_id: Some("gate".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    })
}

use reqwest::Method;

#[tokio::test]
async fn missing_bad_and_revoked_tokens_are_401() {
    let b = start(60).await;
    let token = chief(&b, &[Scope::PlanRead]);

    let (s, body) = call(&b, Method::GET, "/v1/whoami", None, None).await;
    assert_eq!(s, 401, "{body}");
    let (s, _) = call(
        &b,
        Method::GET,
        "/v1/whoami",
        Some("crb_notarealtoken"),
        None,
    )
    .await;
    assert_eq!(s, 401);
    let (s, _) = call(&b, Method::GET, "/v1/whoami", Some("garbage"), None).await;
    assert_eq!(s, 401);
    // Unauthenticated requests never reach a scoped route, even forbidden ones.
    let (s, _) = call(&b, Method::POST, "/v1/wihs/pickup", None, Some(json!({}))).await;
    assert_eq!(s, 401);

    let (s, who) = call(&b, Method::GET, "/v1/whoami", Some(&token), None).await;
    assert_eq!(s, 200);
    assert_eq!(who["actor"], "bot:chief");
    assert_eq!(who["scopes"], json!(["plan:read"]));

    // Revoke = one call; effective on the very next request, no restart.
    assert_eq!(b.store.revoke(None, Some("bot:chief")).unwrap().len(), 1);
    let (s, _) = call(&b, Method::GET, "/v1/whoami", Some(&token), None).await;
    assert_eq!(s, 401);

    let denied: Vec<_> = events(&b)
        .await
        .into_iter()
        .filter(|e| e.r#type == "BridgeRequestDenied")
        .collect();
    assert!(denied.len() >= 4, "every 401 is audited: {}", denied.len());
    assert!(denied.iter().all(|e| e.payload["status"] == 401));
}

#[tokio::test]
async fn execution_lease_and_approval_routes_are_403_even_if_the_file_claims_them() {
    let b = start(600).await;
    let token = chief(&b, &GRANTABLE_SCOPES);

    // Hand-edit the identities file to claim every forbidden scope.
    let path = b.store.path().to_path_buf();
    let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let scopes = file["identities"][0]["scopes"].as_array_mut().unwrap();
    for s in [
        "wih:pickup",
        "wih:close",
        "lease:*",
        "lease:request",
        "wait-gate:resolve",
        "gate:*",
        "plan:refine",
    ] {
        scopes.push(json!(s));
    }
    std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

    // Make a real plan so the refused calls have real targets.
    let (s, plan) = call(
        &b,
        Method::POST,
        "/v1/plan",
        Some(&token),
        Some(json!({"text": "target"})),
    )
    .await;
    assert_eq!(s, 201, "{plan}");
    let dag_id = plan["dag_id"].as_str().unwrap().to_string();
    let node_id = plan["node_id"].as_str().unwrap().to_string();

    let cases: Vec<(Method, String, &str)> = vec![
        (Method::POST, "/v1/wihs/pickup".into(), "wih:pickup"),
        (Method::POST, "/v1/wihs/wih_1/sign".into(), "wih:pickup"),
        (Method::POST, "/v1/wihs/wih_1/close".into(), "wih:close"),
        (Method::POST, "/v1/leases".into(), "lease:*"),
        (Method::DELETE, "/v1/leases/lease_1".into(), "lease:*"),
        (Method::POST, "/v1/leases/lease_1/renew".into(), "lease:*"),
        (Method::POST, "/v1/mail/reserve".into(), "lease:*"),
        (
            Method::POST,
            "/v1/wait-gates/resolve".into(),
            "wait-gate:resolve",
        ),
        (Method::POST, "/v1/gate/decision".into(), "gate:*"),
        (Method::POST, "/v1/gate/mutate".into(), "gate:*"),
        (Method::POST, "/v1/mail/decide".into(), "gate:*"),
        (Method::POST, "/v1/plan/refine".into(), "plan:refine"),
    ];
    for (method, path, cap) in cases {
        let body = json!({"dag_id": dag_id, "node_id": node_id, "role": "agent", "approve": true});
        let (s, resp) = call(&b, method.clone(), &path, Some(&token), Some(body)).await;
        assert_eq!(s, 403, "{method} {path} -> {resp}");
        assert_eq!(resp["error"], "forbidden_capability", "{path}");
        assert_eq!(resp["capability"], cap, "{path}");
    }
    // Not-a-bridge-route (full service surface) is simply absent.
    let (s, _) = call(
        &b,
        Method::POST,
        "/v1/ledger/tail",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 404);
    // Effective scopes never include the hand-edited ones.
    let (_, who) = call(&b, Method::GET, "/v1/whoami", Some(&token), None).await;
    let effective: Vec<String> = serde_json::from_value(who["scopes"].clone()).unwrap();
    assert!(effective
        .iter()
        .all(|s| !s.starts_with("wih:") && !s.starts_with("lease") && !s.starts_with("gate")));

    let evs = events(&b).await;
    for ty in [
        "WIHPickedUp",
        "WIHCreated",
        "WIHClosedSigned",
        "LeaseGranted",
        "DagNodeWaitGateResolved",
    ] {
        assert!(
            evs.iter().all(|e| e.r#type != ty),
            "{ty} must not be emitted"
        );
    }
    let refused = evs
        .iter()
        .filter(|e| e.r#type == "BridgeRequest" && e.payload["status"] == 403)
        .count();
    assert_eq!(refused, 12, "each 403 is audited with the caller as actor");
}

#[tokio::test]
async fn missing_scope_is_403() {
    let b = start(60).await;
    let reader = chief(&b, &[Scope::PlanRead]);
    let (s, resp) = call(
        &b,
        Method::POST,
        "/v1/plan",
        Some(&reader),
        Some(json!({"text": "x"})),
    )
    .await;
    assert_eq!(s, 403);
    assert_eq!(resp["required_scope"], "plan:create");
    let (s, _) = call(
        &b,
        Method::POST,
        "/v1/mail/send",
        Some(&reader),
        Some(json!({"thread_id": "mail:x", "body": "b"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(&b, Method::GET, "/v1/templates", Some(&reader), None).await;
    assert_eq!(s, 403);
    assert!(events(&b).await.iter().all(|e| e.r#type != "PromptCreated"));
}

#[tokio::test]
async fn non_loopback_bind_is_refused_by_the_cli() {
    let tmp = test_root();
    let ids = tmp.path().join("identities.json");
    let bin = env!("CARGO_BIN_EXE_allternit-factory");
    let serve = |bind: &str, allow_remote: bool| {
        let mut cmd = Command::new(bin);
        cmd.args(["internal", "rails", "bridge", "serve", "--bind", bind, "--root"])
            .arg(tmp.path())
            .arg("--identities")
            .arg(&ids);
        if allow_remote {
            cmd.arg("--allow-remote");
        }
        cmd.output().unwrap()
    };
    // Mesh-shaped address without --allow-remote: refused before bind.
    let out = serve("100.64.0.9:7433", false);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--allow-remote"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // --allow-remote but no identity: refused.
    let out = serve("100.64.0.9:7433", true);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no active identities"));
    // Forbidden scopes are refused at grant time by the CLI.
    let out = Command::new(bin)
        .args([
            "internal",
            "rails",
            "identity",
            "add",
            "--actor",
            "bot:chief",
            "--scopes",
            "plan:read,wih:pickup",
            "--identities",
        ])
        .arg(&ids)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!ids.exists(), "a refused grant writes nothing");
    // A valid grant prints the token once on stdout.
    let out = Command::new(bin)
        .args([
            "internal",
            "rails",
            "identity",
            "add",
            "--actor",
            "bot:chief",
            "--scopes",
            "plan:read",
            "--identities",
        ])
        .arg(&ids)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout)
        .trim()
        .starts_with("crb_"));
    // Unspecified address is refused even with the flag and an identity.
    let out = serve("0.0.0.0:7433", true);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("every interface"));
}

#[tokio::test]
async fn created_plans_carry_the_remote_actor_in_the_ledger() {
    let b = start(60).await;
    let token = chief(
        &b,
        &[
            Scope::PlanCreate,
            Scope::PlanRead,
            Scope::TemplateInstantiate,
        ],
    );

    let (s, plan) = call(
        &b,
        Method::POST,
        "/v1/plan",
        Some(&token),
        Some(json!({"text": "ship the P0-2 brief", "decision_ref": "chief-dec-42"})),
    )
    .await;
    assert_eq!(s, 201, "{plan}");
    let prompt_id = plan["prompt_id"].as_str().unwrap();
    let dag_id = plan["dag_id"].as_str().unwrap();
    assert_eq!(plan["submitted_by"], "bot:chief");

    let evs = events(&b).await;
    let prompt = evs
        .iter()
        .find(|e| e.r#type == "PromptCreated" && e.payload["prompt_id"] == prompt_id)
        .expect("PromptCreated");
    assert_eq!(prompt.actor.id, "bot:chief");
    assert_eq!(prompt.actor.r#type, ActorType::Agent);
    assert_eq!(prompt.payload["source"], "bridge");
    assert_eq!(prompt.payload["submitted_by"], "bot:chief");
    assert_eq!(prompt.payload["decision_ref"], "chief-dec-42");
    let request_id = prompt.payload["request_id"].as_str().unwrap();

    // Gate 0: DAG mutations point at the remote actor's prompt.
    let created = evs
        .iter()
        .find(|e| e.r#type == "DagCreated" && e.payload["dag_id"] == dag_id)
        .unwrap();
    assert_eq!(
        created.provenance.as_ref().unwrap().prompt_id.as_deref(),
        Some(prompt_id)
    );
    let delta = evs
        .iter()
        .find(|e| e.r#type == "PromptDeltaAppended" && e.payload["prompt_id"] == prompt_id)
        .unwrap();
    assert_eq!(delta.payload["author"], "bot:chief");

    // The request audit joins the plan by request_id and dag.
    let audit = evs
        .iter()
        .find(|e| e.r#type == "BridgeRequest" && e.payload["request_id"] == request_id)
        .expect("BridgeRequest");
    assert_eq!(audit.actor.id, "bot:chief");
    assert_eq!(audit.payload["status"], 201);
    assert_eq!(audit.payload["target_dag"], dag_id);
    assert_eq!(
        audit.scope.as_ref().unwrap().dag_id.as_deref(),
        Some(dag_id)
    );

    // Template path: store id only, instantiated with the same attribution.
    let tdir = b.root.join(".allternit/rails/templates");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(
        tdir.join("chief-brief.json"),
        serde_json::to_string(&json!({
            "id": "chief-brief",
            "name": "Chief brief",
            "description": "two-step brief",
            "params": [{"name": "topic"}],
            "steps": [
                {"id": "research", "title": "Research {{ params.topic }}", "executor": "bot:chief"},
                {"id": "write", "title": "Write", "description": "From {{ research.output }}", "blocked_by": ["research"]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let (s, list) = call(&b, Method::GET, "/v1/templates", Some(&token), None).await;
    assert_eq!(s, 200);
    assert_eq!(list["templates"][0]["id"], "chief-brief");
    let (s, bad) = call(
        &b,
        Method::POST,
        "/v1/templates/chief-brief/instantiate",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 422, "missing required param: {bad}");
    let (s, _) = call(
        &b,
        Method::POST,
        "/v1/templates/..%2Fetc/instantiate",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert!(
        s == 400 || s == 404,
        "path-ish template ids are refused, got {s}"
    );
    let (s, inst) = call(
        &b,
        Method::POST,
        "/v1/templates/chief-brief/instantiate",
        Some(&token),
        Some(json!({"params": {"topic": "Jev"}})),
    )
    .await;
    assert_eq!(s, 201, "{inst}");
    let t_dag = inst["dag_id"].as_str().unwrap();
    let (s, shown) = call(
        &b,
        Method::GET,
        &format!("/v1/plan/{t_dag}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(
        shown["dag"]["nodes"].as_object().unwrap().len(),
        3,
        "root + 2 steps"
    );
    let evs = events(&b).await;
    let t_prompt = evs
        .iter()
        .find(|e| e.r#type == "PromptCreated" && e.payload["prompt_id"] == inst["prompt_id"])
        .unwrap();
    assert_eq!(t_prompt.actor.id, "bot:chief");
    let refine = evs
        .iter()
        .find(|e| e.r#type == "PromptDeltaAppended" && e.payload["delta_id"] == inst["delta_id"])
        .unwrap();
    assert_eq!(refine.payload["author"], "bot:chief");

    let (s, _) = call(&b, Method::GET, "/v1/plan/dag_nope", Some(&token), None).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn rate_limit_is_per_identity_and_audited_once() {
    let b = start(3).await;
    let a = chief(&b, &[Scope::PlanRead]);
    let other = b
        .store
        .add("bot:raven", &[Scope::PlanRead], None)
        .unwrap()
        .token;
    for _ in 0..3 {
        let (s, _) = call(&b, Method::GET, "/v1/whoami", Some(&a), None).await;
        assert_eq!(s, 200);
    }
    for _ in 0..3 {
        let (s, body) = call(&b, Method::GET, "/v1/whoami", Some(&a), None).await;
        assert_eq!(s, 429, "{body}");
        assert!(body["retry_after_secs"].as_u64().unwrap() >= 1);
    }
    // A different identity has its own budget.
    let (s, _) = call(&b, Method::GET, "/v1/whoami", Some(&other), None).await;
    assert_eq!(s, 200);
    let limited = events(&b)
        .await
        .into_iter()
        .filter(|e| e.r#type == "BridgeRequest" && e.payload["status"] == 429)
        .count();
    assert_eq!(limited, 1, "only the first refusal per window is audited");
}

#[tokio::test]
async fn mail_send_and_read_are_attributed_to_the_caller() {
    let b = start(60).await;
    let token = chief(&b, &[Scope::MailSend, Scope::MailRead]);
    let (s, sent) = call(
        &b,
        Method::POST,
        "/v1/mail/send",
        Some(&token),
        Some(json!({"thread_id": "mail:chief", "body": "hello mac", "subject": "hi"})),
    )
    .await;
    assert_eq!(s, 200, "{sent}");
    let (s, bad) = call(
        &b,
        Method::POST,
        "/v1/mail/send",
        Some(&token),
        Some(json!({"thread_id": "general", "body": "x"})),
    )
    .await;
    assert_eq!(s, 400, "{bad}");
    let (s, inbox) = call(
        &b,
        Method::GET,
        "/v1/mail/inbox?thread_id=mail:chief",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, 200);
    let msgs = inbox["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["body"], "hello mac");
    assert_eq!(msgs[0]["from_agent"], "bot:chief");
    assert_eq!(msgs[0]["actor"], "bot:chief");
    // Bridge audit events never masquerade as mail.
    assert!(events(&b)
        .await
        .iter()
        .filter(|e| e.r#type.starts_with("Bridge"))
        .all(|e| e.payload.get("thread_id").is_none()));
}

fn python3() -> Option<PathBuf> {
    for p in [
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
    ] {
        if Path::new(p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    None
}

fn client_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/commrails-bridge-client/commrails-bridge")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_mirror_writes_a_read_only_snapshot() {
    let Some(py) = python3() else {
        eprintln!("python3 not found; skipping client test");
        return;
    };
    let b = start(600).await;
    let token = chief(&b, &GRANTABLE_SCOPES);
    let token_file = b.root.parent().unwrap().join("chief.token");
    std::fs::write(&token_file, &token).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let run = |args: &[&str]| {
        let out = Command::new(&py)
            .arg(client_script())
            .args(["--url", &b.url, "--token-file"])
            .arg(&token_file)
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    // Chief creates a plan from the box...
    let (code, out, err) = tokio::task::block_in_place(|| run(&["plan-new", "mirror me"]));
    assert_eq!(code, 0, "{err}");
    let created: Value = serde_json::from_str(&out).unwrap();
    let dag_id = created["dag_id"].as_str().unwrap().to_string();

    // ...the Mac side adds a node, picks it up, and closes it with an output.
    let gate = local_gate(&b.root, b.ledger.clone()).await;
    gate.plan_refine(
        &dag_id,
        "add work",
        "user",
        vec![DagMutation::CreateNode {
            node_id: "mirror_n1".into(),
            node_kind: "task".into(),
            title: "Do it".into(),
            parent_node_id: Some(created["node_id"].as_str().unwrap().to_string()),
            execution_mode: "shared".into(),
            description: None,
            executor: None,
        }],
    )
    .await
    .unwrap();
    let wih = gate
        .wih_pickup(&dag_id, "mirror_n1", "agent-x")
        .await
        .unwrap();
    gate.wih_close_with(
        &wih,
        "DONE",
        &["ok".to_string()],
        Some("# result\nshipped\n"),
    )
    .await
    .unwrap();

    // Other client commands work against the loopback server.
    let (code, out, err) = tokio::task::block_in_place(|| run(&["plan-show", &dag_id]));
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("mirror_n1") && out.contains("[output]"),
        "{out}"
    );
    let (code, _, err) = tokio::task::block_in_place(|| {
        run(&["mail-send", &format!("dag:{dag_id}"), "--body", "mirrored"])
    });
    assert_eq!(code, 0, "{err}");
    let (code, out, _) =
        tokio::task::block_in_place(|| run(&["mail-read", "--thread", &format!("dag:{dag_id}")]));
    assert_eq!(code, 0);
    assert!(out.contains("mirrored"));

    // Mirror twice (refresh must replace a read-only snapshot).
    let runs = b.root.parent().unwrap().join("runs");
    for _ in 0..2 {
        let (code, _, err) =
            tokio::task::block_in_place(|| run(&["mirror", &dag_id, runs.to_str().unwrap()]));
        assert_eq!(code, 0, "{err}");
    }
    let dir = runs.join(&dag_id);
    let plan: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("plan.json")).unwrap()).unwrap();
    assert_eq!(plan["mirror"], true);
    assert_eq!(plan["authoritative"], false);
    assert_eq!(plan["actor"], "bot:chief");
    assert!(plan["dag"]["nodes"]["mirror_n1"].is_object());
    assert_eq!(
        std::fs::read_to_string(dir.join("nodes/mirror_n1.out.md")).unwrap(),
        "# result\nshipped\n"
    );
    let readme = std::fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(readme.contains("MIRROR") && readme.contains("NOT TRUTH"));
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["README.md", "nodes", "plan.json"]);
    let leftovers = std::fs::read_dir(&runs).unwrap().count();
    assert_eq!(leftovers, 1, "no temp dirs left behind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("plan.json")), 0o444);
        assert_eq!(mode(&dir.join("nodes/mirror_n1.out.md")), 0o444);
        assert_eq!(mode(&dir), 0o555);
        // Let TempDir clean up.
        for p in [dir.join("nodes"), dir.clone()] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    // A revoked box token stops the client with the auth exit code.
    b.store.revoke(None, Some("bot:chief")).unwrap();
    let (code, _, err) = tokio::task::block_in_place(|| run(&["whoami"]));
    assert_eq!(code, 3, "{err}");
}
