//! Content-addressed chunk files on disk. Refcounts live in the index DB (`chunks`
//! table) so they change in the same transaction as the revisions that hold them.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::chunker::{hash_bytes, is_hash, MAX_BYTES};
use crate::error::{FilesError, Result};

#[derive(Debug, Clone)]
pub struct ChunkStore {
    dir: PathBuf,
}

impl ChunkStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(dir.join("tmp"))?;
        // A temp file left by a killed process is never a stored chunk.
        for entry in fs::read_dir(dir.join("tmp"))? {
            let _ = fs::remove_file(entry?.path());
        }
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, hash: &str) -> Result<PathBuf> {
        if !is_hash(hash) {
            return Err(FilesError::InvalidRequest(format!("{hash:?} is not a blake3 hex hash")));
        }
        Ok(self.dir.join(&hash[..2]).join(hash))
    }

    pub fn has(&self, hash: &str) -> bool {
        self.path(hash).map(|p| p.is_file()).unwrap_or(false)
    }

    /// Stores `data` under `hash` after checking the hash. Written to a temp file and
    /// renamed, so a crash never leaves a partial chunk under a real name.
    /// Returns true when the chunk was not stored before.
    pub fn put(&self, hash: &str, data: &[u8]) -> Result<bool> {
        if data.len() > MAX_BYTES as usize {
            return Err(FilesError::PayloadTooLarge(format!("chunk is {} bytes; max is {MAX_BYTES}", data.len())));
        }
        let actual = hash_bytes(data);
        if actual != hash {
            return Err(FilesError::ChunkHashMismatch(format!("bytes hash to {actual}, not {hash}")));
        }
        let path = self.path(hash)?;
        if path.is_file() {
            return Ok(false);
        }
        fs::create_dir_all(path.parent().expect("chunk path has a parent"))?;
        let tmp = self.dir.join("tmp").join(format!("{hash}.{}", rand::random::<u64>()));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_data()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(true)
    }

    /// Reads a chunk and checks its hash. A chunk whose bytes changed on disk is
    /// removed and reported as `chunk_hash_mismatch`, so the next have/need asks for it.
    pub fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.path(hash)?;
        let data = fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FilesError::NotFound(format!("chunk {hash}")),
            _ => FilesError::Io(e),
        })?;
        if hash_bytes(&data) != hash {
            let _ = fs::remove_file(&path);
            return Err(FilesError::ChunkHashMismatch(format!("stored chunk {hash} is damaged")));
        }
        Ok(data)
    }

    pub fn remove(&self, hash: &str) -> Result<()> {
        match fs::remove_file(self.path(hash)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
