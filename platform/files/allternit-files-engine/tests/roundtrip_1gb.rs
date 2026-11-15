//! 1 GiB upload round trip with a kill in the middle. Ignored by default (it writes
//! ~3 GiB to a temp dir). Run:
//!   cargo test -p allternit-files-engine --release --test roundtrip_1gb -- --ignored --nocapture

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::Instant;

use allternit_files_core::{
    OnConflict, PrincipalKind, PrincipalRef, RootKind, RootPolicy, UploadCreateRequest, VersionPolicy,
};
use allternit_files_engine::chunker::{self, chunk_reader};
use allternit_files_engine::{Drive, DriveOptions, RootSpec};

const SIZE: u64 = 1 << 30;

fn read_chunk(f: &mut fs::File, offset: u64, size: u64) -> Vec<u8> {
    f.seek(SeekFrom::Start(offset)).unwrap();
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf).unwrap();
    buf
}

#[test]
#[ignore]
fn one_gib_upload_resumes_after_kill_and_sends_only_missing_chunks() {
    let started = Instant::now();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("source.bin");
    let data_dir = tmp.path().join("runtime-data");
    let root_dir = tmp.path().join("home/Allternit/Downloads");

    // 1 GiB of incompressible bytes.
    {
        let mut h = blake3::Hasher::new();
        h.update(b"files-drive 1 GiB round trip");
        let mut xof = h.finalize_xof();
        let mut out = std::io::BufWriter::with_capacity(8 << 20, fs::File::create(&src).unwrap());
        let mut buf = vec![0u8; 8 << 20];
        for _ in 0..(SIZE / buf.len() as u64) {
            xof.fill(&mut buf);
            out.write_all(&buf).unwrap();
        }
        out.flush().unwrap();
    }

    // Client side: chunk the file.
    let t = Instant::now();
    let c = chunk_reader(std::io::BufReader::with_capacity(4 << 20, fs::File::open(&src).unwrap()), |_| Ok(())).unwrap();
    println!("chunked {} bytes into {} chunks in {:.1?}", c.size, c.chunks.len(), t.elapsed());
    assert_eq!(c.size, SIZE);
    let distinct: HashSet<&str> = c.chunks.iter().map(|r| r.hash.as_str()).collect();
    assert_eq!(distinct.len(), c.chunks.len(), "random data has no repeated chunks");

    let owner = PrincipalRef { kind: PrincipalKind::Owner, id: "usr_test".into(), display_name: None };
    let policy = RootPolicy {
        read: true,
        write: true,
        shareable: false,
        agent_process: false,
        protected: false,
        offline: true,
        replica: false,
        versions: VersionPolicy { keep_count: 0, keep_days: 0 },
        trash_days: 30,
    };

    let mut file = fs::File::open(&src).unwrap();
    let session_id;
    let sent_before_kill: HashSet<String>;
    let mut bytes_before_kill = 0u64;
    {
        let drive = Drive::open(&data_dir, DriveOptions::default()).unwrap();
        let root = drive
            .add_root(RootSpec { kind: RootKind::Downloads, name: "Downloads".into(), local_path: root_dir.clone(), policy: policy.clone() })
            .unwrap();
        let status = drive
            .create_upload(
                UploadCreateRequest {
                    root_id: root.root_id.clone(),
                    parent_id: root.root_item_id.clone(),
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
                    device_name: Some("Test Mac".into()),
                },
                &owner,
                Some(-360),
            )
            .unwrap();
        session_id = status.session_id.clone();
        assert_eq!(status.need.len(), c.chunks.len());

        let half = c.chunks.len() / 2;
        let mut sent = HashSet::new();
        for r in &c.chunks[..half] {
            let bytes = read_chunk(&mut file, r.offset, r.size);
            drive.put_upload_chunk(&session_id, &r.hash, &bytes).unwrap();
            bytes_before_kill += r.size;
            sent.insert(r.hash.clone());
        }
        sent_before_kill = sent;
        // A chunk was being written when the process died.
        fs::write(data_dir.join("chunks/tmp/partial.123"), &read_chunk(&mut file, c.chunks[half].offset, 1000)).unwrap();
        println!("sent {} chunks ({} bytes), then killed the runtime", sent_before_kill.len(), bytes_before_kill);
        // Dropping the drive drops all in-process state (index connection, caches).
    }

    // Restarted runtime: resume asks only for what never arrived.
    let drive = Drive::open(&data_dir, DriveOptions::default()).unwrap();
    assert!(!data_dir.join("chunks/tmp/partial.123").exists(), "partial chunk discarded on restart");
    let status = drive.upload_status(&session_id).unwrap();
    let need: HashSet<String> = status.need.iter().cloned().collect();
    let expected: HashSet<String> = c.chunks.iter().map(|r| r.hash.clone()).filter(|h| !sent_before_kill.contains(h)).collect();
    assert_eq!(need, expected, "resume needs exactly the chunks not sent before the kill");
    assert_eq!(status.received_bytes, bytes_before_kill);

    let mut bytes_after = 0u64;
    let mut puts_after = 0usize;
    for r in c.chunks.iter().filter(|r| need.contains(&r.hash)) {
        let bytes = read_chunk(&mut file, r.offset, r.size);
        drive.put_upload_chunk(&session_id, &r.hash, &bytes).unwrap();
        bytes_after += r.size;
        puts_after += 1;
    }
    println!("resumed: sent {puts_after} chunks ({bytes_after} bytes)");
    assert_eq!(bytes_before_kill + bytes_after, SIZE, "no byte sent twice");
    assert!(drive.upload_status(&session_id).unwrap().need.is_empty());

    let m = drive.commit_upload(&session_id).unwrap();
    assert_eq!(m.item.size, SIZE);
    assert_eq!(m.item.content_hash.as_deref(), Some(c.content_hash.as_str()));

    // Final bytes on disk hash to the source hash.
    let mut h = blake3::Hasher::new();
    let mut f = fs::File::open(root_dir.join("big.bin")).unwrap();
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let final_hash = h.finalize().to_hex().to_string();
    println!("source blake3 {}\nfinal  blake3 {}", c.content_hash, final_hash);
    assert_eq!(final_hash, c.content_hash);

    // Staged chunks are freed once the file is committed (no versions kept).
    let freed = drive.gc().unwrap();
    println!("gc freed {freed} staged chunks; total {:.1?}", started.elapsed());
    assert_eq!(freed, c.chunks.len());
    drop(drive);
    let path = tmp.path().to_path_buf();
    drop(tmp);
    assert!(!path.exists(), "temp dir deleted");
}
