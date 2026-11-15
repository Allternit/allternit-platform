//! Content-defined chunking (SPEC section 5): FastCDC 2020, blake3-256, min 256 KiB,
//! avg 1 MiB, max 4 MiB.
//!
//! Boundaries come from `fastcdc::v2020` at normalization level 1 with its default
//! gear table (seed 0). Clients that want dedup with the runtime use the same.

use std::io::Read;

use allternit_files_core::{ChunkRef, ChunkerParams};
use fastcdc::v2020::{FastCDC, StreamCDC};

use crate::error::{FilesError, Result};

pub const MIN_BYTES: u32 = 262_144;
pub const AVG_BYTES: u32 = 1_048_576;
/// 4 MiB: the largest chunk the schema and the relay accept.
pub const MAX_BYTES: u32 = 4_194_304;
pub const ALGO: &str = "fastcdc-2020";
pub const HASH: &str = "blake3";

/// The v1 chunker parameters every manifest carries.
pub fn params() -> ChunkerParams {
    ChunkerParams {
        algo: ALGO.to_string(),
        hash: HASH.to_string(),
        min_bytes: MIN_BYTES as u64,
        avg_bytes: AVG_BYTES as u64,
        max_bytes: MAX_BYTES as u64,
    }
}

/// Refuses any parameters other than v1, so a manifest can't claim a chunk size the
/// relay would reject.
pub fn check_params(p: &ChunkerParams) -> Result<()> {
    if *p != params() {
        return Err(FilesError::ManifestInvalid(format!(
            "chunker must be {ALGO}/{HASH} {MIN_BYTES}/{AVG_BYTES}/{MAX_BYTES}"
        )));
    }
    Ok(())
}

/// blake3-256 of `data`, lower-case hex.
pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

pub fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One chunk with its bytes.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub hash: String,
    pub offset: u64,
    pub data: Vec<u8>,
}

impl Chunk {
    pub fn to_ref(&self) -> ChunkRef {
        ChunkRef { hash: self.hash.clone(), offset: self.offset, size: self.data.len() as u64 }
    }
}

/// The result of chunking a whole stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunked {
    pub chunks: Vec<ChunkRef>,
    pub size: u64,
    /// blake3 of the whole content.
    pub content_hash: String,
}

/// Chunks a stream without holding it in memory, calling `each` per chunk in order.
pub fn chunk_reader<R: Read>(reader: R, mut each: impl FnMut(&Chunk) -> Result<()>) -> Result<Chunked> {
    let mut whole = blake3::Hasher::new();
    let mut chunks = Vec::new();
    let mut size = 0u64;
    for item in StreamCDC::new(reader, MIN_BYTES, AVG_BYTES, MAX_BYTES) {
        let data = item.map_err(|e| match e {
            fastcdc::v2020::Error::IoError(io) => FilesError::Io(io),
            other => FilesError::Internal(format!("chunker: {other}")),
        })?;
        whole.update(&data.data);
        let chunk = Chunk { hash: hash_bytes(&data.data), offset: data.offset, data: data.data };
        size += chunk.data.len() as u64;
        chunks.push(chunk.to_ref());
        each(&chunk)?;
    }
    Ok(Chunked { chunks, size, content_hash: whole.finalize().to_hex().to_string() })
}

/// Chunks bytes already in memory. Same boundaries as [`chunk_reader`].
pub fn chunk_bytes(data: &[u8]) -> Chunked {
    let chunks = FastCDC::new(data, MIN_BYTES, AVG_BYTES, MAX_BYTES)
        .map(|c| ChunkRef {
            hash: hash_bytes(&data[c.offset..c.offset + c.length]),
            offset: c.offset as u64,
            size: c.length as u64,
        })
        .collect();
    Chunked { chunks, size: data.len() as u64, content_hash: hash_bytes(data) }
}

/// Checks a chunk list: v1 sizes, hex hashes, contiguous offsets starting at `start`.
/// Returns the offset after the last chunk.
pub fn check_refs(refs: &[ChunkRef], start: u64) -> Result<u64> {
    let mut next = start;
    for r in refs {
        if !is_hash(&r.hash) {
            return Err(FilesError::ManifestInvalid(format!("{:?} is not a blake3 hex hash", r.hash)));
        }
        if r.size == 0 || r.size > MAX_BYTES as u64 {
            return Err(FilesError::ManifestInvalid(format!("chunk {} has size {}", r.hash, r.size)));
        }
        if r.offset != next {
            return Err(FilesError::ManifestInvalid(format!(
                "chunk {} starts at {} but the previous one ends at {next}",
                r.hash, r.offset
            )));
        }
        next += r.size;
    }
    Ok(next)
}
