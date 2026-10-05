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
        .env_remove("ALLTERNIT_COMMRAILS_ROOT")
        .output()
        .expect("run allternit-factory")
}

/// stdout must be exactly one JSON document.
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
    for old in ["commrails", "rails", " ao "] {
        assert!(!text.to_lowercase().contains(old), "old name {old:?} in --help: {text}");
    }
}

#[test]
fn not_built_verbs_say_so_with_exit_2() {
    let home = tempfile::tempdir().unwrap();
    for (args, fact) in [
        (vec!["agents", "up", "--json"], "agents up is not built yet"),
        (vec!["agents", "whoami", "--json"], "agents whoami is not built yet"),
        (vec!["workspace", "board", "--json"], "workspace board is not built yet"),
        (vec!["workspace", "proof", "add", "x", "--json"], "workspace proof is not built yet"),
        (vec!["orchestration", "threads", "list", "--json"], "orchestration threads is not built yet"),
        (vec!["orchestration", "coordinate", "p", "hi", "--json"], "orchestration coordinate is not built yet"),
    ] {
        let out = factory(home.path(), &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(error_of(&out), ("not_found".to_string(), fact.to_string()), "{args:?}");
    }

    // Without --json: nothing on stdout, the fact on stderr, same exit code.
    let out = factory(home.path(), &["agents", "up"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("agents up is not built yet"));
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
    let out = factory(
        home.path(),
        &["orchestration", "send", "worker", "hello", "there", "--dry-run", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = one_json(&out);
    assert_eq!(doc["dryRun"], true);
    assert_eq!(doc["changed"], false);
    let argv: Vec<&str> = doc["wouldRun"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(argv, ["allternit-factory", "pane", "send", "worker", "hello", "there"]);

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
        ["allternit-factory", "internal", "rails", "--root", root, "wait-gate", "resolve", "--node", "d/n", "g1", "--outcome", "ok", "--actor", "user:eoj"]
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
    assert_eq!(one_json(&out), serde_json::json!({ "templates": [] }));
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
fn internal_rails_keeps_its_own_help() {
    let home = tempfile::tempdir().unwrap();
    let out = factory(home.path(), &["internal", "rails", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for group in ["ledger", "index", "lease", "vault", "replay", "hook"] {
        assert!(text.contains(group), "internal rails --help missing {group}: {text}");
    }
}
