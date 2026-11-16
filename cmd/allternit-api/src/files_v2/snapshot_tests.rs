//! The asleep snapshot writer (SPEC section 11, plan step 13 runtime side).
//! Real engine and temp folders; only cloud-api is a recorder (the network
//! is the edge).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use allternit_files_core::*;
use allternit_files_engine::EditingDevice;

use super::cloud::CloudCalls;
use super::snapshot::{self, Written, MANIFEST_PATH, ROOT_PATH_PREFIX};
use super::{Config, FilesV2, Inner};
use crate::db::DbHandle;

/// cloud-api, recorded: every PUT path and body; paths in `fail` answer 503 once.
#[derive(Default)]
struct Cloud {
    sent: Mutex<Vec<(String, Vec<u8>)>>,
    fail_once: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl CloudCalls for Cloud {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
        assert_eq!(method, "PUT");
        let mut fail = self.fail_once.lock().unwrap();
        if let Some(i) = fail.iter().position(|p| p == path) {
            fail.remove(i);
            return Ok((503, b"{}".to_vec()));
        }
        self.sent.lock().unwrap().push((path.to_string(), body));
        Ok((204, Vec::new()))
    }
}

impl Cloud {
    fn take(&self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

struct T {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
    files: Arc<FilesV2>,
    inner: Arc<Inner>,
}

async fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    let cfg = Config {
        enabled: true,
        data_dir: base.join("data"),
        home: Some(base.join("home")),
        computer_id: "rd_snap".into(),
        memory_db: Some(DbHandle::new_memory().unwrap()),
        models_dir: None,
        code_dir: None,
        watch: false,
        clock: None,
    };
    let files = Arc::new(FilesV2::new(cfg));
    let inner = files.inner().await.unwrap();
    T { home: base.join("home"), _tmp: tmp, files, inner }
}

impl T {
    fn root(&self, kind: RootKind) -> allternit_files_engine::RootRecord {
        self.inner.drive.roots().unwrap().into_iter().find(|r| r.kind == kind).unwrap()
    }

    fn put(&self, kind: RootKind, name: &str) {
        let root = self.root(kind);
        let owner = PrincipalRef { kind: PrincipalKind::Owner, id: "u1".into(), display_name: None };
        self.inner
            .drive
            .write_small(&root.root_id, &root.root_item_id, name, b"hello", None, OnConflict::KeepBoth, &owner, &EditingDevice::default())
            .unwrap();
    }
}

fn paths(sent: &[(String, Vec<u8>)]) -> Vec<String> {
    sent.iter().map(|(p, _)| p.clone()).collect()
}

#[tokio::test]
async fn the_snapshot_never_holds_a_protected_root_or_a_local_path() {
    let t = setup().await;
    let memory = t.root(RootKind::MemoryDrive);
    assert!(memory.policy.protected, "Memory Drive is protected by default");
    std::fs::write(memory.local_path.join("secret-plan.md"), "x").unwrap();
    t.inner.drive.scan_root(&memory.root_id).unwrap();
    t.put(RootKind::Inbox, "Lease.pdf");

    let cloud = Cloud::default();
    let n = snapshot::pass(&t.files, &cloud, &Mutex::new(Written::default())).await.unwrap();
    let sent = cloud.take();
    assert_eq!(n, sent.len());
    assert_eq!(sent[0].0, MANIFEST_PATH, "the manifest goes first: cloud-api refuses root files it does not list");

    let manifest: SnapshotManifest = serde_json::from_slice(&sent[0].1).unwrap();
    assert_eq!(manifest.computer_id, "rd_snap");
    assert_eq!((manifest.encryption, manifest.names_encrypted), (SnapshotEncryption::None, false), "plain names (Eoj 2026-11-15)");
    let roots = manifest.roots.unwrap();
    assert!(roots.iter().all(|r| !r.policy.protected && r.path_hint.is_none()));
    assert!(!roots.iter().any(|r| r.id == memory.root_id));
    assert!(!paths(&sent).contains(&format!("{ROOT_PATH_PREFIX}{}", memory.root_id)));

    let home = t.home.to_string_lossy().to_string();
    for (path, body) in &sent {
        let text = String::from_utf8_lossy(body);
        assert!(!text.contains(&home), "{path} carries a local path");
        assert!(!text.contains("secret-plan.md"), "{path} carries a Memory Drive name");
    }
    let inbox = t.root(RootKind::Inbox);
    let (_, body) = sent.iter().find(|(p, _)| *p == format!("{ROOT_PATH_PREFIX}{}", inbox.root_id)).unwrap();
    let file: SnapshotRootFile = serde_json::from_slice(body).unwrap();
    assert_eq!(file.cursor, t.inner.drive.current_cursor(&inbox.root_id).unwrap());
    assert!(file.items.unwrap().iter().any(|i| i.name == "Lease.pdf" && i.parent_id.as_deref() == Some(inbox.root_item_id.as_str())));
}

#[tokio::test]
async fn an_unchanged_drive_sends_nothing_and_a_change_sends_only_its_root() {
    let t = setup().await;
    let cloud = Cloud::default();
    let written = Mutex::new(Written::default());
    snapshot::pass(&t.files, &cloud, &written).await.unwrap();
    assert!(cloud.take().len() > 1);

    assert_eq!(snapshot::pass(&t.files, &cloud, &written).await.unwrap(), 0, "nothing changed: no upload");
    assert!(cloud.take().is_empty());

    t.put(RootKind::Inbox, "note.txt");
    snapshot::pass(&t.files, &cloud, &written).await.unwrap();
    let inbox = t.root(RootKind::Inbox);
    assert_eq!(paths(&cloud.take()), vec![MANIFEST_PATH.to_string(), format!("{ROOT_PATH_PREFIX}{}", inbox.root_id)]);
}

#[tokio::test]
async fn a_root_cloud_api_did_not_store_is_sent_again_next_pass() {
    let t = setup().await;
    let inbox = format!("{ROOT_PATH_PREFIX}{}", t.root(RootKind::Inbox).root_id);
    let cloud = Cloud { fail_once: Mutex::new(vec![inbox.clone()]), ..Default::default() };
    let written = Mutex::new(Written::default());
    snapshot::pass(&t.files, &cloud, &written).await.unwrap();
    assert!(!paths(&cloud.take()).contains(&inbox));

    snapshot::pass(&t.files, &cloud, &written).await.unwrap();
    assert!(paths(&cloud.take()).contains(&inbox), "the missed root goes out on the next pass");
}

#[test]
fn a_removed_root_rewrites_the_manifest() {
    let last = BTreeMap::from([("rt_a".to_string(), "c1".to_string()), ("rt_b".to_string(), "c1".to_string())]);
    let now = BTreeMap::from([("rt_a".to_string(), "c1".to_string())]);
    assert_eq!(snapshot::plan(&last, &now), (true, vec![]));
    assert_eq!(snapshot::plan(&now, &now), (false, vec![]));
}
