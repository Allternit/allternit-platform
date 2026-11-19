//! Sync-root test for the Windows helper (plan step 24 done check). Runs on a
//! Windows CI runner, unpackaged, against an in-process `files/2` runtime:
//!
//!   cargo test --manifest-path surfaces/allternit-desktop/native/files-win/Cargo.toml -- --nocapture --test-threads=1
//!
//! Real cfapi end to end: register, on-demand listing, FETCH_DATA hydration in
//! aligned ranges of at most 4 MiB, free up space, write back through an upload
//! session, a keep-both conflict, the change feed, ask-before-wake (browsing never
//! wakes; "Not now" never wakes; "Wake" wakes once), unregister.
#![cfg(windows)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use allternit_files_win::cf;
use allternit_files_win::client::{FilesClient, Transport};
use allternit_files_win::provider::{Options, Provider, SyncOutcome};
use allternit_files_win::sync_root::{self, Registration, Spec};
use allternit_files_win::wake::{WakeAnswer, WakeGate, WakePrompt};
use support::{Fake, COMPUTER};

struct TestPrompt(Arc<Mutex<WakeAnswer>>);

impl WakePrompt for TestPrompt {
    fn ask(&self, _computer: &str, _what: &str) -> WakeAnswer {
        *self.0.lock().unwrap()
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn log(s: &str) {
    println!("[files-win] {s}");
}

#[test]
fn sync_root_end_to_end() {
    // 9 MiB + 123: three ranges, the last one unaligned at EOF.
    let big: Vec<u8> = (0..(9 * 1024 * 1024 + 123) as u32).map(|i| (i % 251) as u8).collect();
    let fake = Fake::start();
    let ids = fake.seed(&big);

    let base = std::env::temp_dir().join(format!("allternit-files-win-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root: PathBuf = base.join("Allternit Files").join("Test Computer");
    let state_dir = base.join("state");

    // (1) register
    let spec = Spec { path: &root, computer_id: COMPUTER, display_name: "Test Computer", icon: None };
    log("STEP0 fake runtime up, registering");
    let registration = sync_root::register(&spec, &state_dir).expect("register sync root");
    log(&format!("STEP1 registered via {registration:?}"));
    assert!(sync_root::is_registered(&root), "cfapi knows the sync root");

    let client = Arc::new(FilesClient::new(
        Transport::Direct { base: format!("{}/api/v1/files/v2", fake.base()) },
        &fake.base(),
        COMPUTER,
        "dev_test",
        "Test PC",
    ));
    client.set_token(Some("test-token".into()));
    let answer = Arc::new(Mutex::new(WakeAnswer::NotNow));
    let wake = Arc::new(WakeGate::new("Test Computer", Box::new(TestPrompt(Arc::clone(&answer)))).with_decline_hold(Duration::ZERO));
    let provider = Provider::new(
        Options {
            root: root.clone(),
            state_file: state_dir.join("index.json"),
            icons_dir: base.join("icons"),
            badges: matches!(registration, Registration::StorageProvider { .. }),
        },
        Arc::clone(&client),
        Arc::clone(&wake),
    );
    let conn = provider.connect().expect("CfConnectSyncRoot");

    // (2) on-demand listing: roots at the top, protected Memory Drive hidden.
    assert_eq!(names(&root), ["Allternit", "Code"]);
    let home = root.join("Allternit");
    assert_eq!(names(&home), ["big.bin", "docs", "hello.txt"]);
    log("STEP2 FETCH_PLACEHOLDERS listed roots and Allternit");

    // (3) hydrate on open, exact bytes, aligned ranges of at most 4 MiB.
    let hello = home.join("hello.txt");
    assert_eq!(std::fs::read(&hello).unwrap(), b"hello from the allternit fake runtime\n");
    assert_eq!(std::fs::read(home.join("big.bin")).unwrap(), big, "big.bin bytes");
    {
        let st = fake.state.lock().unwrap();
        let big_ranges: Vec<_> = st.ranges.iter().filter(|(_, l)| *l > 64).cloned().collect();
        log(&format!("STEP3 ranges read: {:?}", st.ranges));
        assert!(big_ranges.len() >= 3, "big.bin came in several ranges");
        for (o, l) in &st.ranges {
            assert_eq!(o % 4096, 0, "range offset aligned");
            assert!(*l <= 4 * 1024 * 1024, "range at most 4 MiB");
        }
    }

    // (4) free up space, then open again: hydrates again from the server.
    let fetches = provider.stats.fetch_data.load(Ordering::SeqCst);
    provider.free_up(&home.join("big.bin")).expect("dehydrate");
    assert_ne!(cf::attributes(&home.join("big.bin")) & cf::ATTR_RECALL_ON_DATA_ACCESS, 0, "dehydrated");
    assert_eq!(std::fs::read(home.join("big.bin")).unwrap(), big);
    assert!(provider.stats.fetch_data.load(Ordering::SeqCst) > fetches, "second open fetched again");
    log("STEP4 dehydrate + rehydrate ok");

    // (5) write back an edit with its base rev.
    std::fs::write(&hello, b"edited on windows\n").unwrap();
    let out = provider.sync_path(&hello).expect("upload edit");
    assert!(matches!(out, SyncOutcome::Uploaded { .. }), "{out:?}");
    assert_eq!(fake.state.lock().unwrap().items[&ids["hello"]].data, b"edited on windows\n");
    assert!(cf::placeholder_state(&hello).unwrap().in_sync, "back in sync after upload");
    log("STEP5 write-back ok");

    // (6) a new local file in a folder becomes a server item and a placeholder.
    let docs = home.join("docs");
    assert_eq!(names(&docs), ["note.md"]);
    let new_file = docs.join("new.txt");
    std::fs::write(&new_file, b"made on windows\n").unwrap();
    let out = provider.sync_path(&new_file).expect("upload new file");
    assert!(matches!(out, SyncOutcome::Created { .. }), "{out:?}");
    assert!(fake.state.lock().unwrap().find(&ids["docs"], "new.txt").is_some(), "server has new.txt");
    assert!(cf::placeholder_state(&new_file).is_some(), "new.txt is a placeholder now");
    log("STEP6 new file ok");

    // (7) conflict: the server copy moved on; our stale edit becomes the conflict copy.
    fake.state.lock().unwrap().modify(&ids["hello"], b"edited on the server\n");
    std::fs::write(&hello, b"stale local edit\n").unwrap();
    let out = provider.sync_path(&hello).expect("upload conflicting edit");
    let SyncOutcome::Conflict { conflict_name } = out else { panic!("expected a conflict, got {out:?}") };
    assert_eq!(conflict_name, "hello (conflict Test PC 2026-11-15 1042).txt");
    assert_eq!(std::fs::read(home.join(&conflict_name)).unwrap(), b"stale local edit\n", "our bytes kept");
    assert_eq!(std::fs::read(&hello).unwrap(), b"edited on the server\n", "server bytes under the old name");
    log("STEP7 keep-both conflict mirrored");

    // (8) change feed: server-side create, rename and trash show up locally.
    let created = fake.state.lock().unwrap().add("rt_home", Some(&ids["docs"]), "from-server.txt", false, b"hi\n");
    fake.state.lock().unwrap().rename(&ids["note"], "renamed.md");
    provider.poll_once("rt_home").expect("poll changes");
    let listed = names(&docs);
    assert!(listed.contains(&"from-server.txt".to_string()) && listed.contains(&"renamed.md".to_string()), "{listed:?}");
    assert!(!listed.contains(&"note.md".to_string()), "{listed:?}");
    fake.state.lock().unwrap().trash(&created);
    provider.poll_once("rt_home").expect("poll changes");
    assert!(!names(&docs).contains(&"from-server.txt".to_string()));
    log("STEP8 change feed applied");

    // (9) asleep: browsing never wakes; "Not now" never wakes; "Wake" wakes once.
    provider.free_up(&home.join("big.bin")).expect("dehydrate");
    {
        let mut st = fake.state.lock().unwrap();
        st.asleep = true;
        let code = st.roots[1].item.clone();
        st.add("rt_code", Some(&code), "late.md", false, b"late\n");
    }
    assert_eq!(names(&root.join("Code")), ["late.md", "readme.md"], "listing while asleep");
    assert_eq!(fake.state.lock().unwrap().consent_calls, 0, "browsing never wakes");
    assert!(std::fs::read(home.join("big.bin")).is_err(), "declined open fails");
    assert_eq!(fake.state.lock().unwrap().consent_calls, 0, "Not now never wakes");
    assert!(wake.prompts.load(Ordering::SeqCst) >= 1, "the user was asked");
    *answer.lock().unwrap() = WakeAnswer::Wake;
    assert_eq!(std::fs::read(home.join("big.bin")).unwrap(), big, "open after consent");
    let st = fake.state.lock().unwrap();
    assert_eq!(st.consent_calls, 1, "one consent, one wake");
    assert!(st.ticketed_requests > 0, "reads carried the wake ticket");
    drop(st);
    log("STEP9 ask-before-wake ok");

    // (10) unregister.
    conn.disconnect().expect("disconnect");
    sync_root::unregister(&root, &state_dir).expect("unregister");
    assert!(!sync_root::is_registered(&root), "sync root gone");
    log("STEP10 unregistered");
    let _ = std::fs::remove_dir_all(&base);
}
