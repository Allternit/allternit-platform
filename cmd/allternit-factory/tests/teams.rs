//! Teams, bots and workspace verbs end to end over the built binary:
//! `agents up --dry-run` plans, team.yaml validation, whoami, pack/install,
//! `workflows run --team`, the board, node pages, proof add, and the
//! allternit-api configuration checks (no API here: those paths must say so).

use std::path::Path;
use std::process::{Command, Output};

use allternit_factory_engine::core::types::LedgerQuery;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::work::project_dag;
use allternit_factory_engine::Ledger;
use serde_json::Value;

const TEAM: &str = r#"
name: product-build
bots:
  - { bot: al,       role: coordinator, binding: hosted }
  - { bot: builder,  role: build,       binding: terminal, harness: claude, harnesses: [claude, codex] }
  - { bot: checker,  role: check,       binding: terminal, harness: codex }
  - { bot: research, role: research,    binding: vendor,   vendor: chatgpt, lane: official, directed_by: al }
reach:
  al: [telegram]
presets:
  cheap:
    bots:
      builder: { harness: codex }
"#;

fn factory_env(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_allternit-factory"));
    cmd.args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".state"))
        .env("XDG_RUNTIME_DIR", home.join(".run"));
    for k in [
        "HERDR_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_ENV",
        "ALLTERNIT_FACTORY_ROOT",
        "ALLTERNIT_FACTORY_BIN",
        "ALLTERNIT_FACTORY_BOT",
        "ALLTERNIT_FACTORY_TEAM",
        "ALLTERNIT_FACTORY_BOT_ID",
        "ALLTERNIT_FACTORY_WIH",
        "ALLTERNIT_FACTORY_DAG",
        "ALLTERNIT_FACTORY_PANE_ID",
        "ALLTERNIT_API_URL",
        "ALLTERNIT_API_TOKEN",
        "ALLTERNIT_DESKTOP_ACCESS_TOKEN",
        "ALLTERNIT_USER_ID",
    ] {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run allternit-factory")
}

fn factory(home: &Path, args: &[&str]) -> Output {
    factory_env(home, args, &[])
}

fn one_json(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!("stdout is not one JSON document ({e}): {text:?}\nstderr: {}", String::from_utf8_lossy(&out.stderr))
    })
}

fn error_of(out: &Output) -> (String, String) {
    let doc = one_json(out);
    assert!(doc["error"]["action"].is_string(), "error.action missing: {doc}");
    (doc["error"]["code"].as_str().unwrap().to_string(), doc["error"]["fact"].as_str().unwrap().to_string())
}

fn workspace_with_team(text: &str) -> tempfile::TempDir {
    let ws = tempfile::tempdir().unwrap();
    let dir = ws.path().join(".allternit/teams/product-build");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("team.yaml"), text).unwrap();
    std::fs::write(dir.join("CULTURE.md"), "# Rules\nShow proof.\n").unwrap();
    ws
}

fn listing(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            out.push(p.strip_prefix(base).unwrap().display().to_string());
            if p.is_dir() {
                walk(base, &p, out);
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn up_dry_run_prints_the_same_plan_twice_and_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace_with_team(TEAM);
    let root = ws.path().to_str().unwrap();
    let before = listing(ws.path());
    let args = ["agents", "up", "product-build", "--dry-run", "--json", "--root", root];
    let a = factory(home.path(), &args);
    assert_eq!(a.status.code(), Some(0), "{}", String::from_utf8_lossy(&a.stderr));
    let b = factory(home.path(), &args);
    assert_eq!(a.stdout, b.stdout, "the dry run is deterministic");
    let doc = one_json(&a);
    assert_eq!(doc["applied"], false);
    let steps: Vec<(&str, &str)> = doc["plan"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["action"].as_str().unwrap(), s["agent"].as_str().unwrap()))
        .collect();
    assert_eq!(
        steps,
        [
            ("bind", "al@product-build"),
            ("spawn", "builder@product-build"),
            ("spawn", "checker@product-build"),
            ("bind", "research@product-build"),
        ]
    );
    assert_eq!(doc["plan"][1]["harness"], "claude");
    assert_eq!(listing(ws.path()), before, "a dry run writes nothing");

    // --on shows the machine in the plan.
    let out = factory(home.path(), &["agents", "up", "product-build", "--on", "mac-mini", "--dry-run", "--json", "--root", root]);
    assert_eq!(one_json(&out)["plan"][1]["machine"], "mac-mini");

    // A preset that puts two bots on one instruction file in one workdir is refused, dry run included.
    let out = factory(home.path(), &["agents", "up", "product-build", "--preset", "cheap", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(1));
    let (code, fact) = error_of(&out);
    assert_eq!(code, "refused");
    assert!(fact.contains("AGENTS.md"), "{fact}");

    // Unknown team / preset.
    let out = factory(home.path(), &["agents", "up", "nope", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_of(&out).0, "not_found");
    let out = factory(home.path(), &["agents", "up", "product-build", "--preset", "zzz", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(64));
}

#[test]
fn invalid_team_yaml_reports_every_problem() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace_with_team(
        "bots:\n  - { bot: a, role: r, binding: cloud }\n  - { bot: b, role: r, binding: terminal }\nreach:\n  a: [fax]\n",
    );
    let root = ws.path().to_str().unwrap();
    let out = factory(home.path(), &["agents", "up", "product-build", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(64));
    let (code, fact) = error_of(&out);
    assert_eq!(code, "usage");
    for path in ["bots[0].binding", "bots[1].harness", "reach.a[0]"] {
        assert!(fact.contains(path), "{path} missing from {fact}");
    }
}

#[test]
fn whoami_and_my_nodes_outside_a_pane_are_not_found() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().to_str().unwrap();
    for args in [vec!["agents", "whoami", "--json"], vec!["workspace", "node", "list", "--mine", "--json"]] {
        let mut a = args.clone();
        a.extend(["--root", root]);
        let out = factory(home.path(), &a);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let (code, fact) = error_of(&out);
        assert_eq!(code, "not_found");
        assert!(fact.contains("not inside a factory pane"), "{fact}");
    }
    // Inside a pane: identity from the env, no owned nodes in an empty workspace.
    let out = factory_env(
        home.path(),
        &["agents", "whoami", "--json", "--root", root],
        &[("ALLTERNIT_FACTORY_BOT", "builder"), ("ALLTERNIT_FACTORY_TEAM", "product-build"), ("ALLTERNIT_FACTORY_PANE_ID", "builder-product-build")],
    );
    assert_eq!(out.status.code(), Some(0));
    let doc = one_json(&out);
    assert_eq!(doc["address"], "builder@product-build");
    assert_eq!(doc["paneId"], "builder-product-build");
    assert_eq!(doc["ownedNodes"], serde_json::json!([]));
    assert!(!ws.path().join(".allternit").exists(), "whoami reads, never creates");
}

#[test]
fn pack_then_install_round_trip() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace_with_team(TEAM);
    let root = ws.path().to_str().unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let archive = out_dir.path().join("pb.tar.gz");
    let out = factory(home.path(), &["agents", "pack", "product-build", "--out", archive.to_str().unwrap(), "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let packed = one_json(&out);
    assert_eq!(packed["manifest"]["name"], "product-build");
    assert_eq!(packed["archiveSha256"].as_str().unwrap().len(), 64);
    let files: Vec<&str> = packed["manifest"]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(files, ["CULTURE.md", "team.yaml"]);

    let other = tempfile::tempdir().unwrap();
    let oroot = other.path().to_str().unwrap();
    let out = factory(home.path(), &["agents", "install", archive.to_str().unwrap(), "--dry-run", "--json", "--root", oroot]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let report = one_json(&out);
    assert_eq!(report["team"], "product-build");
    assert_eq!(report["installed"], false);
    assert_eq!(report["files"], packed["manifest"]["files"]);
    assert!(!other.path().join(".allternit").exists(), "dry run installs nothing");

    let out = factory(home.path(), &["agents", "install", archive.to_str().unwrap(), "--json", "--root", oroot]);
    assert_eq!(out.status.code(), Some(0));
    assert!(other.path().join(".allternit/teams/product-build/team.yaml").is_file());
    // Again without --force: refused, with the way out.
    let out = factory(home.path(), &["agents", "install", archive.to_str().unwrap(), "--json", "--root", oroot]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(error_of(&out).0, "refused");
}

fn ledger_dag(root: &Path, dag_id: &str) -> allternit_factory_engine::work::DagState {
    let ledger = Ledger::new(LedgerOptions {
        root_dir: Some(root.to_path_buf()),
        ledger_dir: Some(std::path::PathBuf::from(".allternit/ledger")),
    });
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let events = rt.block_on(ledger.query(LedgerQuery::default())).unwrap();
    let evs: Vec<_> = events.into_iter().filter(|e| e.payload.get("dag_id").and_then(Value::as_str) == Some(dag_id)).collect();
    project_dag(&evs, dag_id)
}

#[test]
fn run_with_a_team_then_board_node_page_proof_and_my_nodes() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace_with_team(TEAM);
    let root = ws.path().to_str().unwrap();

    // Role steps without --team: usage, naming the roles.
    let out = factory(home.path(), &["workflows", "run", "build-check-prove", "--param", "intent=x", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(64));
    assert!(error_of(&out).1.contains("build, check"));

    // Dry run: the would-be nodes, nothing written.
    let out = factory(home.path(), &["workflows", "run", "build-check-prove", "--team", "product-build", "--param", "intent=x", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let dry = one_json(&out);
    assert_eq!(dry["nodes"][0]["executor"], "bot:builder");
    assert_eq!(dry["nodes"][1]["executor"], "bot:checker");
    assert!(!ws.path().join(".allternit/ledger").exists());

    let out = factory(home.path(), &["workflows", "run", "build-check-prove", "--team", "product-build", "--param", "intent=a login page", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let run = one_json(&out);
    assert!(run["campaignId"].is_null());
    let dag_id = run["dagId"].as_str().unwrap().to_string();
    let node = |step: &str| {
        run["nodes"].as_array().unwrap().iter().find(|n| n["stepId"] == step).unwrap()["nodeId"].as_str().unwrap().to_string()
    };
    let (build, check) = (node("build"), node("check"));
    let dag = ledger_dag(ws.path(), &dag_id);
    assert_eq!(dag.nodes[&build].executor.as_deref(), Some("bot:builder"));
    assert_eq!(dag.nodes[&check].executor.as_deref(), Some("bot:checker"));

    // Node folders with the exact headings.
    let folder = ws.path().join(format!(".allternit/work/dags/{dag_id}/nodes/{build}"));
    let spec = std::fs::read_to_string(folder.join("SPEC.md")).unwrap();
    for h in ["# Build it", "## Intent", "## Mini-requirements", "## Proof contract"] {
        assert!(spec.lines().any(|l| l == h), "SPEC.md lacks {h:?}:\n{spec}");
    }
    assert!(spec.contains("a login page"));
    assert!(folder.join("PROGRESS.md").is_file() && folder.join("PROOF.md").is_file() && folder.join("proof").is_dir());

    // Board.
    let out = factory(home.path(), &["workspace", "board", &dag_id, "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0));
    let board = one_json(&out);
    assert_eq!(board["campaign"]["id"], dag_id.as_str());
    for k in ["now", "next", "proven", "needsYou"] {
        assert!(!board["summary"][k].is_null(), "summary.{k} missing: {board}");
    }
    assert_eq!(board["waves"][0]["depth"], 0);
    assert_eq!(board["waves"][0]["nodes"][0]["nodeId"], build.as_str());
    let out = factory(home.path(), &["workspace", "board", "nope", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(2));

    // Proof: a contract line, then proof add (dry run first) writes the file and a receipt.
    std::fs::write(folder.join("SPEC.md"), spec.replace("## Proof contract\n- (none yet)", "## Proof contract\n- [ ] Tests pass")).unwrap();
    let evidence = ws.path().join("tests.txt");
    std::fs::write(&evidence, "12 passed\n").unwrap();
    let ev = evidence.to_str().unwrap();
    let out = factory(home.path(), &["workspace", "proof", "add", &build, "Tests pass", ev, "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(one_json(&out)["changed"], false);
    assert!(std::fs::read_dir(folder.join("proof")).unwrap().next().is_none());
    let out = factory(home.path(), &["workspace", "proof", "add", &build, "Tests pass", ev, "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let added = one_json(&out);
    let path = added["path"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(ws.path().join(path)).unwrap(), "12 passed\n");
    assert!(added["receiptId"].as_str().unwrap().starts_with("rcpt_"));
    assert!(added["sha256"].as_str().unwrap().starts_with("sha256:"));
    let out = factory(home.path(), &["workspace", "proof", "add", &build, "Not a line", ev, "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(2));

    // Node page and proof show carry the evidence.
    let out = factory(home.path(), &["workspace", "node", "show", &dag_id, &build, "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0));
    let page = one_json(&out);
    assert_eq!(page["spec"]["proofContract"][0]["evidence"][0]["receiptId"], added["receiptId"]);
    assert_eq!(page["deliveries"], serde_json::json!([]));
    let out = factory(home.path(), &["workspace", "proof", "show", &build, "--json", "--root", root]);
    assert_eq!(one_json(&out)["files"].as_array().unwrap().len(), 1);

    // My nodes, inside builder's pane: the node whose executor is bot:builder.
    let out = factory_env(
        home.path(),
        &["workspace", "node", "list", "--mine", "--json", "--root", root],
        &[("ALLTERNIT_FACTORY_BOT", "builder"), ("ALLTERNIT_FACTORY_TEAM", "product-build")],
    );
    assert_eq!(out.status.code(), Some(0));
    let mine: Vec<String> = one_json(&out)["nodes"].as_array().unwrap().iter().map(|n| n["nodeId"].as_str().unwrap().to_string()).collect();
    assert_eq!(mine, [build.clone()]);

    // drive --team with a vendor bot and no allternit-api: refused before anything runs.
    let out = factory(home.path(), &["workflows", "drive", &dag_id, "--team", "product-build", "--once", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(3));
    let (code, fact) = error_of(&out);
    assert_eq!(code, "transport");
    assert!(fact.contains("research") && fact.contains("ALLTERNIT_API_URL"), "{fact}");
}

#[test]
fn bot_add_needs_the_api_and_validates_first() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["agents", "bot", "add", "scout", "--binding", "terminal", "--harness", "claude", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let doc = one_json(&out);
    assert_eq!(doc["error"]["code"], "transport");
    assert!(doc["error"]["action"].as_str().unwrap().contains("ALLTERNIT_API_URL"));

    let out = factory(home.path(), &["agents", "bot", "add", "scout", "--binding", "vendor", "--json"]);
    assert_eq!(out.status.code(), Some(64));
    let fact = error_of(&out).1;
    assert!(fact.contains("--vendor") && fact.contains("--directed-by"), "{fact}");

    let out = factory(home.path(), &["agents", "bot", "add", "scout", "--binding", "terminal", "--harness", "claude", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let doc = one_json(&out);
    assert_eq!(doc["wouldPost"]["path"], "/api/v1/factory/bots");
    assert_eq!(doc["wouldPost"]["body"]["binding"]["type"], "terminal");
}

#[test]
fn snapshot_and_restore_dry_run() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace_with_team(TEAM);
    let root = ws.path().to_str().unwrap();
    let out = factory(home.path(), &["agents", "restore", "product-build", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(2), "no snapshot yet");
    let out = factory(home.path(), &["agents", "snapshot", "product-build", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let path = one_json(&out)["path"].as_str().unwrap().to_string();
    assert!(Path::new(&path).is_file());
    let out = factory(home.path(), &["agents", "restore", "product-build", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0));
    let doc = one_json(&out);
    assert_eq!(doc["teamChanged"], false);
    assert_eq!(doc["applied"], false);
    assert_eq!(doc["plan"][1]["action"], "spawn");
}
