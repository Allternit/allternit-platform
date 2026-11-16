//! Step 17: guest drops queued while the computer slept are pulled into
//! Inbox on wake. Real engine, real Inbox tasks; only cloud-api is faked
//! (the network is the edge).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use allternit_files_core::*;

use super::cloud::CloudCalls;
use super::drops::{self, safe_name, LIST_PATH};
use super::inbox::{self, TaskState};
use super::{Config, FilesV2, Inner};
use crate::db::DbHandle;

/// cloud-api's drop routes, faked: queued drops with their bytes, the acks it
/// received, and an optional status for the next acks.
#[derive(Default)]
struct Cloud {
    drops: Mutex<Vec<QueuedDrop>>,
    bytes: Mutex<HashMap<String, Vec<u8>>>,
    acks: Mutex<Vec<(String, QueuedDropAck)>>,
    ack_status: Mutex<Option<u16>>,
}

#[async_trait::async_trait]
impl CloudCalls for Cloud {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
        if method == "GET" && path == LIST_PATH {
            let queued: Vec<QueuedDrop> = self.drops.lock().unwrap().iter().filter(|d| d.state == QueuedDropState::Queued).cloned().collect();
            return Ok((200, serde_json::to_vec(&QueuedDropsResponse { drops: queued }).unwrap()));
        }
        let rest = path.strip_prefix(&format!("{LIST_PATH}/")).expect("a drop route");
        let (id, what) = rest.split_once('/').unwrap();
        match (method, what) {
            ("GET", "content") => Ok(self.bytes.lock().unwrap().get(id).map(|b| (200, b.clone())).unwrap_or((404, Vec::new()))),
            ("POST", "ack") => {
                let ack: QueuedDropAck = serde_json::from_slice(&body).unwrap();
                self.acks.lock().unwrap().push((id.to_string(), ack.clone()));
                if let Some(s) = *self.ack_status.lock().unwrap() {
                    return Ok((s, Vec::new()));
                }
                for d in self.drops.lock().unwrap().iter_mut().filter(|d| d.drop_id == id) {
                    d.state = match ack.state {
                        QueuedDropAckState::Delivered => QueuedDropState::Delivered,
                        QueuedDropAckState::Rejected => QueuedDropState::Rejected,
                    };
                }
                Ok((204, Vec::new()))
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

impl Cloud {
    fn queue(&self, id: &str, name: &str, bytes: &[u8], hash: Option<&str>) {
        self.drops.lock().unwrap().push(QueuedDrop {
            drop_id: id.into(),
            share_id: "sh_upload_only".into(),
            guest_id: "gst_dana".into(),
            name: name.into(),
            size: bytes.len() as u64,
            content_hash: hash.map(str::to_string).unwrap_or_else(|| allternit_files_engine::chunker::hash_bytes(bytes)),
            r2_key: format!("files-drops/rd_drops/{id}"),
            received_at: "2026-11-15T03:00:00Z".into(),
            state: QueuedDropState::Queued,
        });
        self.bytes.lock().unwrap().insert(id.into(), bytes.to_vec());
    }
}

struct T {
    _tmp: tempfile::TempDir,
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
        computer_id: "rd_drops".into(),
        memory_db: Some(DbHandle::new_memory().unwrap()),
        models_dir: None,
        code_dir: None,
        watch: false,
        clock: None,
    };
    let files = Arc::new(FilesV2::new(cfg));
    let inner = files.inner().await.unwrap();
    T { _tmp: tmp, files, inner }
}

impl T {
    fn inbox_names(&self) -> Vec<String> {
        let inbox = self.inner.drive.roots().unwrap().into_iter().find(|r| r.kind == RootKind::Inbox).unwrap();
        let mut v: Vec<String> = self.inner.drive.list(&inbox.root_item_id).unwrap().into_iter().map(|i| i.name).collect();
        v.sort();
        v
    }

    fn tasks(&self) -> Vec<inbox::Task> {
        inbox::list(&self.inner.meta.lock().unwrap(), None, 100).unwrap()
    }
}

#[tokio::test]
async fn a_drop_kept_while_asleep_lands_in_inbox_as_an_untrusted_task_from_the_guest() {
    let t = setup().await;
    let cloud = Cloud::default();
    cloud.queue("drp_1", "W-9.pdf", b"%PDF-1.7 tax form", None);

    assert_eq!(drops::pull(&t.files, &cloud).await.unwrap(), 1);
    assert_eq!(t.inbox_names(), ["W-9.pdf"]);
    let tasks = t.tasks();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.state, TaskState::NeedsApproval, "nothing reads a drop before the owner approves");
    assert!(task.untrusted);
    assert_eq!((task.dropped_by.kind, task.dropped_by.id.as_str()), (PrincipalKind::Guest, "gst_dana"));

    let acks = cloud.acks.lock().unwrap().clone();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].1.state, QueuedDropAckState::Delivered);
    assert_eq!(acks[0].1.item_id.as_deref(), Some(task.item_id.as_str()));

    // Acked drops are not listed again: a second pull writes nothing.
    assert_eq!(drops::pull(&t.files, &cloud).await.unwrap(), 0);
    assert_eq!(t.inbox_names(), ["W-9.pdf"]);
}

#[tokio::test]
async fn a_drop_whose_bytes_do_not_match_is_rejected_and_never_written() {
    let t = setup().await;
    let cloud = Cloud::default();
    cloud.queue("drp_bad", "lease.pdf", b"swapped in R2", Some(&"ab".repeat(32)));

    assert_eq!(drops::pull(&t.files, &cloud).await.unwrap(), 0);
    assert!(t.inbox_names().is_empty());
    assert!(t.tasks().is_empty());
    let acks = cloud.acks.lock().unwrap().clone();
    assert_eq!((acks[0].0.as_str(), acks[0].1.state), ("drp_bad", QueuedDropAckState::Rejected));
}

#[tokio::test]
async fn a_lost_ack_is_sent_again_without_a_second_copy() {
    let t = setup().await;
    let cloud = Cloud::default();
    cloud.queue("drp_2", "notes.txt", b"meeting notes", None);
    *cloud.ack_status.lock().unwrap() = Some(500);
    drops::pull(&t.files, &cloud).await.unwrap();
    assert_eq!(t.inbox_names(), ["notes.txt"]);

    *cloud.ack_status.lock().unwrap() = None;
    drops::pull(&t.files, &cloud).await.unwrap();
    assert_eq!(t.inbox_names(), ["notes.txt"], "the retry must not write `notes (2).txt`");
    assert_eq!(t.tasks().len(), 1);
    let acks = cloud.acks.lock().unwrap().clone();
    assert_eq!(acks.len(), 2);
    assert_eq!(acks[0].1.item_id, acks[1].1.item_id, "the same item is acked both times");
}

#[test]
fn a_guest_name_cannot_hide_a_drop_or_pose_as_a_provenance_sidecar() {
    assert_eq!(safe_name(".env"), "env");
    assert_eq!(safe_name("..."), "Dropped file");
    assert_eq!(safe_name("  W-9.pdf "), "W-9.pdf");
    assert_eq!(safe_name("x summary.md.provenance.json"), "x summary.md.provenance.json.dropped");
}
