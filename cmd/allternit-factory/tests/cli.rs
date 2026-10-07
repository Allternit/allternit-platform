//! The `allternit-factory` CLI contract (API.md §2): exit-code table, the
//! `--json` document and error shape, honest "not built yet" verbs, and
//! `--dry-run` that changes nothing.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn factory(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_allternit-factory"))
        .args(args)
        // Keep every engine/pane state lookup inside the temp dir.
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".state"))
        .env("XDG_RUNTIME_DIR", home.join(".run"))
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .env_remove("HERDR_ENV")
        .env_remove("ALLTERNIT_FACTORY_ROOT")
        .output()
        .expect("run allternit-factory")
}

/// stdout must be exactly one JSON document.
/// A send dry run resolves the target and lists the records it would write,
/// and writes none of them.
#[test]
fn send_dry_run_plans_the_delivery_and_writes_nothing() {
    // Short: the pane engine's socket lives under HOME (macOS caps the path).
    let home = tempfile::Builder::new().prefix("f3cli").tempdir_in("/tmp").unwrap();
    let ws = tempfile::Builder::new().prefix("f3ws").tempdir_in("/tmp").unwrap();
    let reg = home.path().join(".allternit/factory");
    std::fs::create_dir_all(&reg).unwrap();
    std::fs::write(reg.join("registry.json"), r#"{"sessions":{"ao-worker":{"cwd":"/w","lifecycle":"running"}}}"#).unwrap();
    let root = ws.path().to_str().unwrap();
    let out = factory(home.path(), &["orchestration", "send", "worker", "hello", "there", "--dry-run", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = one_json(&out);
    assert_eq!(doc["dryRun"], true);
    assert_eq!(doc["plan"]["to"], "local:worker");
    // No pane engine is running in this HOME, so the pane is gone: queued.
    assert_eq!(doc["plan"]["via"], "pane_queue");
    assert_eq!(doc["plan"]["records"], serde_json::json!(["MessageSent", "BusMessageSent", "factory.delivery"]));
    assert!(!ws.path().join(".allternit/ledger").exists(), "a dry run wrote the ledger");
}

fn one_json(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {text:?}"))
}

fn error_of(out: &Output) -> (String, String) {
    let doc = one_json(out);
    let err = &doc["error"];
    assert!(err["action"].is_string(), "error.action missing: {doc}");
    (
        err["code"].as_str().expect("error.code").to_string(),
        err["fact"].as_str().expect("error.fact").to_string(),
    )
}

#[test]
fn help_lists_the_four_parts_and_hides_internal() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for part in ["serve", "pane", "agents", "orchestration", "workflows", "workspace"] {
        assert!(text.contains(part), "--help is missing {part}: {text}");
    }
    assert!(
        !text.lines().any(|l| l.trim_start().starts_with("internal")),
        "the internal group must stay hidden: {text}"
    );
    for old in ["commrails", "rails", " ao "] { // old-names: keep (asserts the old names are gone)
        assert!(!text.to_lowercase().contains(old), "old name {old:?} in --help: {text}");
    }
}

#[test]
fn not_built_verbs_say_so_with_exit_2() {
    let home = tempfile::tempdir().unwrap();
    for (args, fact) in [
        (vec!["agents", "model", "builder", "opus", "--json"], "agents model is not built yet"),
        (vec!["agents", "handoff", "builder", "--json"], "agents handoff is not built yet"),
        (vec!["agents", "templates", "--json"], "agents templates is not built yet"),
    ] {
        let out = factory(home.path(), &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(error_of(&out), ("not_found".to_string(), fact.to_string()), "{args:?}");
    }

    // Without --json: nothing on stdout, the fact on stderr, same exit code.
    let out = factory(home.path(), &["agents", "model", "builder", "opus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("agents model is not built yet"));
}

#[test]
fn account_verbs_point_to_gizzi() {
    // Tasks, threads and the Coordinator live with the account in
    // allternit-api, so the engine sends you to Gizzi (usage, exit 64).
    let home = tempfile::tempdir().unwrap();
    for (args, action) in [
        (vec!["workspace", "tasks", "--json"], "gizzi workspace tasks board"),
        (vec!["orchestration", "threads", "list", "--json"], "gizzi orchestration threads"),
        (vec!["orchestration", "coordinate", "p", "hi", "--json"], "gizzi orchestration coordinate"),
    ] {
        let out = factory(home.path(), &args);
        assert_eq!(out.status.code(), Some(64), "{args:?}");
        let (code, fact) = error_of(&out);
        assert_eq!(code, "usage", "{args:?}");
        assert!(fact.contains("lives with your account"), "{fact}");
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(doc["error"]["action"].as_str().unwrap().contains(action), "{args:?}: {doc}");
    }
}

#[test]
fn usage_errors_exit_64_with_the_usage_code() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["agents", "no-such-verb", "--json"]);
    assert_eq!(out.status.code(), Some(64));
    assert_eq!(error_of(&out).0, "usage");

    let out = factory(home.path(), &["workspace", "approve", "dag/node", "--json"]);
    assert_eq!(out.status.code(), Some(64));
    assert_eq!(error_of(&out).0, "usage");

    let out = factory(home.path(), &["agents", "recover", "--apply", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(64));
}

#[test]
fn dry_run_prints_the_command_and_runs_nothing() {
    let home = tempfile::tempdir().unwrap();
    // An agent nobody started is not found, dry run or not.
    let out = factory(home.path(), &["agents", "down", "worker", "--dry-run", "--json"]);
    assert_eq!(error_of(&out).0, "not_found", "{}", String::from_utf8_lossy(&out.stdout));

    // A recorded session: the dry run names it and changes nothing.
    let reg = home.path().join(".allternit/factory/registry.json");
    std::fs::create_dir_all(reg.parent().unwrap()).unwrap();
    let text = r#"{"sessions":{"ao-worker":{"cwd":"/tmp","dead":true,"lifecycle":"dead","lead":"me"}}}"#;
    std::fs::write(&reg, text).unwrap();
    let out = factory(home.path(), &["agents", "down", "worker", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = one_json(&out);
    assert_eq!(doc["dryRun"], true);
    assert_eq!(doc["changed"], false);
    assert_eq!(doc["wouldStop"], "ao-worker");
    let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&reg).unwrap()).unwrap();
    assert_eq!(after["sessions"]["ao-worker"]["lifecycle"], "dead");

    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().to_str().unwrap();
    let out = factory(
        home.path(),
        &["workspace", "approve", "d/n", "g1", "--actor", "user:eoj", "--dry-run", "--json", "--root", root],
    );
    assert_eq!(out.status.code(), Some(0));
    let doc = one_json(&out);
    let argv: Vec<&str> = doc["wouldRun"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(
        argv,
        ["allternit-factory", "internal", "core", "--root", root, "wait-gate", "resolve", "--node", "d/n", "g1", "--outcome", "ok", "--actor", "user:eoj"]
    );
    // Nothing was written into the workspace.
    assert!(!ws.path().join(".allternit").exists());

    // Passthrough groups accept --dry-run anywhere.
    let out = factory(home.path(), &["orchestration", "mail", "send", "x", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(one_json(&out)["dryRun"], true);
}

#[test]
fn node_list_on_an_empty_workspace_is_one_json_document() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().to_str().unwrap();
    let out = factory(home.path(), &["workspace", "node", "list", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(one_json(&out), serde_json::json!({ "nodes": [] }));
}

#[test]
fn template_list_reads_without_creating_anything() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().to_str().unwrap();
    let out = factory(home.path(), &["workflows", "template", "list", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0));
    // A fresh workspace sees the built-in templates, and reading creates nothing.
    let doc = one_json(&out);
    let ids: Vec<&str> = doc["templates"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["build-check-prove", "fact-check"]);
    assert_eq!(doc["templates"][0]["builtin"], true);
    assert_eq!(doc["templates"][0]["stepCount"], 3);
    assert!(!ws.path().join(".allternit").exists());

    let out = factory(home.path(), &["workflows", "template", "show", "build-check-prove", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(0));
    let t = one_json(&out);
    assert_eq!(t["id"], "build-check-prove");
    assert_eq!(t["steps"][1]["onFail"], "build");
    assert_eq!(t["maxRounds"], 3);
    assert!(!ws.path().join(".allternit").exists());

    let out = factory(home.path(), &["workflows", "template", "show", "nope", "--json", "--root", root]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_of(&out).0, "not_found");
}

#[test]
fn transcript_of_an_unknown_agent_is_not_found() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["orchestration", "transcript", "nobody", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let (code, fact) = error_of(&out);
    assert_eq!(code, "not_found");
    assert!(fact.contains("ao-nobody"), "{fact}");
}

#[test]
fn internal_core_keeps_its_own_help() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["internal", "core", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for group in ["ledger", "index", "lease", "vault", "replay", "hook"] {
        assert!(text.contains(group), "internal core --help missing {group}: {text}");
    }
}

#[test]
fn agents_ps_without_a_pane_server_is_one_document_with_agents() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["agents", "ps", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = one_json(&out);
    assert_eq!(doc["agents"], serde_json::json!([]));
    // The engine being down is reported in the document, not hidden.
    assert!(doc["engine"]["error"].is_string(), "{doc}");
}

#[test]
fn version_is_one_line() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.starts_with("allternit-factory "), "{text:?}");
}

#[test]
fn template_save_checks_copies_and_refuses_to_overwrite() {
    let home = tempfile::tempdir().unwrap();
    let ws = home.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let src = home.path().join("my-flow.md");
    std::fs::write(&src, include_str!("../../../factory/engine/templates/fact-check.md")).unwrap();
    let root = ws.to_str().unwrap();
    let file = src.to_str().unwrap();
    let saved = ws.join(".allternit/rails/templates/my-flow.md");

    // --dry-run writes nothing and says what it would do.
    let out = factory(home.path(), &["--root", root, "workflows", "template", "save", file, "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = one_json(&out);
    assert_eq!(doc["dryRun"], true);
    assert_eq!(doc["plan"]["id"], "my-flow");
    assert!(!saved.exists());

    // Save, then it shows up in the list.
    let out = factory(home.path(), &["--root", root, "workflows", "template", "save", file, "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(saved.is_file());
    let out = factory(home.path(), &["--root", root, "workflows", "template", "list", "--json"]);
    assert!(one_json(&out)["templates"].as_array().unwrap().iter().any(|t| t["id"] == "my-flow"));

    // A second save is refused (exit 1) unless --force.
    let out = factory(home.path(), &["--root", root, "workflows", "template", "save", file, "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(error_of(&out).0, "refused");
    let out = factory(home.path(), &["--root", root, "workflows", "template", "save", file, "--force", "--json"]);
    assert_eq!(out.status.code(), Some(0));

    // An invalid file is a usage error and saves nothing.
    let bad = home.path().join("bad.md");
    std::fs::write(&bad, "not a template").unwrap();
    let out = factory(home.path(), &["--root", root, "workflows", "template", "save", bad.to_str().unwrap(), "--json"]);
    assert_ne!(out.status.code(), Some(0));
    assert!(!ws.join(".allternit/rails/templates/bad.md").exists());
}
