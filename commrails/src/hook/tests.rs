use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::gate::GateOptions;
use crate::leases::LeasesOptions;
use crate::ledger::LedgerOptions;
use crate::receipts::{ReceiptStore, ReceiptStoreOptions};

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    ledger: Arc<Ledger>,
    leases: Arc<Leases>,
    gate: Gate,
}

async fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let ledger = Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.clone()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }));
    let leases = Arc::new(
        Leases::new(LeasesOptions {
            root_dir: Some(root.clone()),
            leases_dir: Some(PathBuf::from(".allternit/leases")),
            event_sink: Some(ledger.clone()),
            actor_id: Some("gate".to_string()),
            auto_renewal_enabled: false,
            ..Default::default()
        })
        .await
        .unwrap(),
    );
    let receipts = Arc::new(
        ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.clone()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })
        .unwrap(),
    );
    let gate = Gate::new(GateOptions {
        ledger: ledger.clone(),
        leases: leases.clone(),
        receipts,
        index: None,
        vault: None,
        oauth_vault: None,
        root_dir: Some(root.clone()),
        actor_id: Some("gate".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    });
    Fixture {
        _tmp: tmp,
        root,
        ledger,
        leases,
        gate,
    }
}

/// An open-signed WIH holding a granted lease on `src/**`.
async fn bound_wih(f: &Fixture) -> String {
    let (_, dag_id, node_id) = f.gate.plan_new("spawn gate test", None).await.unwrap();
    let wih_id = f.gate.wih_pickup(&dag_id, &node_id, "agent-1").await.unwrap();
    f.gate.wih_sign_open(&wih_id, "sig").await.unwrap();
    let lease_id = f
        .gate
        .lease_request(&wih_id, "agent-1", vec!["src/**".to_string()], Some(3600))
        .await
        .unwrap();
    let until = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    f.leases.grant(&lease_id, &until).await.unwrap();
    wih_id
}

fn bash(cmd: &str, cwd: &Path) -> HookRequest {
    HookRequest::from_json(&json!({
        "tool_name": "Bash",
        "tool_input": { "command": cmd },
        "cwd": cwd.to_string_lossy(),
        "session_id": "s1",
    }))
    .unwrap()
}

fn write(path: &str, cwd: &Path) -> HookRequest {
    HookRequest::from_json(&json!({
        "tool_name": "Write",
        "tool_input": { "file_path": path, "content": "x" },
        "cwd": cwd.to_string_lossy(),
    }))
    .unwrap()
}

const HOME: &str = "/Users/hook-test";

#[tokio::test]
async fn floor_denies_rm_rf_home_without_wih() {
    let f = fixture().await;
    let d = decide(&bash("rm -rf ~", &f.root), &f.root, Some(Path::new(HOME)), None).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    assert!(d.verdict.reason().contains("hard floor"));
    let out = claude_hook_output(&d.verdict).expect("deny prints hook JSON");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
}

#[tokio::test]
async fn floor_allows_ls_without_wih() {
    let f = fixture().await;
    let d = decide(&bash("ls -la", &f.root), &f.root, Some(Path::new(HOME)), None).await;
    assert!(!d.verdict.is_deny());
    assert!(claude_hook_output(&d.verdict).is_none(), "allow prints nothing");
}

#[tokio::test]
async fn floor_applies_even_with_wih_bound() {
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    let binding = WihBinding {
        wih_id: &wih,
        gate: &f.gate,
        leases: &f.leases,
    };
    let d = decide(&bash("rm -rf ~", &f.root), &f.root, Some(Path::new(HOME)), Some(binding)).await;
    assert!(d.verdict.reason().contains("hard floor"), "{:?}", d.verdict);
}

#[tokio::test]
async fn wih_bound_denies_write_outside_lease_and_allows_inside() {
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    let bind = || WihBinding {
        wih_id: &wih,
        gate: &f.gate,
        leases: &f.leases,
    };
    let home = Some(Path::new(HOME));

    // Outside the root entirely.
    let d = decide(&bash("touch /tmp/allternit-hook-probe-1", &f.root), &f.root, home, Some(bind())).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    assert!(d.verdict.reason().contains("outside"), "{:?}", d.verdict);

    // Inside the root but not in the lease.
    let d = decide(&write("docs/readme.md", &f.root), &f.root, home, Some(bind())).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);

    // Inside the lease: Bash and Write.
    let d = decide(&bash("mkdir -p src/a && touch src/a/b.rs", &f.root), &f.root, home, Some(bind())).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
    assert_eq!(d.paths, vec!["src/a".to_string(), "src/a/b.rs".to_string()]);
    let abs = f.root.join("src/lib.rs");
    let d = decide(&write(&abs.to_string_lossy(), &f.root), &f.root, home, Some(bind())).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);

    // Reads are not lease-gated.
    let d = decide(&bash("cat /etc/hosts | head", &f.root), &f.root, home, Some(bind())).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);

    // Unresolvable targets fail closed.
    let d = decide(&bash("echo x > $OUT", &f.root), &f.root, home, Some(bind())).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
}

#[tokio::test]
async fn wih_bound_denies_lease_held_by_another_wih() {
    let f = fixture().await;
    let mine = bound_wih(&f).await;
    // A second WIH holds a lease on docs/**.
    let other = bound_wih(&f).await;
    let lease_id = f
        .gate
        .lease_request(&other, "agent-2", vec!["docs/**".to_string()], Some(3600))
        .await
        .unwrap();
    f.leases
        .grant(&lease_id, &(Utc::now() + chrono::Duration::hours(1)).to_rfc3339())
        .await
        .unwrap();
    let d = decide(
        &write("docs/x.md", &f.root),
        &f.root,
        None,
        Some(WihBinding {
            wih_id: &mine,
            gate: &f.gate,
            leases: &f.leases,
        }),
    )
    .await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    assert!(d.verdict.reason().contains("not covered by a lease this WIH holds"));
}

#[tokio::test]
async fn unsigned_or_unknown_wih_is_denied() {
    let f = fixture().await;
    let d = decide(
        &write("src/x.rs", &f.root),
        &f.root,
        None,
        Some(WihBinding {
            wih_id: "wih_missing",
            gate: &f.gate,
            leases: &f.leases,
        }),
    )
    .await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
}

#[tokio::test]
async fn decision_event_lands_in_ledger() {
    let f = fixture().await;
    let req = bash("rm -rf /", &f.root);
    let d = decide(&req, &f.root, None, None).await;
    f.ledger.append(decision_event(&req, "claude-code", None, &d)).await.unwrap();
    let events = f
        .ledger
        .query(LedgerQuery {
            r#type: Some(HOOK_EVENT.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["decision"], "deny");
    assert_eq!(events[0].payload["tool"], "Bash");
}

#[tokio::test]
async fn admission_admits_every_harness_in_auto_approve() {
    // Eoj, 2026-09-30: no harness is refused or held out of auto-approve;
    // Allternit's gate is the gate. admit() only records the enforcement class.
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    let leased = load_wih_policy(&f.ledger, &wih).await.unwrap();
    assert_eq!(leased.requires_lease_for_write, Some(true));
    let open = WihPolicy { wih_id: "w".into(), requires_lease_for_write: Some(false) };
    let unknown = load_wih_policy(&f.ledger, "wih_nope").await.unwrap();
    for policy in [Some(&leased), Some(&open), Some(&unknown), None] {
        for h in ["cline", "pi", "agy", "opencode"] {
            assert_eq!(admit(h, policy), Ok(HarnessGate::Ungated), "{h}");
        }
        for h in ["kimi", "gemini", "/usr/bin/kimi"] {
            assert_eq!(admit(h, policy), Ok(HarnessGate::Acp), "{h}");
        }
        for h in ["codex", "claude", "qwen", "/usr/bin/qwen"] {
            assert_eq!(admit(h, policy), Ok(HarnessGate::Hook), "{h}");
        }
    }
}

#[test]
fn claude_settings_carry_hook_in_bypass_mode() {
    let s = claude_settings(HookTarget {
        commrails_bin: Path::new("/opt/bin/allternit-commrails"),
        root: Path::new("/w/it's"),
        workspace: Some(Path::new("/w/wt")),
        wih_id: Some("wih_1"),
    });
    assert_eq!(s["permissions"]["defaultMode"], "bypassPermissions");
    let hook = &s["hooks"]["PreToolUse"][0];
    assert_eq!(hook["matcher"], "*");
    let cmd = hook["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.starts_with("'/opt/bin/allternit-commrails' --root '/w/it'\\''s' hook claude-pretool"));
    assert!(cmd.ends_with("--workspace '/w/wt' --wih 'wih_1'"));
}

#[test]
fn gate_argv_rewrites_bypass_flags() {
    let argv: Vec<String> = ["claude", "-p", "do it", "--dangerously-skip-permissions", "--permission-mode", "bypassPermissions"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let out = gate_argv(&argv, Some(Path::new("/s/settings.json")));
    let joined = out.join(" ");
    assert!(!joined.contains("dangerously"));
    assert_eq!(joined.matches("--permission-mode").count(), 1);
    assert!(joined.ends_with("--permission-mode bypassPermissions --settings /s/settings.json"));

    let argv: Vec<String> = ["codex", "exec", "hi", "--dangerously-bypass-approvals-and-sandbox", "-s", "danger-full-access", "-c", "sandbox_mode=\"danger-full-access\""]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let joined = gate_argv(&argv, None).join(" ");
    assert!(!joined.contains("dangerously"));
    assert!(!joined.contains("-s danger-full-access"));
    assert_eq!(joined.matches("sandbox_mode=").count(), 1);
    assert!(joined.contains("-c sandbox_mode=\"danger-full-access\""));
    assert!(joined.contains("approval_policy=\"never\""));

    let kimi: Vec<String> = vec!["kimi".into(), "--yolo".into()];
    assert_eq!(gate_argv(&kimi, None), kimi);
}

#[test]
fn codex_array_commands_are_unwrapped() {
    let req = HookRequest::from_json(&json!({
        "tool_name": "shell",
        "tool_input": { "command": ["bash", "-lc", "rm -rf ~"] },
        "cwd": "/w",
    }))
    .unwrap();
    assert_eq!(req.command().as_deref(), Some("rm -rf ~"));
    assert!(floor::check(&req.command().unwrap(), Some(Path::new(HOME))).is_some());
}

fn target() -> HookTarget<'static> {
    HookTarget {
        commrails_bin: Path::new("/bin/allternit-commrails"),
        root: Path::new("/r"),
        workspace: Some(Path::new("/w")),
        wih_id: Some("wih_1"),
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn codex_spawn_carries_hook_per_spawn_and_stays_yolo() {
    let argv = strs(&["codex", "exec", "hi", "--dangerously-bypass-approvals-and-sandbox", "-c", "sandbox_mode=\"read-only\""]);
    let out = gate_spawn(&argv, None, Some(target())).argv;
    assert_eq!(out[0], "codex");
    // Global options precede the subcommand.
    let exec_at = out.iter().position(|w| w == "exec").unwrap();
    let hook_at = out.iter().position(|w| w.starts_with("hooks.PreToolUse=")).unwrap();
    assert!(hook_at < exec_at);
    assert!(out.contains(&"--dangerously-bypass-hook-trust".to_string()));
    assert!(out.contains(&"approval_policy=\"never\"".to_string()));
    assert_eq!(out.iter().filter(|w| w.starts_with("sandbox_mode=")).count(), 1);
    assert!(out.contains(&"sandbox_mode=\"danger-full-access\"".to_string()));
    let hook = &out[hook_at];
    assert!(hook.contains("codex-pretool --harness codex"), "{hook}");
    assert!(hook.contains("--wih 'wih_1'"), "{hook}");
}

#[test]
fn qwen_spawn_keeps_yolo_and_points_at_session_settings() {
    let argv = strs(&["qwen", "--approval-mode", "default", "-p", "hi"]);
    let out = gate_spawn(&argv, Some(Path::new("/s/q.json")), Some(target()));
    assert_eq!(out.argv, strs(&["qwen", "-p", "hi", "--yolo"]));
    assert_eq!(
        out.env,
        vec![("QWEN_CODE_SYSTEM_SETTINGS_PATH".to_string(), "/s/q.json".to_string())]
    );
    let (name, settings) = hook_settings_file("qwen", target()).unwrap();
    assert_eq!(name, "qwen-settings.json");
    let cmd = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.contains("qwen-pretool --harness qwen"), "{cmd}");
    assert!(hook_settings_file("codex", target()).is_none());
}

#[tokio::test]
async fn one_decision_path_for_codex_and_qwen_payloads() {
    // codex sends `Bash`; qwen sends `run_shell_command`; both hit the same floor.
    for tool in ["Bash", "run_shell_command"] {
        let deny = HookRequest::from_json(&json!({
            "tool_name": tool, "tool_input": { "command": "rm -rf ~" }, "cwd": "/w", "session_id": "s",
        }))
        .unwrap();
        let ok = HookRequest::from_json(&json!({
            "tool_name": tool, "tool_input": { "command": "ls -la" }, "cwd": "/w", "session_id": "s",
        }))
        .unwrap();
        assert!(decide(&deny, Path::new("/w"), Some(Path::new(HOME)), None).await.verdict.is_deny(), "{tool}");
        assert!(!decide(&ok, Path::new("/w"), Some(Path::new(HOME)), None).await.verdict.is_deny(), "{tool}");
    }
}

/// A second Gate on the same root: stands in for the separate hook / ACP-gate
/// process, which only sees persisted replay state.
fn other_process_gate(f: &Fixture) -> Gate {
    let receipts = Arc::new(
        ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(f.root.clone()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })
        .unwrap(),
    );
    Gate::new(GateOptions {
        ledger: f.ledger.clone(), leases: f.leases.clone(), receipts, index: None, vault: None, oauth_vault: None,
        root_dir: Some(f.root.clone()), actor_id: Some("hook".to_string()), strict_provenance: None,
        visual_provider: None, visual_config: None,
    })
}

#[tokio::test]
async fn replay_hook_denies_during_replay_and_allows_after_end() {
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    let home = Some(Path::new(HOME));
    let hook_gate = other_process_gate(&f);
    let bind = |g| WihBinding { wih_id: &wih, gate: g, leases: &f.leases };
    let inside = || write(&f.root.join("src/lib.rs").to_string_lossy(), &f.root);

    assert!(!decide(&inside(), &f.root, home, Some(bind(&hook_gate))).await.verdict.is_deny());

    // Record a run, then put the bound WIH's run into replay.
    f.gate.record_policy_decision("recorded", "plan", "ALLOW").unwrap();
    f.gate.post_tool("recorded", "shell", json!({"cmd": "touch src/x", "idempotency_key": "hook-replay-01"})).await.unwrap();
    let cassette = f.gate.record_cassette("recorded", None, 0).unwrap();
    f.gate.begin_replay(&wih, cassette).unwrap();

    // Hook process (separate Gate, no in-memory session) denies every tool, reads included.
    for req in [inside(), bash("ls src", &f.root)] {
        let d = decide(&req, &f.root, home, Some(bind(&hook_gate))).await;
        assert!(d.verdict.is_deny(), "{:?}", d.verdict);
        assert!(d.verdict.reason().starts_with("replay: recorded result served by the gate"), "{:?}", d.verdict);
    }
    // Gate 2 pre_tool denies too; post_tool in the other process fails closed.
    let pre = hook_gate.pre_tool(&wih, "Write", &["src/lib.rs".to_string()]).await.unwrap();
    assert!(!pre.allowed && pre.reason.unwrap().starts_with("replay:"));
    let err = hook_gate.post_tool(&wih, "shell", json!({"cmd": "touch src/x", "idempotency_key": "hook-replay-01"})).await.unwrap_err();
    assert!(err.to_string().contains("another process"), "{err}");
    // The owning process serves the recorded result.
    f.gate.record_policy_decision(&wih, "plan", "ALLOW").unwrap();
    f.gate.post_tool(&wih, "shell", json!({"cmd": "touch src/x", "idempotency_key": "hook-replay-01"})).await.unwrap();

    let rep = f.gate.end_replay(&wih).unwrap();
    assert_eq!(rep.verdict, crate::replay::Verdict::Identical, "{rep:?}");
    assert!(!hook_gate.is_replaying(&wih));
    let d = decide(&inside(), &f.root, home, Some(bind(&hook_gate))).await;
    assert!(!d.verdict.is_deny(), "allowed again after end_replay: {:?}", d.verdict);
}
