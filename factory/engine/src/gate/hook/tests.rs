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
    let open = WihPolicy { wih_id: "w".into(), requires_lease_for_write: Some(false), fence_strict: false };
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
        factory_bin: Path::new("/opt/bin/allternit-factory"),
        root: Path::new("/w/it's"),
        workspace: Some(Path::new("/w/wt")),
        wih_id: Some("wih_1"),
    });
    assert_eq!(s["permissions"]["defaultMode"], "bypassPermissions");
    let hook = &s["hooks"]["PreToolUse"][0];
    assert_eq!(hook["matcher"], "*");
    let cmd = hook["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.starts_with("'/opt/bin/allternit-factory' internal hook --root '/w/it'\\''s' claude-pretool"));
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
        factory_bin: Path::new("/bin/allternit-factory"),
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

// ------------------------------------------------ review fixes #2 #3 #4 #5

fn bind<'a>(f: &'a Fixture, wih: &'a str) -> WihBinding<'a> {
    WihBinding {
        wih_id: wih,
        gate: &f.gate,
        leases: &f.leases,
    }
}

/// An open-signed WIH holding `src/**`, with an optional node policy.
async fn bound_wih_with(f: &Fixture, policy: Option<crate::judge::policy::JudgePolicy>) -> String {
    let (_, dag_id, node_id) = f.gate.plan_new("spawn gate test", None).await.unwrap();
    if let Some(p) = policy {
        f.gate
            .set_judge_policy(
                &dag_id,
                Some(&node_id),
                p,
                &Actor {
                    r#type: ActorType::User,
                    id: "eoj".to_string(),
                },
            )
            .await
            .unwrap();
    }
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

/// Unscannable programs used in the #2 tests. None of them name a path
/// literal outside the lease.
const UNSCANNABLE: &[&str] = &[
    "cargo test",
    "npm test",
    "python -c 'print(1)'",
    r#"python3 -c 'open(p,"w").write("x")'"#,
    "bash ./script.sh",
    "make -j4",
    "sed -n 'w out.txt' src/a.rs",
    "if true; then python3 -c 'x'; fi",
];

/// #2 under Q25 (guardrails, not walls): inline interpreter code and
/// unscannable programs are allowed and recorded as `unresolved_effect`,
/// denied when a statically visible path literal lands outside the lease or
/// workspace.
#[tokio::test]
async fn unresolved_effects_are_allowed_and_recorded_under_wih() {
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    let home = Some(Path::new(HOME));
    for cmd in UNSCANNABLE {
        let req = bash(cmd, &f.root);
        let d = decide(&req, &f.root, home, Some(bind(&f, &wih))).await;
        assert!(!d.verdict.is_deny(), "{cmd}: {:?}", d.verdict);
        assert!(d.verdict.reason().contains("unresolved_effect recorded"), "{cmd}: {:?}", d.verdict);
        let evt = decision_event(&req, "claude-code", Some(&wih), &d);
        let rec = evt.payload["unresolved_effect"].as_array().expect("recorded");
        assert!(!rec.is_empty(), "{cmd}: {}", evt.payload);
        assert_eq!(evt.payload["command"], *cmd);
        assert_eq!(evt.scope.as_ref().unwrap().wih_id.as_deref(), Some(wih.as_str()));
    }
    // A path literal outside the workspace or the lease is denied.
    for cmd in [
        r#"python3 -c 'open("/outside/x","w").write("x")'"#,
        r#"node -e 'require("fs").writeFileSync("/outside/x","x")'"#,
        r#"perl -e 'open(F,">/outside/x")'"#,
        r#"ruby -e 'File.write("../x","x")'"#,
        "./custom-tool --out /outside/x",
        "awk 'BEGIN{print 1 > \"/outside/x\"}'",
        "find . -name x -exec sh -c 'echo hi > /outside/x' +",
        "sed -n 'w /outside/x' src/a.rs",
        "sort -o /outside/x input.txt",
    ] {
        let d = decide(&bash(cmd, &f.root), &f.root, home, Some(bind(&f, &wih))).await;
        assert!(d.verdict.is_deny(), "{cmd}: {:?}", d.verdict);
    }
    let docs = f.root.join("docs/x.md");
    let cmd = format!("python3 -c 'open(\"{}\",\"w\")'", docs.display());
    let d = decide(&bash(&cmd, &f.root), &f.root, home, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "in workspace, outside lease: {:?}", d.verdict);
    let inside = f.root.join("src/x.rs");
    let cmd = format!("python3 -c 'open(\"{}\",\"w\")'", inside.display());
    let d = decide(&bash(&cmd, &f.root), &f.root, home, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny(), "inside lease: {:?}", d.verdict);
    // Ordinary read-only commands record nothing.
    for cmd in [
        "ls -la",
        "cat src/lib.rs | head -5",
        "git status",
        "rg foo src",
        "sed -n '1,5p' src/a.rs",
        "sed 's/world/there/g' src/a.rs",
        "if [ -f src/x ]; then cat src/x; fi",
        "for f in a b; do echo $f; done",
    ] {
        let req = bash(cmd, &f.root);
        let d = decide(&req, &f.root, home, Some(bind(&f, &wih))).await;
        assert!(!d.verdict.is_deny(), "{cmd}: {:?}", d.verdict);
        assert!(req.unresolved_effects().is_empty(), "{cmd}");
    }
    // No WIH bound: only the hard floor applies (Q24 auto-approve unchanged).
    let d = decide(&bash("python3 -c 'print(1)'", &f.root), &f.root, home, None).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
}

/// #2 with the opt-in strict fence: every unresolved effect is denied.
#[tokio::test]
async fn strict_fence_denies_unresolved_effects() {
    let f = fixture().await;
    let wih = bound_wih_with(
        &f,
        Some(crate::judge::policy::JudgePolicy {
            fence: Some(crate::judge::policy::Fence::Strict),
            ..Default::default()
        }),
    )
    .await;
    for cmd in UNSCANNABLE {
        let d = decide(&bash(cmd, &f.root), &f.root, None, Some(bind(&f, &wih))).await;
        assert!(d.verdict.is_deny(), "{cmd}: {:?}", d.verdict);
        assert!(d.verdict.reason().contains("strict fence"), "{cmd}: {:?}", d.verdict);
    }
    let d = decide(&bash("git status && ls", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
}

/// #3: a symlink inside the leased directory that points outside the
/// workspace does not make its target writable.
#[tokio::test]
async fn symlink_escape_from_lease_is_denied() {
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    let outside = TempDir::new().unwrap();
    std::fs::create_dir_all(f.root.join("src")).unwrap();
    std::os::unix::fs::symlink(outside.path(), f.root.join("src/link")).unwrap();
    let target = f.root.join("src/link/x");
    let d = decide(&write(&target.to_string_lossy(), &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    let d = decide(&write("src/link/x", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    let d = decide(&bash("touch src/link/x", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    // A symlink that stays in the workspace is checked at its destination.
    std::fs::create_dir_all(f.root.join("docs")).unwrap();
    std::os::unix::fs::symlink(f.root.join("docs"), f.root.join("src/docs-link")).unwrap();
    let d = decide(&write("src/docs-link/x.md", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    // A plain file under the lease is still fine.
    let d = decide(&write("src/real.rs", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
}

/// #4: `src/**` covers `src` and `src/...`, never a sibling like `src-private`.
#[tokio::test]
async fn recursive_lease_does_not_cover_sibling_prefix() {
    let f = fixture().await;
    let wih = bound_wih(&f).await;
    for p in ["src-private/secret.ts", "src2/x"] {
        let d = decide(&write(p, &f.root), &f.root, None, Some(bind(&f, &wih))).await;
        assert!(d.verdict.is_deny(), "{p}: {:?}", d.verdict);
    }
    for p in ["src/a.rs", "src/deep/b.rs"] {
        let d = decide(&write(p, &f.root), &f.root, None, Some(bind(&f, &wih))).await;
        assert!(!d.verdict.is_deny(), "{p}: {:?}", d.verdict);
    }
    let d = decide(&bash("mkdir src", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
}

/// #5: the judge hard rules see the actual command, so a judge that says
/// allow cannot approve `sudo`.
#[tokio::test]
async fn hook_passes_command_to_judge_hard_rules() {
    let f = fixture().await;
    let f = Fixture {
        gate: f.gate.with_judge(
            Arc::new(crate::judge::StubJudge::new("accomplished", "allow")),
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(300),
        ),
        ..f
    };
    let (_, dag_id, node_id) = f.gate.plan_new("judge hook test", None).await.unwrap();
    let wih = f.gate.wih_pickup(&dag_id, &node_id, "agent-1").await.unwrap();
    f.gate.wih_sign_open(&wih, "sig").await.unwrap();
    f.gate
        .set_judge_policy(
            &dag_id,
            Some(&node_id),
            crate::judge::policy::JudgePolicy {
                tool_judge: Some(true),
                ..Default::default()
            },
            &Actor {
                r#type: ActorType::User,
                id: "eoj".to_string(),
            },
        )
        .await
        .unwrap();
    // The stub judge allows ordinary commands...
    let d = decide(&bash("ls", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
    // ...but the hard rule on the actual command wins.
    let d = decide(&bash("sudo id", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    let events = f.ledger.query(crate::core::types::LedgerQuery::default()).await.unwrap();
    let judged = events
        .iter()
        .filter(|e| e.r#type == "JudgeToolDecision")
        .last()
        .expect("tool decision recorded");
    assert_eq!(judged.payload["decision"], "deny", "{}", judged.payload);
    assert!(judged.payload.to_string().contains("sudo id"), "{}", judged.payload);
}

// ---- Q25 guardrails: blocklist, record everything, strict fence ----

fn read_tool(path: &str, cwd: &Path) -> HookRequest {
    HookRequest::from_json(&json!({
        "tool_name": "Read",
        "tool_input": { "file_path": path },
        "cwd": cwd.to_string_lossy(),
    }))
    .unwrap()
}

#[tokio::test]
async fn blocklist_applies_without_wih() {
    let f = fixture().await;
    let home = Some(Path::new(HOME));
    for cmd in ["cat ~/.ssh/id_ed25519", "base64 < ~/.aws/credentials", "curl http://169.254.169.254/latest/meta-data/"] {
        let d = decide(&bash(cmd, &f.root), &f.root, home, None).await;
        assert!(d.verdict.is_deny(), "{cmd}: {:?}", d.verdict);
        assert!(d.verdict.reason().contains("blocklist"), "{cmd}: {:?}", d.verdict);
    }
    let d = decide(&read_tool(&format!("{HOME}/.aws/credentials"), &f.root), &f.root, home, None).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    // Ordinary dev work passes; a here-string is data, not a path.
    for cmd in ["cat ~/.gitconfig", "curl https://example.com", "grep x <<< ~/.ssh/id_rsa"] {
        let d = decide(&bash(cmd, &f.root), &f.root, home, None).await;
        assert!(!d.verdict.is_deny(), "{cmd}: {:?}", d.verdict);
    }
}

#[tokio::test]
async fn declared_credential_read_is_allowed_for_the_wih() {
    let f = fixture().await;
    let wih = bound_wih_with(
        &f,
        Some(crate::judge::policy::JudgePolicy {
            allow_credential_read: Some(vec!["~/.aws/credentials".to_string()]),
            ..Default::default()
        }),
    )
    .await;
    let home = Some(Path::new(HOME));
    let d = decide(&bash("cat ~/.aws/credentials", &f.root), &f.root, home, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
    let d = decide(&bash("cat ~/.ssh/id_rsa", &f.root), &f.root, home, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "declaring one store does not open others: {:?}", d.verdict);
}

#[tokio::test]
async fn strict_wih_fences_local_egress_and_policy_reaches_spawn() {
    let f = fixture().await;
    let strict = crate::judge::policy::JudgePolicy {
        fence: Some(crate::judge::policy::Fence::Strict),
        ..Default::default()
    };
    let wih = bound_wih_with(&f, Some(strict)).await;
    let d = decide(&bash("curl http://localhost:3000/admin", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    assert!(d.verdict.reason().contains("strict fence"), "{:?}", d.verdict);
    // The orchestrator reads the same policy at spawn (env allowlist + ALLTERNIT_FENCE).
    assert!(load_wih_policy(&f.ledger, &wih).await.unwrap().fence_strict);
    let open = bound_wih(&f).await;
    assert!(!load_wih_policy(&f.ledger, &open).await.unwrap().fence_strict);
    let d = decide(&bash("curl http://localhost:3000/admin", &f.root), &f.root, None, Some(bind(&f, &open))).await;
    assert!(!d.verdict.is_deny(), "guardrail default allows local egress: {:?}", d.verdict);
}

#[test]
fn unbound_strict_fence_keeps_writes_in_worktree_and_temp() {
    let root = TempDir::new().unwrap();
    let d = decide_unbound(&write("/etc/hosts", root.path()), root.path(), None, true);
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    let d = decide_unbound(&write("src/a.rs", root.path()), root.path(), None, true);
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
    let tmp = std::env::temp_dir().join("q25-x");
    let d = decide_unbound(&write(&tmp.to_string_lossy(), root.path()), root.path(), None, true);
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
    // Guardrail default: writes anywhere are allowed (and recorded).
    let d = decide_unbound(&write("/etc/hosts", root.path()), root.path(), None, false);
    assert!(!d.verdict.is_deny(), "{:?}", d.verdict);
    assert_eq!(d.paths, vec!["/etc/hosts".to_string()]);
}

#[tokio::test]
async fn every_decision_is_recorded() {
    let f = fixture().await;
    let home = Some(Path::new(HOME));
    for cmd in ["ls -la", "cat ~/.ssh/id_rsa", "python3 -c 'print(1)'"] {
        let req = bash(cmd, &f.root);
        let d = decide(&req, &f.root, home, None).await;
        record_decision(&f.ledger, Some(&req), "claude", None, &d).await;
    }
    let events = f
        .ledger
        .query(LedgerQuery { r#type: Some(HOOK_EVENT.to_string()), ..Default::default() })
        .await
        .unwrap();
    let labels: Vec<_> = events.iter().filter_map(|e| e.payload["decision"].as_str().map(str::to_string)).collect();
    assert_eq!(labels, ["allow", "deny", "unresolved"]);
}

// ---- Agent rules enforcement (ask / deny / allow) ----

fn write_tool(path: &str, cwd: &Path) -> HookRequest {
    HookRequest::from_json(&json!({
        "tool_name": "Write",
        "tool_input": { "file_path": path, "content": "x" },
        "cwd": cwd.to_string_lossy(),
    }))
    .unwrap()
}

fn rule(id: &str, when: &str, action: crate::judge::policy::RuleAction) -> crate::judge::policy::CustomRule {
    crate::judge::policy::CustomRule { id: id.into(), text: format!("no {when}"), when: when.into(), action }
}

#[tokio::test]
async fn outside_scope_write_asks_when_enabled_else_denies() {
    let f = fixture().await;
    let outside = format!("{}/elsewhere-xyz/out.txt", f.root.parent().unwrap().display());
    let ask = bound_wih_with(&f, Some(crate::judge::policy::JudgePolicy { ask_outside_scope: Some(true), ..Default::default() })).await;
    let d = decide(&write_tool(&outside, &f.root), &f.root, None, Some(bind(&f, &ask))).await;
    assert!(matches!(d.verdict, Verdict::Ask(_)), "{:?}", d.verdict);
    assert_eq!(decision_label(&write_tool(&outside, &f.root), &d), "ask");
    let plain = bound_wih(&f).await;
    let d = decide(&write_tool(&outside, &f.root), &f.root, None, Some(bind(&f, &plain))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
}

#[tokio::test]
async fn private_network_asks_but_metadata_stays_denied() {
    let f = fixture().await;
    let wih = bound_wih_with(&f, Some(crate::judge::policy::JudgePolicy { ask_private_network: Some(true), ..Default::default() })).await;
    let d = decide(&bash("curl http://10.1.2.3/api", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(matches!(d.verdict, Verdict::Ask(_)), "{:?}", d.verdict);
    let d = decide(&bash("curl http://169.254.169.254/latest", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny(), "{:?}", d.verdict);
    let d = decide(&bash("curl https://example.com", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(!d.verdict.is_deny() && !matches!(d.verdict, Verdict::Ask(_)), "{:?}", d.verdict);
    let open = bound_wih(&f).await;
    let d = decide(&bash("curl http://10.1.2.3/api", &f.root), &f.root, None, Some(bind(&f, &open))).await;
    assert!(matches!(d.verdict, Verdict::Allow(_)), "no rule, no ask: {:?}", d.verdict);
}

#[tokio::test]
async fn custom_rules_ask_deny_allow_and_reach_the_ledger() {
    use crate::judge::policy::RuleAction::{Ask, Deny};
    let f = fixture().await;
    let wih = bound_wih_with(
        &f,
        Some(crate::judge::policy::JudgePolicy {
            custom_rules: Some(vec![rule("r1", "npm publish", Ask), rule("r2", "rm -rf *build", Deny)]),
            ..Default::default()
        }),
    )
    .await;
    let d = decide(&bash("npm publish --tag x", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(matches!(d.verdict, Verdict::Ask(_)), "{:?}", d.verdict);
    let req = bash("rm -rf ./build", &f.root);
    let d = decide(&req, &f.root, None, Some(bind(&f, &wih))).await;
    assert!(d.verdict.is_deny() && d.verdict.reason().contains("r2"), "{:?}", d.verdict);
    record_decision(&f.ledger, Some(&req), "claude", Some(&wih), &d).await;
    let d = decide(&bash("ls", &f.root), &f.root, None, Some(bind(&f, &wih))).await;
    assert!(matches!(d.verdict, Verdict::Allow(_)), "{:?}", d.verdict);
    assert!(rules::when_matches("src/*.env", "cat src/app.env"));
    assert!(!rules::when_matches("", "anything"));
}

#[test]
fn hook_request_carries_the_harness_tool_call_id() {
    let r = HookRequest::from_json(&json!({ "tool_name": "Bash", "tool_input": {}, "tool_use_id": "toolu_9" })).unwrap();
    assert_eq!(r.tool_call_id.as_deref(), Some("toolu_9"));
    let r = HookRequest::from_json(&json!({ "tool_name": "shell", "call_id": "call_3" })).unwrap();
    assert_eq!(r.tool_call_id.as_deref(), Some("call_3"));
    let r = HookRequest::from_json(&json!({ "tool_name": "Bash", "tool_use_id": "" })).unwrap();
    assert_eq!(r.tool_call_id, None);
}

#[test]
fn claude_settings_register_the_s1_outcome_hooks_when_given() {
    let t = HookTarget { factory_bin: Path::new("/opt/bin/allternit-factory"), root: Path::new("/w"), workspace: None, wih_id: None };
    let s = claude_settings_with_outcome(t, Some("'/opt/bin/system-one' hook-outcome"));
    for ev in ["PermissionRequest", "PostToolUse", "PostToolUseFailure"] {
        assert_eq!(s["hooks"][ev][0]["matcher"], "*", "{ev}");
        assert_eq!(s["hooks"][ev][0]["hooks"][0]["command"], "'/opt/bin/system-one' hook-outcome", "{ev}");
    }
    assert!(s["hooks"]["Stop"][0].get("matcher").is_none());
    assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "'/opt/bin/system-one' hook-outcome");
    // The gate itself is unchanged.
    assert!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().contains("internal hook --root"));
    let bare = claude_settings_with_outcome(t, None);
    assert_eq!(bare["hooks"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["PreToolUse"]);
    // Qwen gets the same outcome hooks next to its own PreToolUse gate.
    let q = qwen_settings_with_outcome(t, Some("'/opt/bin/system-one' hook-outcome"));
    for ev in ["PermissionRequest", "PostToolUse", "PostToolUseFailure", "Stop"] {
        assert_eq!(q["hooks"][ev][0]["hooks"][0]["command"], "'/opt/bin/system-one' hook-outcome", "{ev}");
    }
    assert!(q["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().contains("--harness qwen"));
    assert_eq!(qwen_settings_with_outcome(t, None)["hooks"].as_object().unwrap().len(), 1);
}

#[test]
fn s1_outcome_hooks_have_an_env_opt_out_and_an_explicit_binary() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("system-one");
    std::fs::write(&bin, "").unwrap();
    std::env::set_var("ALLTERNIT_SYSTEM_ONE_BIN", &bin);
    std::env::set_var("ALLTERNIT_S1_OUTCOME_HOOKS", "1");
    assert_eq!(s1_outcome_hook_command(), Some(format!("'{}' hook-outcome", bin.display())));
    std::env::set_var("ALLTERNIT_S1_OUTCOME_HOOKS", "0");
    assert_eq!(s1_outcome_hook_command(), None);
    std::env::remove_var("ALLTERNIT_S1_OUTCOME_HOOKS");
    std::env::remove_var("ALLTERNIT_SYSTEM_ONE_BIN");
}
