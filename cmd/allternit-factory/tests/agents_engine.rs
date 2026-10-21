//! Agent sessions on the engine path: the gated spawn, the mailbox drain and
//! `agents recover`, through the [`Spawner`] and a fake pane backend whose
//! panes are real child processes (see `support/fake_panes.rs`).
//!
//! The harnesses are stand-in scripts on `PATH` that record the argv they
//! were started with, so the tests see exactly what the spawn gate launched.
//!
//! [`Spawner`]: allternit_factory_engine::spawn::Spawner

#[path = "support/fake_panes.rs"]
mod fake_panes;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use allternit_factory_engine::backend::{self, PaneBackend};
use allternit_factory_engine::registry::{Entry, Registry};
use allternit_factory_engine::spawn::{resume_argv, RecoverOptions, SpawnOptions, Spawner};
use tempfile::TempDir;

struct Env {
    panes: Arc<fake_panes::FakePanes>,
    /// Factory home (registry, logs) and the stand-in harnesses' bin dir.
    home: TempDir,
}

/// One fake pane engine, factory home and PATH per test process; the tests
/// share them, so they run one at a time.
fn env() -> (&'static Env, MutexGuard<'static, ()>) {
    static ENV: OnceLock<Env> = OnceLock::new();
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let env = ENV.get_or_init(|| {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Each stand-in harness writes its argv (one per line) next to it, then
        // stays up like an interactive agent would.
        for name in ["claude", "kimi"] {
            let script = bin.join(name);
            std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}/{name}.argv\"\nexec sleep 30\n", bin.display()))
                .unwrap();
            make_executable(&script);
        }
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        std::env::set_var("PATH", path);
        std::env::set_var("ALLTERNIT_FACTORY_HOME", home.path().join("factory"));
        // The spawn gate's hook is the engine binary.
        std::env::set_var("ALLTERNIT_FACTORY_BIN", env!("CARGO_BIN_EXE_allternit-factory"));
        Env { panes: fake_panes::FakePanes::install(), home }
    });
    (env, guard)
}

#[cfg(unix)]
fn make_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn argv_seen(env: &Env, harness: &str) -> Vec<String> {
    let file = env.home.path().join("bin").join(format!("{harness}.argv"));
    for _ in 0..50 {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let _ = std::fs::remove_file(&file);
            return text.lines().map(str::to_string).collect();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("{harness} was never started");
}

fn spawn_opts<'a>(slug: &'a str, repo: &'a Path, cmd: &'a [String], vendor: &'a str) -> SpawnOptions<'a> {
    SpawnOptions {
        slug,
        repo,
        cmd,
        worktree: false,
        vendor,
        mode: "test",
        task_file: None,
        notes_sentinel: None,
        wih: None,
        capture: None,
        bot: None,
        env: BTreeMap::from([("ALLTERNIT_FACTORY_TEAM".to_string(), "t".to_string())]),
        lead: Some("lead-a".into()),
    }
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|s| s.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_spawn_is_gated_and_recorded() {
    let (env, _serial) = env();
    let root = tempfile::tempdir().unwrap();
    let spawner = Spawner::new(root.path().to_path_buf()).unwrap();
    let cmd = words(&["claude", "--dangerously-skip-permissions", "--model", "opus"]);
    let r = spawner.spawn(spawn_opts("gated", root.path(), &cmd, "claude")).await.unwrap();

    // The bypass flag never reaches the harness; the Gate hook settings do.
    let argv = argv_seen(env, "claude");
    assert!(!argv.iter().any(|a| a == "--dangerously-skip-permissions"), "{argv:?}");
    let at = argv.iter().position(|a| a == "--settings").expect("--settings");
    let settings = std::fs::read_to_string(&argv[at + 1]).unwrap();
    assert!(settings.contains("internal hook") && settings.contains("claude-pretool"), "{settings}");
    assert!(argv.windows(2).any(|w| w == ["--model", "opus"]), "{argv:?}");

    // The registry keeps what was asked for (recover rebuilds from it).
    let entry = &Registry::open_default().load().unwrap().sessions[&r.session];
    assert_eq!(entry.argv.as_deref(), Some(&cmd[..]));
    assert_eq!(entry.env.get("ALLTERNIT_FACTORY_TEAM").map(String::as_str), Some("t"));
    assert_eq!(entry.lead.as_deref(), Some("lead-a"));
    assert!(entry.is_running());

    spawner.kill("gated", false).await.unwrap();
    let entry = &Registry::open_default().load().unwrap().sessions[&r.session];
    assert!(entry.dead);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hooked_harness_without_the_engine_binary_is_refused() {
    let (env, _serial) = env();
    let root = tempfile::tempdir().unwrap();
    let spawner = Spawner::new(root.path().to_path_buf()).unwrap();
    std::env::remove_var("ALLTERNIT_FACTORY_BIN");
    let cmd = words(&["claude", "-p", "hi"]);
    let res = spawner.spawn(spawn_opts("unhooked", root.path(), &cmd, "claude")).await;
    std::env::set_var("ALLTERNIT_FACTORY_BIN", env!("CARGO_BIN_EXE_allternit-factory"));
    let err = format!("{:#}", res.err().expect("refused"));
    assert!(err.contains("spawn-gate hook"), "{err}");
    // Never run unhooked: no pane, no record.
    assert!(env.panes.find("ao-unhooked").unwrap().is_none());
    assert!(!Registry::open_default().load().unwrap().sessions.contains_key("ao-unhooked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_delivers_oldest_first_and_settles_only_verified() {
    let (env, _serial) = env();
    let root = tempfile::tempdir().unwrap();
    let spawner = Spawner::new(root.path().to_path_buf()).unwrap();
    let cmd = words(&["kimi"]);
    spawner.spawn(spawn_opts("mail", root.path(), &cmd, "kimi")).await.unwrap();
    argv_seen(env, "kimi");
    let pane = backend::backend().unwrap();
    let session = "ao-mail";

    // A busy pane: both sends queue, and a drain leaves the oldest queued.
    *env.panes.busy.lock().unwrap() = true;
    for text in ["first", "second", "third"] {
        assert!(matches!(pane.send(root.path(), session, text, "user:t", false).unwrap(), backend::PaneSend::Queued { .. }));
    }
    let r = root.path().to_path_buf();
    let left = backend::blocking(move || backend::drain(&r, session, true)).await.unwrap();
    assert_eq!(left.len(), 1);
    assert!(!left[0].delivered);
    assert_eq!(pane.mailbox(root.path(), session).unwrap().len(), 3, "nothing settled");

    // Idle: one message without --all, then the rest, in order.
    *env.panes.busy.lock().unwrap() = false;
    let r = root.path().to_path_buf();
    let one = backend::blocking(move || backend::drain(&r, session, false)).await.unwrap();
    assert_eq!(one.len(), 1);
    assert!(one[0].delivered);
    let r = root.path().to_path_buf();
    let rest = backend::blocking(move || backend::drain(&r, session, true)).await.unwrap();
    assert_eq!(rest.iter().filter(|d| d.delivered).count(), 2);
    assert!(pane.mailbox(root.path(), session).unwrap().is_empty());
    let sent: Vec<String> =
        env.panes.sent.lock().unwrap().iter().filter(|(s, _)| s == session).map(|(_, t)| t.clone()).collect();
    assert_eq!(sent, ["first", "second", "third"]);

    // Nothing queued: an empty drain, not an error.
    let r = root.path().to_path_buf();
    assert!(backend::blocking(move || backend::drain(&r, session, true)).await.unwrap().is_empty());
    spawner.kill("mail", false).await.unwrap();
}

#[test]
fn resume_argv_uses_each_harness_own_resume() {
    let r = |w: &[&str]| resume_argv(&words(w));
    assert_eq!(r(&["codex", "resume", "c-1"]), Some(words(&["codex", "resume", "c-1"])));
    assert_eq!(r(&["claude", "--dangerously-skip-permissions", "--resume", "k-2"]), Some(words(&["claude", "--resume", "k-2"])));
    assert_eq!(r(&["kimi", "-S", "k-3", "--yolo"]), Some(words(&["kimi", "--session", "k-3"])));
    assert_eq!(r(&["/opt/bin/agy", "--conversation", "a-4"]), Some(words(&["/opt/bin/agy", "--conversation", "a-4"])));
    assert_eq!(r(&["grok", "--resume", "g-5"]), Some(words(&["grok", "--resume", "g-5"])));
    // No session reference: relaunched verbatim.
    assert_eq!(r(&["claude", "-p", "hi"]), None);
    assert_eq!(r(&["codex", "resume", "--last"]), None);
    assert_eq!(r(&["cat"]), None);
}

fn record(reg: &Registry, session: &str, entry: Entry) {
    reg.update(|f| {
        f.sessions.insert(session.to_string(), entry);
    })
    .unwrap();
}

fn dead(cwd: &Path, lead: Option<&str>, argv: Option<&[&str]>) -> Entry {
    Entry {
        cwd: cwd.to_string_lossy().to_string(),
        dead: true,
        lifecycle: Some("dead".into()),
        world: Some("engine".into()),
        lead: lead.map(str::to_string),
        argv: argv.map(words),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn recover_plans_owned_unfinished_sessions_and_respawns_with_resume() {
    let (env, _serial) = env();
    let root = tempfile::tempdir().unwrap();
    let reg_dir = tempfile::tempdir().unwrap();
    let reg = Registry::at(reg_dir.path().join("registry.json"));
    let spawner = Spawner::new(root.path().to_path_buf()).unwrap().with_registry(Registry::at(reg.path()));
    let cwd = root.path();

    let mut resumable = dead(cwd, Some("lead-a"), Some(&["kimi", "--session", "abc", "--yolo"]));
    resumable.env = BTreeMap::from([("ALLTERNIT_FACTORY_BOT".to_string(), "builder".to_string())]);
    record(&reg, "ao-r-resume", resumable);
    let done = cwd.join("DONE.sentinel");
    std::fs::write(&done, "status: done\n").unwrap();
    let mut finished_by_sentinel = dead(cwd, Some("lead-a"), Some(&["kimi"]));
    finished_by_sentinel.sentinel = Some(done.to_string_lossy().to_string());
    record(&reg, "ao-r-done", finished_by_sentinel);
    record(&reg, "ao-r-other", dead(cwd, Some("lead-b"), Some(&["kimi"])));
    record(&reg, "ao-r-nolead", dead(cwd, None, Some(&["kimi"])));
    let mut finished = dead(cwd, Some("lead-a"), Some(&["kimi"]));
    finished.lifecycle = Some("finished".into());
    record(&reg, "ao-r-finished", finished);
    record(&reg, "ao-r-nocmd", dead(cwd, Some("lead-a"), None));

    // The plan: nothing is started.
    let plan = spawner.recover(RecoverOptions { only: None, apply: false, caller: "lead-a", as_human: false }).await.unwrap();
    let by: BTreeMap<&str, &allternit_factory_engine::spawn::RecoverStep> = plan.iter().map(|s| (s.session.as_str(), s)).collect();
    assert_eq!(by["ao-r-resume"].action, "plan");
    assert!(by["ao-r-resume"].resumed);
    assert_eq!(by["ao-r-resume"].argv.as_deref(), Some(&words(&["kimi", "--session", "abc"])[..]));
    assert_eq!(by["ao-r-done"].action, "skip");
    assert_eq!(by["ao-r-other"].action, "refuse", "another lead's session");
    assert_eq!(by["ao-r-nolead"].action, "refuse", "no recorded owner is never a guess");
    assert!(!by.contains_key("ao-r-finished"));
    assert_eq!(by["ao-r-nocmd"].action, "skip");
    assert!(env.panes.find("ao-r-resume").unwrap().is_none());

    // --as-human overrides ownership.
    let human = spawner.recover(RecoverOptions { only: Some("r-other"), apply: false, caller: "x", as_human: true }).await.unwrap();
    assert_eq!(human[0].action, "plan");

    // --apply respawns through the spawn path, with the harness's resume.
    let applied = spawner.recover(RecoverOptions { only: Some("r-resume"), apply: true, caller: "lead-a", as_human: false }).await.unwrap();
    assert_eq!(applied[0].action, "recovered", "{applied:?}");
    assert_eq!(argv_seen(env, "kimi"), words(&["--session", "abc"]));
    let entry = &reg.load().unwrap().sessions["ao-r-resume"];
    assert!(entry.is_running());
    assert_eq!(entry.lead.as_deref(), Some("lead-a"), "the owner is kept");
    assert_eq!(entry.env.get("ALLTERNIT_FACTORY_BOT").map(String::as_str), Some("builder"));
    // The sentinel said done: --apply marks it finished.
    spawner.recover(RecoverOptions { only: Some("r-done"), apply: true, caller: "lead-a", as_human: false }).await.unwrap();
    assert_eq!(reg.load().unwrap().sessions["ao-r-done"].lifecycle.as_deref(), Some("finished"));

    // A live session is skipped, not respawned twice.
    let again = spawner.recover(RecoverOptions { only: Some("r-resume"), apply: true, caller: "lead-a", as_human: false }).await.unwrap();
    assert_eq!((again[0].action.as_str(), again[0].detail.as_str()), ("skip", "alive"));
    spawner.kill("r-resume", false).await.unwrap();
}
