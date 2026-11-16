//! Replicas (SPEC sections 10 and 13): encrypted copies of one root on another of
//! the user's devices.
//!
//! Three parties, three views:
//! - **Source** (the computer that owns the files, holding the Files root key):
//!   seals each chunk with `files-replica/1/<replica_id>` ([`seal_blob`]) and a
//!   catalog of the root (names, folders, chunk lists) that is itself chunked and
//!   sealed, plus a small sealed [`Head`] that names the catalog parts.
//! - **Host** ([`HostStore`]): keeps sealed blobs named by locator and the sealed
//!   head. It has no key, so it can check shapes and sizes ([`check_sealed`]) but
//!   never read a name, a hash or a byte of content.
//! - **Reader / restorer** (any of the user's devices with the key): opens the
//!   head and catalog, then opens chunks one by one. Every chunk is re-hashed on
//!   open and every file is re-hashed whole ([`restore`]), so a changed blob fails
//!   with `integrity_failed` and is listed in the report.
//!
//! Also here: the deterministic replica id ([`replica_id`]) and the device
//! envelope that wraps the Files root key to one device ([`wrap_root_key`]).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use allternit_files_core::{ChunkRef, EncryptedChunkHeader, ErrorCode, ItemKind, ReplicaRestoreReport, RestoreFailure};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::chunker::{self, hash_bytes, is_hash, MAX_BYTES};
use crate::crypto::{replica_info, ChunkSealer, FilesRootKey, CIPHER};
use crate::error::{FilesError, Result};
use crate::model::EMPTY_HASH;

/// Poly1305 tag added to every sealed chunk.
pub const TAG_BYTES: u64 = 16;
/// A sealed head is small: it names the catalog parts, nothing else.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;
/// `KeyEnvelope.wrap_alg`.
pub const WRAP_ALG: &str = "x25519-hkdf-sha256-xchacha20poly1305";
const MAX_HEADER_JSON: usize = 4096;

/// The replica id for (source computer, root, host device). Both the source and
/// the host derive it from `CreateReplicaRequest` alone, so neither has to tell
/// the other: `rp_` + the first 24 hex digits of
/// `blake3("files-replica-id/1|<source>|<root>|<host>")`.
pub fn replica_id(source_computer_id: &str, root_id: &str, host_device_id: &str) -> String {
    let h = blake3::hash(format!("files-replica-id/1|{source_computer_id}|{root_id}|{host_device_id}").as_bytes());
    let hex = h.to_hex();
    format!("rp_{}", &hex[..24])
}

/// The 6-digit code a new device shows next to its key-share request
/// (`KeyShareRequest.confirm_code`). Derived from the device's X25519 public key,
/// so the approving device computes the same code from the request and the user
/// compares the two: `u32_be(blake3("files-key-confirm/1|" || device_pub)[0..4]) % 1000000`.
pub fn confirm_code(device_pub: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"files-key-confirm/1|");
    h.update(device_pub);
    let b = h.finalize();
    let n = u32::from_be_bytes(b.as_bytes()[..4].try_into().expect("4 bytes")) % 1_000_000;
    format!("{n:06}")
}

// ---- sealed blobs ----

/// `u32 BE header length | header JSON | ciphertext` (SPEC section 13).
pub fn encode_blob(header: &EncryptedChunkHeader, ciphertext: &[u8]) -> Vec<u8> {
    let json = serde_json::to_vec(header).expect("header serializes");
    let mut out = Vec::with_capacity(4 + json.len() + ciphertext.len());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(ciphertext);
    out
}

pub fn decode_blob(blob: &[u8]) -> Result<(EncryptedChunkHeader, &[u8])> {
    let bad = || FilesError::IntegrityFailed("sealed blob is damaged".into());
    if blob.len() < 4 {
        return Err(bad());
    }
    let hlen = u32::from_be_bytes(blob[..4].try_into().expect("4 bytes")) as usize;
    if hlen > MAX_HEADER_JSON {
        return Err(bad());
    }
    let header_bytes = blob.get(4..4 + hlen).ok_or_else(bad)?;
    let header: EncryptedChunkHeader = serde_json::from_slice(header_bytes).map_err(|_| bad())?;
    Ok((header, &blob[4 + hlen..]))
}

/// Seals one plaintext chunk: `(locator, blob)`.
pub fn seal_blob(sealer: &ChunkSealer, plaintext: &[u8]) -> Result<(String, Vec<u8>)> {
    let (header, ct) = sealer.seal(plaintext)?;
    let locator = header.locator.clone();
    Ok((locator, encode_blob(&header, &ct)))
}

/// Opens one blob and checks it is the chunk named by `plaintext_hash`.
pub fn open_blob(sealer: &ChunkSealer, blob: &[u8], plaintext_hash: &str) -> Result<Vec<u8>> {
    let (header, ct) = decode_blob(blob)?;
    sealer.open(&header, ct, plaintext_hash)
}

/// What a host can check without the key: the blob parses, is stored under its
/// own locator, is v1 XChaCha20-Poly1305, holds at most one 4 MiB chunk plus the
/// tag, and (once the replica has one) uses the replica's `key_id`.
pub fn check_sealed(locator: &str, blob: &[u8], key_id: Option<&str>) -> Result<EncryptedChunkHeader> {
    if !is_hash(locator) {
        return Err(FilesError::InvalidRequest(format!("{locator:?} is not a locator")));
    }
    let (h, ct) = decode_blob(blob).map_err(|_| FilesError::InvalidRequest("not a sealed chunk".into()))?;
    if h.v != 1 || h.cipher != CIPHER {
        return Err(FilesError::InvalidRequest(format!("unknown sealed chunk format v{} {}", h.v, h.cipher)));
    }
    if h.locator != locator {
        return Err(FilesError::InvalidRequest("the blob's locator is not the name it was sent under".into()));
    }
    if h.plaintext_size > MAX_BYTES as u64 {
        return Err(FilesError::PayloadTooLarge(format!("a sealed chunk holds at most {MAX_BYTES} bytes")));
    }
    if ct.len() as u64 != h.plaintext_size + TAG_BYTES {
        return Err(FilesError::InvalidRequest("ciphertext length does not match plaintext_size".into()));
    }
    if URL_SAFE_NO_PAD.decode(&h.nonce).map(|n| n.len()).unwrap_or(0) != 24 {
        return Err(FilesError::InvalidRequest("nonce must be 24 bytes".into()));
    }
    if let Some(k) = key_id.filter(|k| !k.is_empty()) {
        if h.key_id != k {
            return Err(FilesError::InvalidRequest(format!("sealed with key {}, this replica uses {k}", h.key_id)));
        }
    }
    Ok(h)
}

// ---- the host's store ----

/// One replica on its host: `blobs/<locator[..2]>/<locator>` plus `head`.
/// Holds only ciphertext; there is no key anywhere in this type.
#[derive(Debug, Clone)]
pub struct HostStore {
    dir: PathBuf,
}

impl HostStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(dir.join("blobs"))?;
        Ok(Self { dir })
    }

    fn blob_path(&self, locator: &str) -> Result<PathBuf> {
        if !is_hash(locator) {
            return Err(FilesError::InvalidRequest(format!("{locator:?} is not a locator")));
        }
        Ok(self.dir.join("blobs").join(&locator[..2]).join(locator))
    }

    fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp{}", rand::random::<u32>()));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        fs::rename(tmp, path)?;
        Ok(())
    }

    /// Stores a sealed blob after [`check_sealed`]. `Ok((header, false))` when it
    /// was already there (content-addressed, so a resend is a no-op).
    pub fn put_blob(&self, locator: &str, blob: &[u8], key_id: Option<&str>) -> Result<(EncryptedChunkHeader, bool)> {
        let header = check_sealed(locator, blob, key_id)?;
        let path = self.blob_path(locator)?;
        if path.is_file() {
            return Ok((header, false));
        }
        Self::write_atomic(&path, blob)?;
        Ok((header, true))
    }

    pub fn get_blob(&self, locator: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.blob_path(locator)?) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn has_blob(&self, locator: &str) -> bool {
        self.blob_path(locator).map(|p| p.is_file()).unwrap_or(false)
    }

    /// Stores the sealed head. Returns the `key_id` it says it was sealed with
    /// (the only field a host can read).
    pub fn put_head(&self, bytes: &[u8], key_id: Option<&str>) -> Result<String> {
        if bytes.len() > MAX_HEAD_BYTES {
            return Err(FilesError::PayloadTooLarge(format!("a replica head is at most {MAX_HEAD_BYTES} bytes")));
        }
        let env = head_envelope(bytes).map_err(|_| FilesError::InvalidRequest("not a sealed replica head".into()))?.0;
        if let Some(k) = key_id.filter(|k| !k.is_empty()) {
            if env.key_id != k {
                return Err(FilesError::InvalidRequest(format!("sealed with key {}, this replica uses {k}", env.key_id)));
            }
        }
        Self::write_atomic(&self.dir.join("head"), bytes)?;
        Ok(env.key_id)
    }

    pub fn get_head(&self) -> Result<Option<Vec<u8>>> {
        match fs::read(self.dir.join("head")) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Bytes on disk for this replica (ciphertext and head).
    pub fn used_bytes(&self) -> u64 {
        fn walk(p: &std::path::Path) -> u64 {
            let Ok(rd) = fs::read_dir(p) else { return 0 };
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.dir)
    }

    /// Deletes the replica from this host.
    pub fn destroy(self) -> Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

// ---- catalog and head ----

/// One item of the replicated root, as the source saw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub item_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: ItemKind,
    pub size: u64,
    pub content_hash: Option<String>,
    pub rev: String,
    pub modified_at: String,
    /// Plaintext chunk list in order (files only).
    #[serde(default)]
    pub chunks: Vec<ChunkRef>,
}

/// The whole root at one cursor. Sealed in parts like a file, never sent plain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub replica_id: String,
    pub source_computer_id: String,
    pub root_id: String,
    pub root_item_id: String,
    pub cursor: Option<String>,
    pub synced_at: String,
    pub items: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn entry(&self, item_id: &str) -> Option<&CatalogEntry> {
        self.items.iter().find(|e| e.item_id == item_id)
    }
}

/// The sealed pointer a host keeps: which sealed parts make up the catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub v: u32,
    pub replica_id: String,
    pub cursor: Option<String>,
    pub synced_at: String,
    pub catalog_hash: String,
    pub catalog_size: u64,
    pub parts: Vec<ChunkRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadEnvelope {
    v: u32,
    key_id: String,
    nonce: String,
}

fn head_key(frk: &FilesRootKey, replica_id: &str) -> [u8; 32] {
    frk.subkey(&format!("{}/head", replica_info(replica_id)))
}

fn head_aad(replica_id: &str, key_id: &str) -> Vec<u8> {
    format!("afh1|1|{replica_id}|{key_id}").into_bytes()
}

fn head_envelope(bytes: &[u8]) -> Result<(HeadEnvelope, &[u8])> {
    let bad = || FilesError::IntegrityFailed("replica head is damaged".into());
    if bytes.len() < 4 {
        return Err(bad());
    }
    let hlen = u32::from_be_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
    if hlen > MAX_HEADER_JSON {
        return Err(bad());
    }
    let json = bytes.get(4..4 + hlen).ok_or_else(bad)?;
    let env: HeadEnvelope = serde_json::from_slice(json).map_err(|_| bad())?;
    if env.v != 1 {
        return Err(bad());
    }
    Ok((env, &bytes[4 + hlen..]))
}

/// Seals a head under `files-replica/1/<replica_id>/head` (HKDF from the FRK),
/// AAD `afh1|1|<replica_id>|<key_id>`. Same `u32 | JSON | ciphertext` layout as a
/// chunk; the JSON is `{v, key_id, nonce}`.
pub fn seal_head(frk: &FilesRootKey, head: &Head) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let env = HeadEnvelope { v: 1, key_id: frk.key_id().to_string(), nonce: URL_SAFE_NO_PAD.encode(nonce) };
    let pt = serde_json::to_vec(head).map_err(|e| FilesError::Internal(e.to_string()))?;
    let key = head_key(frk, &head.replica_id);
    let ct = XChaCha20Poly1305::new((&key).into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: &pt, aad: &head_aad(&head.replica_id, frk.key_id()) })
        .map_err(|_| FilesError::Internal("seal failed".into()))?;
    let json = serde_json::to_vec(&env).map_err(|e| FilesError::Internal(e.to_string()))?;
    let mut out = Vec::with_capacity(4 + json.len() + ct.len());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open_head(frk: &FilesRootKey, replica_id: &str, bytes: &[u8]) -> Result<Head> {
    let (env, ct) = head_envelope(bytes)?;
    if env.key_id != frk.key_id() {
        return Err(FilesError::KeyUnavailable(format!("replica head sealed with key {}", env.key_id)));
    }
    let nonce = URL_SAFE_NO_PAD
        .decode(&env.nonce)
        .ok()
        .filter(|n| n.len() == 24)
        .ok_or_else(|| FilesError::IntegrityFailed("bad head nonce".into()))?;
    let key = head_key(frk, replica_id);
    let pt = XChaCha20Poly1305::new((&key).into())
        .decrypt(XNonce::from_slice(&nonce), Payload { msg: ct, aad: &head_aad(replica_id, &env.key_id) })
        .map_err(|_| FilesError::IntegrityFailed("replica head failed authentication".into()))?;
    let head: Head = serde_json::from_slice(&pt).map_err(|_| FilesError::IntegrityFailed("replica head is damaged".into()))?;
    if head.replica_id != replica_id {
        return Err(FilesError::IntegrityFailed("replica head belongs to another replica".into()));
    }
    Ok(head)
}

/// Splits a catalog into plaintext parts (FastCDC, so an unchanged stretch of
/// the catalog dedups on the host) and the head that names them. The caller
/// seals each part with [`seal_blob`] and the head with [`seal_head`].
pub fn pack_catalog(catalog: &Catalog) -> Result<(Head, Vec<(ChunkRef, Vec<u8>)>)> {
    let json = serde_json::to_vec(catalog).map_err(|e| FilesError::Internal(e.to_string()))?;
    let c = chunker::chunk_bytes(&json);
    let parts = c
        .chunks
        .iter()
        .map(|r| (r.clone(), json[r.offset as usize..(r.offset + r.size) as usize].to_vec()))
        .collect();
    let head = Head {
        v: 1,
        replica_id: catalog.replica_id.clone(),
        cursor: catalog.cursor.clone(),
        synced_at: catalog.synced_at.clone(),
        catalog_hash: c.content_hash,
        catalog_size: c.size,
        parts: c.chunks,
    };
    Ok((head, parts))
}

/// Fetches a sealed blob by locator: `Ok(None)` when the host does not have it.
pub type Fetch<'a> = dyn FnMut(&str) -> Result<Option<Vec<u8>>> + 'a;

/// Opens the catalog a head names. Every part is re-hashed, then the whole.
pub fn read_catalog(frk: &FilesRootKey, head: &Head, fetch: &mut Fetch<'_>) -> Result<Catalog> {
    let sealer = ChunkSealer::for_replica(frk, &head.replica_id);
    let mut json = Vec::with_capacity(head.catalog_size as usize);
    for part in &head.parts {
        let locator = sealer.locator(&part.hash)?;
        let blob = fetch(&locator)?.ok_or_else(|| FilesError::NotFound("a catalog part is missing on the host".into()))?;
        json.extend_from_slice(&open_blob(&sealer, &blob, &part.hash)?);
    }
    if hash_bytes(&json) != head.catalog_hash {
        return Err(FilesError::IntegrityFailed("replica catalog does not match its head".into()));
    }
    let catalog: Catalog =
        serde_json::from_slice(&json).map_err(|_| FilesError::IntegrityFailed("replica catalog is damaged".into()))?;
    if catalog.replica_id != head.replica_id {
        return Err(FilesError::IntegrityFailed("replica catalog belongs to another replica".into()));
    }
    Ok(catalog)
}

/// Reads one file from a replica, every chunk and the whole file re-hashed.
pub fn read_file(frk: &FilesRootKey, catalog: &Catalog, item_id: &str, fetch: &mut Fetch<'_>) -> Result<Vec<u8>> {
    let entry = catalog.entry(item_id).ok_or_else(|| FilesError::NotFound(format!("{item_id} is not in this replica")))?;
    if entry.kind != ItemKind::File {
        return Err(FilesError::InvalidRequest(format!("{item_id} is a folder")));
    }
    let sealer = ChunkSealer::for_replica(frk, &catalog.replica_id);
    let mut out = Vec::with_capacity(entry.size as usize);
    for c in &entry.chunks {
        let locator = sealer.locator(&c.hash)?;
        let blob = fetch(&locator)?.ok_or_else(|| FilesError::NotFound(format!("chunk {} is missing on the host", c.hash)))?;
        out.extend_from_slice(&open_blob(&sealer, &blob, &c.hash)?);
    }
    let want = entry.content_hash.as_deref().unwrap_or(EMPTY_HASH);
    if out.len() as u64 != entry.size || hash_bytes(&out) != want {
        return Err(FilesError::IntegrityFailed(format!("{} does not match its content hash", entry.name)));
    }
    Ok(out)
}

// ---- restore ----

/// Where a restore writes. Chunks arrive already opened and re-hashed; a file is
/// committed (`finish`) only after its whole-file hash matched, otherwise it is
/// dropped (`abort`), so a tampered replica never leaves a half-written file.
pub trait RestoreSink {
    fn folder(&mut self, entry: &CatalogEntry) -> Result<()>;
    fn begin(&mut self, entry: &CatalogEntry) -> Result<()>;
    fn chunk(&mut self, entry: &CatalogEntry, chunk: &ChunkRef, data: &[u8]) -> Result<()>;
    fn finish(&mut self, entry: &CatalogEntry) -> Result<()>;
    fn abort(&mut self, entry: &CatalogEntry);
}

/// Restores `only` (item ids; a folder brings its subtree; `None` = everything)
/// from a replica into `sink`, checking every chunk hash and every whole-file
/// hash. Never stops at the first bad chunk: the report counts them all and lists
/// each file that failed with its reason (`integrity_failed` for a changed blob,
/// `replica_unavailable` for a missing one, `key_unavailable` for another key).
pub fn restore(
    frk: &FilesRootKey,
    catalog: &Catalog,
    only: Option<&[String]>,
    fetch: &mut Fetch<'_>,
    sink: &mut dyn RestoreSink,
) -> ReplicaRestoreReport {
    let sealer = ChunkSealer::for_replica(frk, &catalog.replica_id);
    let parents: HashMap<&str, Option<&str>> =
        catalog.items.iter().map(|e| (e.item_id.as_str(), e.parent_id.as_deref())).collect();
    let wanted: Option<HashSet<&str>> = only.map(|ids| ids.iter().map(String::as_str).collect());
    let selected = |id: &str| -> bool {
        let Some(w) = &wanted else { return true };
        let mut cur = Some(id);
        let mut hops = 0;
        while let Some(c) = cur {
            if w.contains(c) {
                return true;
            }
            hops += 1;
            if hops > 4096 {
                return false;
            }
            cur = parents.get(c).copied().flatten();
        }
        false
    };

    let mut report = ReplicaRestoreReport {
        replica_id: catalog.replica_id.clone(),
        files_restored: 0,
        bytes: 0,
        chunks_verified: 0,
        chunks_failed: 0,
        failures: Vec::new(),
        verified: false,
    };
    let fail = |report: &mut ReplicaRestoreReport, item_id: &str, reason: ErrorCode| {
        if !report.failures.iter().any(|f| f.item_id == item_id) {
            report.failures.push(RestoreFailure { item_id: item_id.to_string(), reason });
        }
    };

    // Folders that hold a selected file come back too, so it has somewhere to go.
    let mut ancestors: HashSet<&str> = HashSet::new();
    for e in catalog.items.iter().filter(|e| e.kind == ItemKind::File && selected(&e.item_id)) {
        let mut cur = e.parent_id.as_deref();
        while let Some(c) = cur {
            if !ancestors.insert(c) {
                break;
            }
            cur = parents.get(c).copied().flatten();
        }
    }

    // Folders first, parents before children (the catalog is written that way).
    for e in catalog
        .items
        .iter()
        .filter(|e| e.kind == ItemKind::Folder && e.parent_id.is_some() && (selected(&e.item_id) || ancestors.contains(e.item_id.as_str())))
    {
        if let Err(err) = sink.folder(e) {
            fail(&mut report, &e.item_id, err.code());
        }
    }

    for e in catalog.items.iter().filter(|e| e.kind == ItemKind::File && selected(&e.item_id)) {
        if let Err(err) = sink.begin(e) {
            fail(&mut report, &e.item_id, err.code());
            continue;
        }
        let mut ok = true;
        let mut whole = blake3::Hasher::new();
        let mut size = 0u64;
        for c in &e.chunks {
            let opened = sealer.locator(&c.hash).and_then(|loc| match fetch(&loc) {
                Ok(Some(blob)) => open_blob(&sealer, &blob, &c.hash),
                Ok(None) => Err(FilesError::NotFound(format!("chunk {} is missing on the host", c.hash))),
                Err(err) => Err(err),
            });
            match opened {
                Ok(data) => {
                    report.chunks_verified += 1;
                    whole.update(&data);
                    size += data.len() as u64;
                    if ok {
                        if let Err(err) = sink.chunk(e, c, &data) {
                            ok = false;
                            fail(&mut report, &e.item_id, err.code());
                        }
                    }
                }
                Err(err) => {
                    report.chunks_failed += 1;
                    ok = false;
                    let reason = match err.code() {
                        ErrorCode::IntegrityFailed => ErrorCode::IntegrityFailed,
                        ErrorCode::KeyUnavailable => ErrorCode::KeyUnavailable,
                        _ => ErrorCode::ReplicaUnavailable,
                    };
                    fail(&mut report, &e.item_id, reason);
                }
            }
        }
        if ok {
            let want = e.content_hash.as_deref().unwrap_or(EMPTY_HASH);
            if size != e.size || whole.finalize().to_hex().as_str() != want {
                ok = false;
                fail(&mut report, &e.item_id, ErrorCode::IntegrityFailed);
            }
        }
        if ok {
            match sink.finish(e) {
                Ok(()) => {
                    report.files_restored += 1;
                    report.bytes += e.size;
                }
                Err(err) => {
                    sink.abort(e);
                    fail(&mut report, &e.item_id, err.code());
                }
            }
        } else {
            sink.abort(e);
        }
    }
    report.verified = report.failures.is_empty() && report.chunks_failed == 0;
    report
}

// ---- device envelopes ----

fn envelope_kek(shared: &[u8; 32], ephemeral_pub: &[u8; 32], device_pub: &[u8; 32], device_id: &str) -> [u8; 32] {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(ephemeral_pub);
    salt[32..].copy_from_slice(device_pub);
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(format!("files-envelope/1/{device_id}").as_bytes(), &mut out)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

fn envelope_aad(key_id: &str, device_id: &str) -> Vec<u8> {
    format!("afe1|{key_id}|{device_id}").into_bytes()
}

/// Wraps the FRK to one device (`KeyEnvelope`, `x25519-hkdf-sha256-xchacha20poly1305`).
/// `shared` is X25519(ephemeral secret, device public key); the caller does the
/// Diffie-Hellman. KEK = HKDF-SHA256(salt = ephemeral_pub || device_pub,
/// ikm = shared, info = `files-envelope/1/<device_id>`), AAD `afe1|<key_id>|<device_id>`.
/// Returns `(nonce, wrapped_key)`, both base64url without padding.
pub fn wrap_root_key(
    frk: &FilesRootKey,
    shared: &[u8; 32],
    ephemeral_pub: &[u8; 32],
    device_pub: &[u8; 32],
    device_id: &str,
) -> Result<(String, String)> {
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let kek = envelope_kek(shared, ephemeral_pub, device_pub, device_id);
    let ct = XChaCha20Poly1305::new((&kek).into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: frk.secret(), aad: &envelope_aad(frk.key_id(), device_id) })
        .map_err(|_| FilesError::Internal("wrap failed".into()))?;
    Ok((URL_SAFE_NO_PAD.encode(nonce), URL_SAFE_NO_PAD.encode(ct)))
}

/// Opens an envelope on the device it was wrapped to. `shared` is
/// X25519(device secret, ephemeral_pub).
pub fn unwrap_root_key(
    key_id: &str,
    shared: &[u8; 32],
    ephemeral_pub: &[u8; 32],
    device_pub: &[u8; 32],
    device_id: &str,
    nonce: &str,
    wrapped_key: &str,
) -> Result<FilesRootKey> {
    let bad = || FilesError::KeyUnavailable("the Files key envelope does not open on this device".into());
    let nonce = URL_SAFE_NO_PAD.decode(nonce).ok().filter(|n| n.len() == 24).ok_or_else(bad)?;
    let ct = URL_SAFE_NO_PAD.decode(wrapped_key).map_err(|_| bad())?;
    let kek = envelope_kek(shared, ephemeral_pub, device_pub, device_id);
    let pt = XChaCha20Poly1305::new((&kek).into())
        .decrypt(XNonce::from_slice(&nonce), Payload { msg: &ct, aad: &envelope_aad(key_id, device_id) })
        .map_err(|_| bad())?;
    let key: [u8; 32] = pt.as_slice().try_into().map_err(|_| bad())?;
    Ok(FilesRootKey::from_bytes(key_id, key))
}
