//! Writes a local file back through an upload session (SPEC section 5).
//!
//! The file is cut with the pinned v1 chunker (the engine's, so boundaries match
//! the runtime and dedup works). Only the chunks the runtime still needs are sent,
//! each at most 4 MiB, so the relay accepts them. A stale `base_rev` becomes a
//! conflict copy on the server (`on_conflict: keep_both`).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use allternit_files_core::*;
use allternit_files_engine::chunker;

use crate::client::{ApiError, FilesClient};
use crate::wake::{WakeContext, WakeGate};

/// At most this many chunk refs per create or append request (schema limit).
pub const REFS_PER_REQUEST: usize = 10_000;

/// Where the bytes go.
#[derive(Debug, Clone)]
pub struct Target {
    pub root_id: String,
    pub parent_id: String,
    pub name: String,
    /// Set when replacing an existing item.
    pub item_id: Option<String>,
    pub base_rev: Option<String>,
}

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Transport(format!("local file: {e}"))
}

/// The editing device's UTC offset in minutes (conflict names use local time).
pub fn utc_offset_minutes() -> i64 {
    i64::from(chrono::Local::now().offset().local_minus_utc()) / 60
}

fn read_chunk(file: &mut File, r: &ChunkRef) -> Result<Vec<u8>, ApiError> {
    file.seek(SeekFrom::Start(r.offset)).map_err(io)?;
    let mut buf = vec![0u8; r.size as usize];
    file.read_exact(&mut buf).map_err(io)?;
    if chunker::hash_bytes(&buf) != r.hash {
        // The file changed while uploading; the next local change event retries.
        return Err(io("changed during upload"));
    }
    Ok(buf)
}

/// Uploads `path` to `target`. Content needs a wake ticket when the computer sleeps,
/// so every call goes through the wake gate.
pub fn upload_file(client: &FilesClient, wake: &WakeGate, path: &Path, target: &Target) -> Result<UploadCommitResponse, ApiError> {
    let file = File::open(path).map_err(io)?;
    let chunked = chunker::chunk_reader(file, |_| Ok(())).map_err(io)?;
    let by_hash: HashMap<&str, &ChunkRef> = chunked.chunks.iter().map(|c| (c.hash.as_str(), c)).collect();
    let ctx = WakeContext {
        reason: WakeReason::Upload,
        item_id: target.item_id.as_deref(),
        what: format!("Saving \"{}\"", target.name),
        background: false,
    };
    let pages: Vec<&[ChunkRef]> = if chunked.chunks.is_empty() {
        vec![&[][..]]
    } else {
        chunked.chunks.chunks(REFS_PER_REQUEST).collect()
    };
    let req = UploadCreateRequest {
        root_id: target.root_id.clone(),
        parent_id: target.parent_id.clone(),
        name: target.name.clone(),
        item_id: target.item_id.clone(),
        base_rev: target.base_rev.clone(),
        size: chunked.size,
        content_hash: chunked.content_hash.clone(),
        chunker: chunker::params(),
        chunks: pages[0].to_vec(),
        total_chunks: chunked.chunks.len() as u64,
        on_conflict: OnConflict::KeepBoth,
        modified_at: None,
        device_name: Some(client.device_name.clone()),
        utc_offset_minutes: Some(utc_offset_minutes()),
    };
    let mut status = wake.run(client, &ctx, |t| client.upload_create(&req, t))?;
    let session = status.session_id.clone();
    for page in &pages[1..] {
        let append = UploadChunksAppend { chunks: page.to_vec() };
        status = wake.run(client, &ctx, |t| client.upload_append(&session, &append, t))?;
    }
    let mut file = File::open(path).map_err(io)?;
    // One retry: a commit that reports chunk_missing names what to send again.
    for round in 0..2 {
        for hash in status.need.clone() {
            let r = by_hash.get(hash.as_str()).ok_or_else(|| ApiError::Decode(format!("server needs unknown chunk {hash}")))?;
            let bytes = read_chunk(&mut file, r)?;
            wake.run(client, &ctx, |t| client.put_chunk(&session, &hash, &bytes, t))?;
        }
        match wake.run(client, &ctx, |t| client.upload_commit(&session, t)) {
            Err(e) if e.is(ErrorCode::ChunkMissing) && round == 0 => {
                status = wake.run(client, &ctx, |t| client.upload_status(&session, t))?;
            }
            other => return other,
        }
    }
    unreachable!("the second round returns")
}
