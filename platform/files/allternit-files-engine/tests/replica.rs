//! Replica sealing, host store and restore (SPEC sections 10 and 13). Plan
//! section 6: "tampered chunk fails restore; replica unreadable without the key".

use std::collections::HashMap;

use allternit_files_core::{ChunkRef, ErrorCode, ItemKind};
use allternit_files_engine::chunker;
use allternit_files_engine::crypto::{ChunkSealer, FilesRootKey};
use allternit_files_engine::replica::{self, Catalog, CatalogEntry, HostStore, RestoreSink};
use allternit_files_engine::{FilesError, Result};

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// A source with two files sealed into a host store, the way the replicator does it.
struct World {
    _tmp: tempfile::TempDir,
    frk: FilesRootKey,
    host: HostStore,
    catalog: Catalog,
    files: HashMap<String, Vec<u8>>,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let frk = FilesRootKey::generate("fk_1");
    let rid = replica::replica_id("rd_cloud", "rt_home", "rd_mac");
    let sealer = ChunkSealer::for_replica(&frk, &rid);
    let host = HostStore::open(tmp.path().join("rp")).unwrap();
    let mut items = vec![CatalogEntry {
        item_id: "it_root".into(),
        parent_id: None,
        name: "Home".into(),
        kind: ItemKind::Folder,
        size: 0,
        content_hash: None,
        rev: "1-dir".into(),
        modified_at: "2026-11-15T10:00:00Z".into(),
        chunks: vec![],
    }];
    let mut files = HashMap::new();
    for (id, name, data) in [
        ("it_lease", "Lease agreement.pdf", noise(3 * 1024 * 1024 + 17, 7)),
        ("it_notes", "notes.txt", b"secret plans: the quarterly numbers".to_vec()),
    ] {
        let c = chunker::chunk_bytes(&data);
        for r in &c.chunks {
            let (loc, blob) = replica::seal_blob(&sealer, &data[r.offset as usize..(r.offset + r.size) as usize]).unwrap();
            host.put_blob(&loc, &blob, Some("fk_1")).unwrap();
        }
        items.push(CatalogEntry {
            item_id: id.into(),
            parent_id: Some("it_root".into()),
            name: name.into(),
            kind: ItemKind::File,
            size: c.size,
            content_hash: Some(c.content_hash.clone()),
            rev: format!("1-{}", c.content_hash),
            modified_at: "2026-11-15T10:00:00Z".into(),
            chunks: c.chunks.clone(),
        });
        files.insert(id.to_string(), data);
    }
    let catalog = Catalog {
        replica_id: rid.clone(),
        source_computer_id: "rd_cloud".into(),
        root_id: "rt_home".into(),
        root_item_id: "it_root".into(),
        cursor: Some("c1.rt_home.000003".into()),
        synced_at: "2026-11-15T10:01:00Z".into(),
        items,
    };
    let (head, parts) = replica::pack_catalog(&catalog).unwrap();
    for (r, pt) in &parts {
        assert_eq!(chunker::hash_bytes(pt), r.hash);
        let (loc, blob) = replica::seal_blob(&sealer, pt).unwrap();
        host.put_blob(&loc, &blob, Some("fk_1")).unwrap();
    }
    host.put_head(&replica::seal_head(&frk, &head).unwrap(), Some("fk_1")).unwrap();
    World { _tmp: tmp, frk, host, catalog, files }
}

#[derive(Default)]
struct Collect {
    open: HashMap<String, Vec<u8>>,
    done: HashMap<String, Vec<u8>>,
}

impl RestoreSink for Collect {
    fn folder(&mut self, _e: &CatalogEntry) -> Result<()> {
        Ok(())
    }
    fn begin(&mut self, e: &CatalogEntry) -> Result<()> {
        self.open.insert(e.item_id.clone(), Vec::new());
        Ok(())
    }
    fn chunk(&mut self, e: &CatalogEntry, _c: &ChunkRef, data: &[u8]) -> Result<()> {
        self.open.get_mut(&e.item_id).unwrap().extend_from_slice(data);
        Ok(())
    }
    fn finish(&mut self, e: &CatalogEntry) -> Result<()> {
        let data = self.open.remove(&e.item_id).unwrap();
        self.done.insert(e.item_id.clone(), data);
        Ok(())
    }
    fn abort(&mut self, e: &CatalogEntry) {
        self.open.remove(&e.item_id);
    }
}

fn from_host(host: &HostStore) -> impl FnMut(&str) -> Result<Option<Vec<u8>>> + '_ {
    move |loc| host.get_blob(loc)
}

#[test]
fn restore_from_an_intact_replica_reports_every_hash_verified() {
    let w = world();
    let head = replica::open_head(&w.frk, &w.catalog.replica_id, &w.host.get_head().unwrap().unwrap()).unwrap();
    let catalog = replica::read_catalog(&w.frk, &head, &mut from_host(&w.host)).unwrap();
    assert_eq!(catalog, w.catalog);
    let mut sink = Collect::default();
    let report = replica::restore(&w.frk, &catalog, None, &mut from_host(&w.host), &mut sink);
    assert!(report.verified, "{report:?}");
    assert_eq!((report.files_restored, report.chunks_failed), (2, 0));
    assert!(report.chunks_verified >= 3);
    assert_eq!(sink.done["it_lease"], w.files["it_lease"]);
    assert_eq!(sink.done["it_notes"], w.files["it_notes"]);
}

#[test]
fn a_tampered_chunk_fails_restore_and_the_report_names_its_file() {
    let w = world();
    let sealer = ChunkSealer::for_replica(&w.frk, &w.catalog.replica_id);
    let lease = w.catalog.entry("it_lease").unwrap();
    let loc = sealer.locator(&lease.chunks[1].hash).unwrap();
    // Flip one ciphertext byte on the host's disk.
    let mut blob = w.host.get_blob(&loc).unwrap().unwrap();
    let last = blob.len() - 1;
    blob[last] ^= 0x01;
    let path = w._tmp.path().join("rp/blobs").join(&loc[..2]).join(&loc);
    std::fs::write(&path, &blob).unwrap();

    let mut sink = Collect::default();
    let report = replica::restore(&w.frk, &w.catalog, None, &mut from_host(&w.host), &mut sink);
    assert!(!report.verified);
    assert_eq!(report.chunks_failed, 1);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].item_id, "it_lease");
    assert_eq!(report.failures[0].reason, ErrorCode::IntegrityFailed);
    assert!(!sink.done.contains_key("it_lease"), "a file with a bad chunk is never committed");
    assert_eq!(sink.done["it_notes"], w.files["it_notes"], "the good file still restores");
    let err = replica::read_file(&w.frk, &w.catalog, "it_lease", &mut from_host(&w.host)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::IntegrityFailed);
}

#[test]
fn a_blob_swapped_for_another_chunk_fails_restore() {
    let w = world();
    let sealer = ChunkSealer::for_replica(&w.frk, &w.catalog.replica_id);
    let lease = w.catalog.entry("it_lease").unwrap();
    let a = sealer.locator(&lease.chunks[0].hash).unwrap();
    let b = sealer.locator(&lease.chunks[1].hash).unwrap();
    // A valid sealed chunk, stored under the wrong name.
    let other = w.host.get_blob(&b).unwrap().unwrap();
    std::fs::write(w._tmp.path().join("rp/blobs").join(&a[..2]).join(&a), other).unwrap();
    let report = replica::restore(&w.frk, &w.catalog, Some(&["it_lease".to_string()]), &mut from_host(&w.host), &mut Collect::default());
    assert!(!report.verified);
    assert_eq!(report.failures[0].reason, ErrorCode::IntegrityFailed);
}

#[test]
fn the_host_holds_no_plaintext_and_no_plaintext_hash() {
    let w = world();
    let mut haystack = Vec::new();
    for entry in walk(w._tmp.path()) {
        haystack.extend(std::fs::read(&entry).unwrap());
        haystack.extend(entry.to_string_lossy().as_bytes());
    }
    let contains = |needle: &[u8]| haystack.windows(needle.len()).any(|win| win == needle);
    assert!(!contains(b"secret plans"), "file content leaked to the host");
    assert!(!contains(b"Lease agreement"), "a file name leaked to the host");
    for e in &w.catalog.items {
        for c in &e.chunks {
            assert!(!contains(c.hash.as_bytes()), "a plaintext chunk hash leaked to the host");
        }
    }
}

#[test]
fn the_replica_is_unreadable_without_the_key() {
    let w = world();
    let head_bytes = w.host.get_head().unwrap().unwrap();
    // Another user's key (different key_id): refused before decrypting.
    let stranger = FilesRootKey::generate("fk_other");
    assert_eq!(replica::open_head(&stranger, &w.catalog.replica_id, &head_bytes).unwrap_err().code(), ErrorCode::KeyUnavailable);
    // A forged key that claims the right key_id: authentication fails.
    let forged = FilesRootKey::from_bytes("fk_1", [9u8; 32]);
    assert_eq!(replica::open_head(&forged, &w.catalog.replica_id, &head_bytes).unwrap_err().code(), ErrorCode::IntegrityFailed);
    let forged_sealer = ChunkSealer::for_replica(&forged, &w.catalog.replica_id);
    let real_sealer = ChunkSealer::for_replica(&w.frk, &w.catalog.replica_id);
    let notes = w.catalog.entry("it_notes").unwrap();
    let blob = w.host.get_blob(&real_sealer.locator(&notes.chunks[0].hash).unwrap()).unwrap().unwrap();
    assert!(replica::open_blob(&forged_sealer, &blob, &notes.chunks[0].hash).is_err());
    // Even with the catalog in hand, a reader without the key gets nothing.
    let report = replica::restore(&forged, &w.catalog, None, &mut from_host(&w.host), &mut Collect::default());
    assert_eq!((report.files_restored, report.verified), (0, false));
}

#[test]
fn the_host_refuses_blobs_that_are_not_sealed_chunks() {
    let w = world();
    let sealer = ChunkSealer::for_replica(&w.frk, &w.catalog.replica_id);
    let (loc, blob) = replica::seal_blob(&sealer, b"hello").unwrap();
    let wrong_name = chunker::hash_bytes(b"elsewhere");
    assert!(matches!(w.host.put_blob(&wrong_name, &blob, None), Err(FilesError::InvalidRequest(_))));
    assert!(matches!(w.host.put_blob(&loc, b"plaintext pretending", None), Err(FilesError::InvalidRequest(_))));
    let mut truncated = blob.clone();
    truncated.pop();
    assert!(matches!(w.host.put_blob(&loc, &truncated, None), Err(FilesError::InvalidRequest(_))));
    let other_key = ChunkSealer::for_replica(&FilesRootKey::generate("fk_2"), &w.catalog.replica_id);
    let (loc2, blob2) = replica::seal_blob(&other_key, b"hello").unwrap();
    assert!(matches!(w.host.put_blob(&loc2, &blob2, Some("fk_1")), Err(FilesError::InvalidRequest(_))));
}

#[test]
fn an_envelope_opens_only_on_its_device() {
    let frk = FilesRootKey::generate("fk_1");
    let (shared, eph, dev) = ([3u8; 32], [4u8; 32], [5u8; 32]);
    let (nonce, wrapped) = replica::wrap_root_key(&frk, &shared, &eph, &dev, "rd_mac").unwrap();
    assert_eq!(base64_len(&wrapped), 48, "32-byte key + 16-byte tag; never the bare key");
    let back = replica::unwrap_root_key("fk_1", &shared, &eph, &dev, "rd_mac", &nonce, &wrapped).unwrap();
    assert_eq!(back.subkey("x"), frk.subkey("x"));
    // Another device id, another shared secret or another key id: refused.
    assert!(replica::unwrap_root_key("fk_1", &shared, &eph, &dev, "rd_phone", &nonce, &wrapped).is_err());
    assert!(replica::unwrap_root_key("fk_1", &[6u8; 32], &eph, &dev, "rd_mac", &nonce, &wrapped).is_err());
    assert!(replica::unwrap_root_key("fk_9", &shared, &eph, &dev, "rd_mac", &nonce, &wrapped).is_err());
}

#[test]
fn replica_ids_and_confirm_codes_are_stable() {
    let a = replica::replica_id("rd_cloud", "rt_home", "rd_mac");
    assert_eq!(a, replica::replica_id("rd_cloud", "rt_home", "rd_mac"));
    assert_ne!(a, replica::replica_id("rd_cloud", "rt_home", "rd_phone"));
    assert!(a.starts_with("rp_") && a.len() == 27);
    let code = replica::confirm_code(&[1u8; 32]);
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(code, replica::confirm_code(&[1u8; 32]));
}

fn base64_len(s: &str) -> usize {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap().len()
}

fn walk(p: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(p).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}
