//! Tests for the tricky parts of the engine, through its public API.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use allternit_files_core::{
    ChangeOp, OnConflict, PrincipalKind, PrincipalRef, RootKind, RootPolicy, UploadCreateRequest, VersionPolicy,
};
use allternit_files_engine::chunker::{self, chunk_bytes, chunk_reader};
use allternit_files_engine::crypto::{ChunkSealer, EncryptedCache, FilesRootKey};
use allternit_files_engine::naming::{conflict_copy_name, duplicate_name};
use allternit_files_engine::{Drive, DriveOptions, DriveWatcher, EditingDevice, FilesError, RootSpec};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone, Utc};

fn noise(seed: &str, len: usize) -> Vec<u8> {
    let mut h = blake3::Hasher::new();
    h.update(seed.as_bytes());
    let mut out = vec![0u8; len];
    h.finalize_xof().fill(&mut out);
    out
}

fn dev() -> EditingDevice {
    EditingDevice::default()
}

fn owner() -> PrincipalRef {
    PrincipalRef { kind: PrincipalKind::Owner, id: "usr_test".into(), display_name: None }
}

fn policy(keep_count: u64, keep_days: u64, trash_days: u64) -> RootPolicy {
    RootPolicy {
        read: true,
        write: true,
        shareable: false,
        agent_process: false,
        protected: false,
        offline: true,
        replica: false,
        versions: VersionPolicy { keep_count, keep_days },
        trash_days,
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    data: std::path::PathBuf,
    home: std::path::PathBuf,
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Env { data, home, now: Arc::new(Mutex::new(Utc.with_ymd_and_hms(2026, 11, 15, 10, 42, 0).unwrap())), _tmp: tmp }
    }

    fn opts(&self) -> DriveOptions {
        let now = self.now.clone();
        DriveOptions {
            clock: Arc::new(move || *now.lock().unwrap()),
            display_offset: FixedOffset::east_opt(0).unwrap(),
            ..DriveOptions::default()
        }
    }

    fn drive(&self) -> Drive {
        Drive::open(&self.data, self.opts()).unwrap()
    }

    fn advance(&self, d: Duration) {
        *self.now.lock().unwrap() += d;
    }
}

fn add_root(drive: &Drive, path: &Path, p: RootPolicy) -> (String, String) {
    let r = drive
        .add_root(RootSpec { kind: RootKind::UserAdded, name: "Docs".into(), local_path: path.to_path_buf(), policy: p })
        .unwrap();
    (r.root_id, r.root_item_id)
}

#[test]
fn chunk_boundaries_resync_after_an_insert() {
    let original = noise("cdc", 24 << 20);
    let mut edited = original.clone();
    let at = 7 << 20;
    edited.splice(at..at, noise("insert", 1000));

    let a = chunk_bytes(&original);
    let b = chunk_bytes(&edited);
    for (i, c) in a.chunks.iter().enumerate() {
        assert!(c.size <= chunker::MAX_BYTES as u64);
        if i + 1 < a.chunks.len() {
            assert!(c.size >= chunker::MIN_BYTES as u64, "chunk {i} too small: {}", c.size);
        }
    }
    // Chunks wholly before the insert are identical.
    let before: Vec<_> = a.chunks.iter().take_while(|c| c.offset + c.size <= at as u64).collect();
    assert!(!before.is_empty());
    for (x, y) in before.iter().zip(&b.chunks) {
        assert_eq!(x.hash, y.hash);
    }
    // Content-defined: only the chunks around the insert change.
    let old: std::collections::HashSet<_> = a.chunks.iter().map(|c| &c.hash).collect();
    let changed = b.chunks.iter().filter(|c| !old.contains(&c.hash)).count();
    assert!(changed <= 2, "{changed} chunks changed after a 1000-byte insert ({} total)", b.chunks.len());
    // The streaming chunker finds the same boundaries as the in-memory one.
    let streamed = chunk_reader(std::io::Cursor::new(&edited), |_| Ok(())).unwrap();
    assert_eq!(streamed, b);
}

#[test]
fn identical_content_is_stored_once_and_freed_when_unreferenced() {
    let env = Env::new();
    let drive = env.drive();
    let (root, top) = add_root(&drive, &env.home.join("Docs"), policy(5, 0, 30));
    let content = noise("dedup", 3 << 20);

    let a = drive.write_small(&root, &top, "a.bin", &content, None, OnConflict::Fail, &owner(), &dev()).unwrap();
    let manifest = drive.manifest(&a.item.id, None).unwrap();
    assert!(manifest.chunks.len() >= 2);
    let hashes: Vec<String> = manifest.chunks.iter().map(|c| c.hash.clone()).collect();

    // Second copy by upload session: have/need says nothing to send.
    let hn = drive.have_need(&hashes).unwrap();
    assert!(hn.need.is_empty());
    let s = drive
        .create_upload(
            UploadCreateRequest {
                root_id: root.clone(),
                parent_id: top.clone(),
                name: "b.bin".into(),
                item_id: None,
                base_rev: None,
                size: manifest.size,
                content_hash: manifest.content_hash.clone(),
                chunker: chunker::params(),
                chunks: manifest.chunks.clone(),
                total_chunks: manifest.chunks.len() as u64,
                on_conflict: OnConflict::Fail,
                modified_at: None,
                device_name: None,
            },
            &owner(),
            None,
        )
        .unwrap();
    assert!(s.need.is_empty());
    let b = drive.commit_upload(&s.session_id).unwrap();
    assert_eq!(fs::read(env.home.join("Docs/b.bin")).unwrap(), content);
    for h in &hashes {
        assert_eq!(drive.chunk_refcount(h).unwrap(), 2);
    }

    drive.delete(&a.item.id, Some(&a.item.rev()), true, &owner()).unwrap();
    assert_eq!(drive.gc().unwrap(), 0);
    for h in &hashes {
        assert_eq!(drive.chunk_refcount(h).unwrap(), 1);
        assert!(drive.store().has(h));
    }
    drive.delete(&b.item.id, Some(&b.item.rev()), true, &owner()).unwrap();
    assert_eq!(drive.gc().unwrap(), hashes.len());
    for h in &hashes {
        assert!(!drive.store().has(h));
    }
}

#[test]
fn rename_and_move_keep_the_item_id() {
    let env = Env::new();
    let drive = env.drive();
    let dir = env.home.join("Docs");
    let (root, top) = add_root(&drive, &dir, policy(0, 0, 30));
    let f = drive.write_small(&root, &top, "draft.txt", b"hello", None, OnConflict::Fail, &owner(), &dev()).unwrap().item;
    let sub = drive.mkdir(&top, "Archive", OnConflict::Fail, &owner()).unwrap().item;

    let moved = drive.move_item(&f.id, &f.rev(), Some(&sub.id), Some("final.txt"), OnConflict::Fail, &owner()).unwrap().item;
    assert_eq!(moved.id, f.id);
    assert_eq!(moved.parent_id.as_deref(), Some(sub.id.as_str()));
    assert_ne!(moved.rev(), f.rev(), "rename bumps meta_version so the rev changes");
    assert_eq!(fs::read(dir.join("Archive/final.txt")).unwrap(), b"hello");
    // A stale base_rev on move is always rev_mismatch.
    let err = drive.move_item(&f.id, &f.rev(), None, Some("x.txt"), OnConflict::KeepBoth, &owner()).unwrap_err();
    assert!(matches!(err, FilesError::RevMismatch { .. }));

    // Renamed and moved outside the engine (Finder, a shell): the watcher's reconcile
    // still keeps the ID.
    fs::rename(dir.join("Archive/final.txt"), dir.join("renamed.txt")).unwrap();
    drive.reconcile_paths(&[dir.join("Archive/final.txt"), dir.join("renamed.txt")]).unwrap();
    let now = drive.stat(&f.id).unwrap();
    assert_eq!(now.name, "renamed.txt");
    assert_eq!(now.parent_id.as_deref(), Some(top.as_str()));
    assert_eq!(drive.list(&sub.id).unwrap().len(), 0);
}

#[test]
fn conflict_copy_and_duplicate_names_follow_the_spec() {
    let when = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap().and_hms_opt(10, 42, 0).unwrap();
    let none = |_: &str| false;
    assert_eq!(conflict_copy_name("Lease.pdf", "iPhone", when, none), "Lease (conflict iPhone 2026-11-15 1042).pdf");
    let taken = |n: &str| n == "Lease (conflict iPhone 2026-11-15 1042).pdf" || n == "Lease (conflict iPhone 2026-11-15 1042 2).pdf";
    assert_eq!(conflict_copy_name("Lease.pdf", "iPhone", when, taken), "Lease (conflict iPhone 2026-11-15 1042 3).pdf");
    assert_eq!(conflict_copy_name("Notes", "Mac", when, none), "Notes (conflict Mac 2026-11-15 1042)");
    assert_eq!(conflict_copy_name(".env", "Mac", when, none), ".env (conflict Mac 2026-11-15 1042)");
    assert_eq!(conflict_copy_name("a.tar.gz", "Joe's/PC", when, none), "a.tar (conflict Joe's-PC 2026-11-15 1042).gz");
    assert_eq!(duplicate_name("Lease.pdf", |n| n == "Lease (2).pdf"), "Lease (3).pdf");
    let long = format!("{}.pdf", "é".repeat(126));
    let n = conflict_copy_name(&long, "iPhone", when, none);
    assert!(n.len() <= 255 && n.ends_with("1042).pdf"), "{n}");

    // End to end: a stale write keeps both and reports the conflict.
    let env = Env::new();
    let drive = env.drive();
    let dir = env.home.join("Docs");
    let (root, top) = add_root(&drive, &dir, policy(3, 0, 30));
    let v1 = drive.write_small(&root, &top, "Lease.pdf", b"v1", None, OnConflict::Fail, &owner(), &dev()).unwrap().item;
    let v2 = drive.write_small(&root, &top, "Lease.pdf", b"v2", Some(&v1.rev()), OnConflict::KeepBoth, &owner(), &dev()).unwrap();
    assert!(v2.conflict.is_none());
    let stale = drive
        .write_small(&root, &top, "Lease.pdf", b"phone edit", Some(&v1.rev()), OnConflict::KeepBoth, &owner(), &EditingDevice { name: Some("iPhone".into()), utc_offset_minutes: Some(-360) })
        .unwrap();
    let c = stale.conflict.expect("conflict info");
    // 10:42 UTC is 04:42 on a phone at UTC-6: the name uses the device's local time.
    assert_eq!(c.conflict_name, "Lease (conflict iPhone 2026-11-15 0442).pdf");
    assert_eq!(c.original_item_id, v1.id);
    assert_eq!(c.current_rev, v2.item.rev());
    assert_eq!(fs::read(dir.join("Lease.pdf")).unwrap(), b"v2");
    assert_eq!(fs::read(dir.join(&c.conflict_name)).unwrap(), b"phone edit");
    for mode in [OnConflict::Fail, OnConflict::Replace] {
        let err = drive.write_small(&root, &top, "Lease.pdf", b"x", Some(&v1.rev()), mode, &owner(), &dev()).unwrap_err();
        match err {
            FilesError::RevMismatch { current } => assert_eq!(current.rev(), v2.item.rev()),
            other => panic!("{other:?}"),
        }
    }
    // A create into a taken name: keep_both is "(2)" with no conflict; fail refuses.
    let dup = drive.write_small(&root, &top, "Lease.pdf", b"new", None, OnConflict::KeepBoth, &owner(), &dev()).unwrap();
    assert_eq!(dup.item.name, "Lease (2).pdf");
    assert!(dup.conflict.is_none());
    let err = drive.write_small(&root, &top, "Lease.pdf", b"new", None, OnConflict::Fail, &owner(), &dev()).unwrap_err();
    assert!(matches!(err, FilesError::AlreadyExists(_)));
}

#[test]
fn change_cursor_is_monotonic_across_restart() {
    let env = Env::new();
    let dir = env.home.join("Docs");
    let (root, top, cursor_a, item);
    {
        let drive = env.drive();
        (root, top) = add_root(&drive, &dir, policy(0, 0, 30));
        cursor_a = drive.current_cursor(&root).unwrap();
        item = drive.write_small(&root, &top, "one.txt", b"1", None, OnConflict::Fail, &owner(), &dev()).unwrap().item;
    }
    let drive = env.drive();
    let page = drive.changes(&root, Some(&cursor_a), 100).unwrap();
    assert!(!page.reset_required);
    assert_eq!(page.changes.len(), 1);
    assert_eq!(page.changes[0].op, ChangeOp::Created);
    let cursor_b = page.cursor.clone();
    let first_seq = page.changes[0].seq;

    drive.write_small(&root, &top, "one.txt", b"2", Some(&item.rev()), OnConflict::Fail, &owner(), &dev()).unwrap();
    let page = drive.changes(&root, Some(&cursor_b), 100).unwrap();
    assert_eq!(page.changes.len(), 1);
    assert!(page.changes[0].seq > first_seq);
    assert_eq!(page.changes[0].op, ChangeOp::Modified);
    assert!(drive.changes(&root, Some(&page.cursor), 100).unwrap().changes.is_empty());

    // Another root's cursor or garbage is 410; a rebuilt index asks for a reset.
    let (other, _) = drive
        .add_root(RootSpec { kind: RootKind::Shared, name: "Shared".into(), local_path: env.home.join("Shared"), policy: policy(0, 0, 30) })
        .map(|r| (r.root_id, r.root_item_id))
        .unwrap();
    assert!(matches!(drive.changes(&other, Some(&cursor_b), 10), Err(FilesError::CursorExpired)));
    assert!(matches!(drive.changes(&root, Some("c1.nope"), 10), Err(FilesError::CursorExpired)));
    drive.new_epoch(&root).unwrap();
    assert!(drive.changes(&root, Some(&cursor_b), 10).unwrap().reset_required);
}

#[test]
fn versions_and_trash_obey_count_and_age_limits() {
    let env = Env::new();
    let drive = env.drive();
    let dir = env.home.join("Docs");
    let (root, top) = add_root(&drive, &dir, policy(2, 3, 7));
    let mut item = drive.write_small(&root, &top, "notes.txt", b"v1", None, OnConflict::Fail, &owner(), &dev()).unwrap().item;
    for v in 2..=5 {
        env.advance(Duration::hours(1));
        item = drive
            .write_small(&root, &top, "notes.txt", format!("v{v}").as_bytes(), Some(&item.rev()), OnConflict::Fail, &owner(), &dev())
            .unwrap()
            .item;
    }
    let versions = drive.versions(&item.id).unwrap();
    assert_eq!(versions.len(), 2, "keep_count = 2");
    assert_eq!(versions[0].content_hash, chunker::hash_bytes(b"v4"));
    assert_eq!(versions[1].content_hash, chunker::hash_bytes(b"v3"));

    // Restoring a version makes v4 current; v5 becomes a version.
    let restored = drive.restore_version(&item.id, &versions[0].version_id, &item.rev(), &owner()).unwrap().item;
    assert_eq!(fs::read(dir.join("notes.txt")).unwrap(), b"v4");
    assert_eq!(drive.versions(&item.id).unwrap()[0].content_hash, chunker::hash_bytes(b"v5"));

    env.advance(Duration::days(4));
    let report = drive.prune().unwrap();
    assert_eq!(report.versions_dropped, 2, "older than keep_days = 3");
    assert!(drive.versions(&item.id).unwrap().is_empty());

    // Trash: a folder with a file comes back whole; after trash_days it is purged.
    let folder = drive.mkdir(&top, "Project", OnConflict::Fail, &owner()).unwrap().item;
    let inner = drive.write_small(&root, &folder.id, "plan.md", b"plan", None, OnConflict::Fail, &owner(), &dev()).unwrap().item;
    let (t, _) = drive.delete(&folder.id, Some(&folder.rev()), false, &owner()).unwrap();
    let t = t.expect("trashed");
    assert!(!dir.join("Project").exists());
    assert_eq!(drive.trash_list(&root).unwrap().len(), 1);
    drive.restore(&t.trash_id, OnConflict::Fail, &owner()).unwrap();
    assert_eq!(fs::read(dir.join("Project/plan.md")).unwrap(), b"plan");
    assert_eq!(drive.stat(&inner.id).unwrap().name, "plan.md", "IDs survive trash and restore");

    let restored_file = drive.stat(&restored.id).unwrap();
    drive.delete(&restored_file.id, Some(&restored_file.rev()), false, &owner()).unwrap();
    env.advance(Duration::days(6));
    assert_eq!(drive.prune().unwrap().trash_purged, 0);
    env.advance(Duration::days(2));
    let report = drive.prune().unwrap();
    assert_eq!(report.trash_purged, 1);
    assert!(drive.trash_list(&root).unwrap().is_empty());
    assert!(!drive.store().has(&chunker::hash_bytes(b"v4")), "purged content is garbage collected");
}

#[test]
fn sealed_cache_round_trips_and_tampering_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let frk = FilesRootKey::generate("fk_1");
    let cache = EncryptedCache::open(tmp.path().join("cache"), &frk, "dev_phone1").unwrap();
    let data = noise("cache", 300_000);
    let hash = cache.put(&data).unwrap();
    assert_eq!(cache.get(&hash).unwrap().unwrap(), data);
    // The cache folder never shows plaintext hashes.
    let name = fs::read_dir(tmp.path().join("cache")).unwrap().next().unwrap().unwrap().file_name();
    assert_ne!(name.to_string_lossy(), hash);

    // Another device's subkey can't find or open it.
    let other = EncryptedCache::open(tmp.path().join("cache"), &frk, "dev_laptop").unwrap();
    assert!(other.get(&hash).unwrap().is_none());

    // Flip one ciphertext byte.
    let path = tmp.path().join("cache").join(&name);
    let mut blob = fs::read(&path).unwrap();
    let last = blob.len() - 1;
    blob[last] ^= 1;
    fs::write(&path, &blob).unwrap();
    assert!(matches!(cache.get(&hash), Err(FilesError::IntegrityFailed(_))));

    // A header edit (claiming another size) fails authentication too.
    let sealer = ChunkSealer::for_replica(&frk, "rp_1");
    let (mut header, ct) = sealer.seal(b"replica chunk").unwrap();
    let h = chunker::hash_bytes(b"replica chunk");
    assert_eq!(sealer.open(&header, &ct, &h).unwrap(), b"replica chunk");
    assert!(matches!(sealer.open(&header, &ct, &chunker::hash_bytes(b"other")), Err(FilesError::IntegrityFailed(_))));
    header.plaintext_size += 1;
    assert!(matches!(sealer.open(&header, &ct, &h), Err(FilesError::IntegrityFailed(_))));
}

#[test]
fn upload_rejects_bad_chunks_and_reports_missing_ones() {
    let env = Env::new();
    let drive = env.drive();
    let (root, top) = add_root(&drive, &env.home.join("Docs"), policy(0, 0, 30));
    let content = noise("upload", 6 << 20);
    let c = chunk_bytes(&content);
    let s = drive
        .create_upload(
            UploadCreateRequest {
                root_id: root,
                parent_id: top,
                name: "big.bin".into(),
                item_id: None,
                base_rev: None,
                size: c.size,
                content_hash: c.content_hash.clone(),
                chunker: chunker::params(),
                chunks: c.chunks.clone(),
                total_chunks: c.chunks.len() as u64,
                on_conflict: OnConflict::Fail,
                modified_at: None,
                device_name: None,
            },
            &owner(),
            None,
        )
        .unwrap();
    let first = &c.chunks[0];
    let mut wrong = content[..first.size as usize].to_vec();
    wrong[0] ^= 0xff;
    assert!(matches!(drive.put_upload_chunk(&s.session_id, &first.hash, &wrong), Err(FilesError::ChunkHashMismatch(_))));
    drive.put_upload_chunk(&s.session_id, &first.hash, &content[..first.size as usize]).unwrap();
    match drive.commit_upload(&s.session_id) {
        Err(FilesError::ChunkMissing { missing }) => {
            assert_eq!(missing, c.chunks[1..].iter().map(|r| r.hash.clone()).collect::<Vec<_>>())
        }
        other => panic!("{other:?}"),
    }
    env.advance(Duration::hours(25));
    assert!(matches!(drive.upload_status(&s.session_id), Err(FilesError::UploadSessionExpired)));
}

#[test]
fn watcher_indexes_files_created_outside_the_engine() {
    let env = Env::new();
    let drive = Arc::new(Drive::open(&env.data, DriveOptions::default()).unwrap());
    let dir = env.home.join("Docs");
    let (_root, top) = add_root(&drive, &dir, policy(0, 0, 30));
    let _w = DriveWatcher::start(drive.clone(), |e| panic!("reconcile failed: {e}")).unwrap();
    std::thread::sleep(StdDuration::from_millis(200));
    fs::create_dir(dir.join("From Finder")).unwrap();
    fs::write(dir.join("From Finder/photo.jpg"), b"jpeg").unwrap();
    let deadline = std::time::Instant::now() + StdDuration::from_secs(15);
    loop {
        let found = drive
            .list(&top)
            .unwrap()
            .into_iter()
            .find(|i| i.name == "From Finder")
            .map(|f| drive.list(&f.id).unwrap().iter().any(|i| i.name == "photo.jpg" && i.size == 4));
        if found == Some(true) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "watcher never indexed the new folder");
        std::thread::sleep(StdDuration::from_millis(100));
    }
}
