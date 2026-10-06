//! `/api/factory` contract tests against a real `allternit-factory serve` on a
//! fresh workspace and a throwaway HOME (no pane engine running there, so the
//! registry's sessions are dead panes). Response shapes are checked against
//! the type blocks in `docs/specs/allternit-factory/API.md`, loaded at test
//! time, so the contract file and the engine can't drift apart silently.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_allternit-factory")
}

// ---------------------------------------------------------------- API.md shapes

fn api_md() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/allternit-factory/API.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The top-level keys of `Name { … }` in API.md: (key, optional).
fn shape(md: &str, name: &str) -> Vec<(String, bool)> {
    let start = md
        .match_indices(&format!("{name} {{"))
        .map(|(i, _)| i)
        .find(|&i| i == 0 || md[..i].ends_with('\n') || md[..i].ends_with(' ') && md[..i].trim_end().ends_with('`'))
        .or_else(|| md.find(&format!("\n{name} {{")).map(|i| i + 1))
        .unwrap_or_else(|| panic!("API.md has no `{name} {{` block"));
    let body_start = start + name.len() + 2;
    let mut depth = 1;
    let mut end = body_start;
    for (i, c) in md[body_start..].char_indices() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &md[body_start..end];
    // Strip comments, then split at depth-0 commas and newlines.
    let mut clean = String::new();
    let mut rest = body;
    while let Some(i) = rest.find("/*") {
        clean.push_str(&rest[..i]);
        rest = rest[i..].find("*/").map(|j| &rest[i + j + 2..]).unwrap_or("");
    }
    clean.push_str(rest);
    let clean: String = clean.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let mut keys = Vec::new();
    let (mut depth, mut piece) = (0, String::new());
    for c in clean.chars().chain(std::iter::once(',')) {
        match c {
            '{' | '(' | '[' | '<' => depth += 1,
            '}' | ')' | ']' | '>' => depth -= 1,
            _ => {}
        }
        if depth == 0 && (c == ',' || c == '\n') {
            let p = piece.trim();
            let key: String = p.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !key.is_empty() {
                let optional = p[key.len()..].trim_start().starts_with('?');
                keys.push((key, optional));
            }
            piece.clear();
        } else {
            piece.push(c);
        }
    }
    keys
}

fn assert_shape(md: &str, name: &str, value: &Value) {
    let obj = value.as_object().unwrap_or_else(|| panic!("{name} is not an object: {value}"));
    let keys = shape(md, name);
    let allowed: BTreeSet<&str> = keys.iter().map(|(k, _)| k.as_str()).collect();
    for (k, optional) in &keys {
        assert!(*optional || obj.contains_key(k), "{name} is missing `{k}`: {value}");
    }
    for k in obj.keys() {
        assert!(allowed.contains(k.as_str()), "{name} has `{k}`, which API.md does not: {value}");
    }
}

#[test]
fn api_md_shapes_parse() {
    let md = api_md();
    let d: Vec<String> = shape(&md, "Delivery").into_iter().map(|(k, _)| k).collect();
    assert_eq!(d, ["id", "to", "via", "state", "ticket", "threadId", "messageId", "nodeId", "dagId", "at", "detail"]);
    let a = shape(&md, "Agent");
    assert!(a.iter().any(|(k, _)| k == "fields") && a.iter().any(|(k, _)| k == "binding"));
}

// ---------------------------------------------------------------- engine harness

struct Engine {
    child: Child,
    base: String,
    root: PathBuf,
    home: PathBuf,
    _tmp: TempDir,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A short temp dir: the pane engine's socket lives under HOME, and macOS
/// caps a Unix socket path at 104 bytes (`$TMPDIR` alone is ~50).
fn short_tmp() -> TempDir {
    tempfile::Builder::new().prefix("f3").tempdir_in("/tmp").unwrap()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn factory(home: &Path, root: &Path) -> Command {
    let mut cmd = Command::new(bin());
    cmd.env("HOME", home)
        .env("ALLTERNIT_FACTORY_HOME", home.join("factory"))
        .env("HERDR_SESSION", format!("f3-api-{}", std::process::id()))
        .env_remove("ALLTERNIT_FACTORY_API_URL")
        .env_remove("ALLTERNIT_API_PORT")
        .arg("--root")
        .arg(root);
    cmd
}

const PROMO: &str = r#"---
name: Motion promo
description: capture -> review
---

```yaml template-spec
params:
  - name: topic
steps:
  - id: capture
    title: "Capture {{ params.topic }}"
    executor: "ao:claude"
  - id: review
    title: Review
    blocked_by: [capture]
    wait_gate:
      kind: manual
      description: "Eoj reviews the {{ params.topic }} cut"
```
"#;

fn start() -> Engine {
    let tmp = short_tmp();
    let root = tmp.path().join("ws");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(root.join(".allternit/rails/templates")).unwrap();
    std::fs::create_dir_all(home.join("factory")).unwrap();
    std::fs::write(root.join(".allternit/rails/templates/promo.md"), PROMO).unwrap();
    // A session the registry says is running, whose pane is gone.
    std::fs::write(
        home.join("factory/registry.json"),
        json!({ "sessions": { "ao-ghost": { "cwd": root, "lifecycle": "running", "dead": false, "harness": "claude" } } })
            .to_string(),
    )
    .unwrap();
    let out = factory(&home, &root)
        .args(["workspace", "campaign", "new", "--id", "c1", "--objective", "Saved views", "--owner", "eoj", "--executor", "bot:al", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "campaign new: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let port = free_port();
    let child = factory(&home, &root)
        .args(["serve", "--port", &port.to_string(), "--no-socket"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if reqwest::blocking::get(format!("{base}/api/factory/health")).map(|r| r.status().is_success()).unwrap_or(false) {
            break;
        }
        assert!(Instant::now() < deadline, "engine did not come up on {base}");
        std::thread::sleep(Duration::from_millis(100));
    }
    Engine { child, base, root, home, _tmp: tmp }
}

fn get(e: &Engine, path: &str) -> (u16, Value) {
    let r = reqwest::blocking::get(format!("{}{path}", e.base)).unwrap();
    let status = r.status().as_u16();
    (status, r.json().unwrap_or(Value::Null))
}

fn post(e: &Engine, path: &str, body: Value) -> (u16, Value) {
    let r = reqwest::blocking::Client::new().post(format!("{}{path}", e.base)).json(&body).send().unwrap();
    let status = r.status().as_u16();
    (status, r.json().unwrap_or(Value::Null))
}

fn ledger_types(root: &Path) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(root.join(".allternit/ledger/events"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.sort();
    files
        .iter()
        .flat_map(|f| std::fs::read_to_string(f).unwrap_or_default().lines().map(str::to_string).collect::<Vec<_>>())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .map(|v| v["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn assert_error(status: u16, body: &Value, want_status: u16, code: &str) {
    assert_eq!(status, want_status, "{body}");
    assert_eq!(body["error"]["code"], code, "{body}");
    assert!(body["error"]["fact"].is_string() && body["error"]["action"].is_string(), "{body}");
}

// ---------------------------------------------------------------- the contract

#[test]
fn factory_api_contract() {
    let md = api_md();
    let e = start();

    // Agents: the registry is reconciled, so the dead pane reads offline and
    // the record now says dead (never running).
    let (s, body) = get(&e, "/api/factory/agents");
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["engine"]["running"], false, "{body}");
    let ghost = body["agents"].as_array().unwrap().iter().find(|a| a["slug"] == "ghost").cloned().expect("ghost");
    assert_shape(&md, "Agent", &ghost);
    assert_eq!(ghost["state"], "offline");
    assert_eq!(ghost["id"], "local:ghost");
    assert_eq!(ghost["binding"]["type"], "terminal");
    assert!(ghost["pane"].is_null());
    let reg: Value = serde_json::from_str(&std::fs::read_to_string(e.home.join("factory/registry.json")).unwrap()).unwrap();
    assert_eq!(reg["sessions"]["ao-ghost"]["dead"], true);
    assert_eq!(reg["sessions"]["ao-ghost"]["bot"]["id"], "local:ghost");
    let (s, one) = get(&e, "/api/factory/agents/local:ghost");
    assert_eq!(s, 200);
    assert_eq!(one, ghost);
    let (s, b) = get(&e, "/api/factory/agents/nobody");
    assert_error(s, &b, 404, "not_found");
    let (s, b) = get(&e, "/api/factory/agents/ghost/capture");
    assert_error(s, &b, 404, "not_found");

    // Send: dry run lists exactly the records the real send then writes.
    let before = ledger_types(&e.root).len();
    let (s, plan) = post(&e, "/api/factory/send", json!({ "to": "ghost", "text": "check the build", "dryRun": true }));
    assert_eq!(s, 200, "{plan}");
    assert_eq!(ledger_types(&e.root).len(), before, "dry run wrote to the ledger");
    let (s, d) = post(&e, "/api/factory/send", json!({ "to": "ghost", "text": "check the build", "nodeId": "n1", "dagId": "d1" }));
    assert_eq!(s, 200, "{d}");
    assert_shape(&md, "Delivery", &d);
    // The pane is gone: the text is queued on its mailbox, and says so.
    assert_eq!((d["via"].as_str(), d["state"].as_str()), (Some("pane_queue"), Some("queued")), "{d}");
    assert_eq!(d["to"], "local:ghost");
    assert!(d["messageId"].as_str().unwrap().starts_with("evt_"));
    let written: Vec<String> = ledger_types(&e.root)[before..].iter().filter(|t| *t != "ThreadCreated").cloned().collect();
    let planned: Vec<String> = plan["plan"]["records"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(written, planned, "the dry run and the real send disagree");

    // Idempotent: the same key twice is one delivery.
    let (_, k1) = post(&e, "/api/factory/send", json!({ "to": "ghost", "text": "once", "idempotencyKey": "k-1" }));
    let n = ledger_types(&e.root).len();
    let (_, k2) = post(&e, "/api/factory/send", json!({ "to": "ghost", "text": "once", "idempotencyKey": "k-1" }));
    assert_eq!(k1, k2);
    assert_eq!(ledger_types(&e.root).len(), n, "a repeated key sent again");

    let (s, b) = post(&e, "/api/factory/send", json!({ "to": "nobody", "text": "hi" }));
    assert_error(s, &b, 404, "not_found");
    let (s, b) = post(&e, "/api/factory/send", json!({ "to": "ghost", "text": "  " }));
    assert_error(s, &b, 400, "usage");

    let (s, list) = get(&e, "/api/factory/deliveries?agent=local:ghost");
    assert_eq!(s, 200);
    let list = list["deliveries"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    list.iter().for_each(|d| assert_shape(&md, "Delivery", d));
    let (_, by_node) = get(&e, "/api/factory/deliveries?node=n1");
    assert_eq!(by_node["deliveries"][0]["id"], d["id"]);

    // Templates.
    let (s, t) = get(&e, "/api/factory/templates");
    assert_eq!(s, 200);
    assert_eq!(t["templates"][0]["id"], "promo");
    let (s, t) = get(&e, "/api/factory/templates/promo");
    assert_eq!(s, 200);
    assert_shape(&md, "Template", &t);
    assert_eq!(t["steps"][1]["blockedBy"], json!(["capture"]));
    assert_eq!(t["steps"][1]["waitGate"]["kind"], "manual");

    // Runs: dry run changes nothing; the run plans a DAG in campaign c1.
    let n = ledger_types(&e.root).len();
    let (s, dry) = post(&e, "/api/factory/runs", json!({ "template": "promo", "campaignId": "c1", "params": { "topic": "Views" }, "dryRun": true }));
    assert_eq!(s, 200, "{dry}");
    assert_eq!(ledger_types(&e.root).len(), n);
    let (s, run) = post(&e, "/api/factory/runs", json!({ "template": "promo", "campaignId": "c1", "intent": "Ship saved views", "params": { "topic": "Views" } }));
    assert_eq!(s, 200, "{run}");
    let dag_id = run["dagId"].as_str().unwrap().to_string();
    assert_eq!(run["campaignId"], "c1");
    let nodes = run["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    nodes.iter().for_each(|c| assert_shape(&md, "NodeCard", c));
    let capture = nodes.iter().find(|c| c["title"] == "Capture Views").unwrap();
    let review = nodes.iter().find(|c| c["title"] == "Review").unwrap();
    assert_eq!((capture["status"].as_str(), capture["bindingType"].as_str(), capture["depth"].as_u64()), (Some("ready"), Some("terminal"), Some(0)));
    assert_eq!(review["blockedBy"], json!([capture["nodeId"]]));
    assert_eq!(review["depth"], 1);
    assert_eq!((review["status"].as_str(), review["needsYou"].as_bool()), (Some("needs_you"), Some(true)));
    assert_eq!(review["gate"]["kind"], "manual");
    let (s, b) = post(&e, "/api/factory/runs", json!({ "template": "promo", "params": { "topic": "x" } }));
    assert_error(s, &b, 400, "usage");
    let (s, b) = post(&e, "/api/factory/runs", json!({ "template": "nope", "campaignId": "c1" }));
    assert_error(s, &b, 404, "not_found");

    let (s, flow) = get(&e, &format!("/api/factory/dags/{dag_id}"));
    assert_eq!(s, 200);
    assert_shape(&md, "Flow", &flow);
    assert_eq!(flow["edges"], json!([{ "from": capture["nodeId"], "to": review["nodeId"], "type": "blocked_by" }]));
    let (s, b) = get(&e, "/api/factory/dags/nope");
    assert_error(s, &b, 404, "not_found");

    let (s, c) = get(&e, "/api/factory/campaigns");
    assert_eq!(s, 200);
    let c1 = c["campaigns"].as_array().unwrap().iter().find(|c| c["id"] == "c1").unwrap().clone();
    let keys: BTreeSet<&str> = c1.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["id", "projectId", "title", "intent", "status", "proven", "total", "needsYou"]));
    assert_eq!((c1["total"].as_u64(), c1["needsYou"].as_u64(), c1["proven"].as_u64()), (Some(2), Some(1), Some(0)));

    let (s, mine) = get(&e, "/api/factory/nodes?assignee=al");
    assert_eq!(s, 200);
    assert_eq!(mine["nodes"], json!([]));
    let (_, open) = get(&e, "/api/factory/nodes");
    assert_eq!(open["nodes"].as_array().unwrap().len(), 2);
    let (s, b) = get(&e, "/api/factory/nodes?status=bogus");
    assert_error(s, &b, 400, "usage");
    let (s, page) = get(&e, &format!("/api/factory/nodes/{dag_id}/{}", capture["nodeId"].as_str().unwrap()));
    assert_eq!(s, 200, "{page}");
    assert_shape(&md, "NodePage", &page);
    assert_shape(&md, "NodeCard", &page["card"]);

    // Teams read from .allternit/teams; booting a team that isn't there is not_found.
    let (s, teams) = get(&e, "/api/factory/teams");
    assert_eq!((s, teams.clone()), (200, json!({ "teams": [], "invalid": [] })));
    let (s, b) = post(&e, "/api/factory/teams/build/up", json!({}));
    assert_error(s, &b, 404, "not_found");
    assert!(b["error"]["fact"].as_str().unwrap().contains("build"), "{b}");
    let (s, b) = get(&e, "/api/factory/campaigns/nope/board");
    assert_error(s, &b, 404, "not_found");

    // Events: replay from the start of the ledger includes the deliveries.
    let resp = reqwest::blocking::Client::new()
        .get(format!("{}/api/factory/events", e.base))
        .header("Last-Event-ID", "evt_0")
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap();
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut text = String::new();
    let mut resp = resp;
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(8);
    use std::io::Read;
    while Instant::now() < deadline && !(text.contains("event: delivery") && text.contains("event: node.status")) {
        match resp.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }
    assert!(text.contains("event: delivery"), "{text}");
    assert!(text.contains("event: node.status"), "{text}");
    assert!(text.contains("id: evt_"), "{text}");
    let first = text.lines().find(|l| l.starts_with("data: ") && l.contains("\"type\":\"delivery\"")).unwrap();
    let ev: Value = serde_json::from_str(first.trim_start_matches("data: ")).unwrap();
    assert_shape(&md, "Delivery", &ev["data"]);
    assert!(ev["at"].is_string());
}

/// API.md §2 exit codes and the `--json` error envelope, through the CLI.
#[test]
fn exit_code_table() {
    let tmp = short_tmp();
    let root = tmp.path().join("ws");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(home.join("factory")).unwrap();
    let run = |args: &[&str]| {
        let out = factory(&home, &root).args(args).output().unwrap();
        let body: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out.status.code().unwrap_or(-1), body)
    };
    let (code, body) = run(&["agents", "ps", "--json"]);
    assert_eq!((code, &body["agents"]), (0, &json!([])), "{body}");
    // No pane engine in this HOME: reported, not hidden.
    assert_eq!(body["engine"]["running"], false, "{body}");
    assert!(body["engine"]["error"].is_string(), "{body}");
    let (code, body) = run(&["orchestration", "send", "nobody", "hi", "--json"]);
    assert_eq!(code, 2, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    let (code, body) = run(&["workspace", "board", "nope", "--json"]);
    assert_eq!((code, body["error"]["code"].as_str()), (2, Some("not_found")), "{body}");
    let (code, body) = run(&["workspace", "board", "--json"]);
    assert_eq!((code, body["error"]["code"].as_str()), (64, Some("usage")), "a board needs its campaign");
    let (code, body) = run(&["orchestration", "send", "nobody", "--json"]);
    assert_eq!((code, body["error"]["code"].as_str()), (64, Some("usage")));
    let (code, _) = run(&["no-such-part", "--json"]);
    assert_eq!(code, 64);
}
