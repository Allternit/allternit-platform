//! Upload sessions (SPEC section 5): create with a chunk list, PUT the chunks the
//! computer needs, append more refs for very large files, commit. Sessions live in the
//! index DB and chunks in the chunk store, so a session survives a dropped connection
//! or a restarted runtime until `expires_at`; `status` says what is still needed.

use std::collections::HashSet;
use std::fs;

use allternit_files_core::{ChunkPutResponse, ChunkRef, PrincipalRef, UploadCreateRequest, UploadState, UploadStatus};
use chrono::DateTime;
use rusqlite::{params, OptionalExtension};

use crate::chunker::{self, Chunked};
use crate::db::{self, from_json, to_json};
use crate::drive::{item_path, new_id, ts, Ctx, Drive, EditingDevice, Incoming, MANIFEST_PAGE};
use crate::error::{FilesError, Result};
use crate::model::Mutation;
use crate::naming::{validate_name, TEMP_PREFIX};

struct Session {
    id: String,
    req: UploadCreateRequest,
    principal: PrincipalRef,
    state: String,
    expires_at: String,
    utc_offset_minutes: Option<i32>,
}

impl Ctx<'_> {
    fn session(&self, session_id: &str) -> Result<Session> {
        let row: Option<(String, String, String, String, Option<i32>)> = self
            .c
            .query_row(
                "SELECT request, principal, state, expires_at, device_utc_offset FROM uploads WHERE session_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let (req, principal, state, expires_at, utc_offset_minutes) = row.ok_or(FilesError::UploadSessionNotFound)?;
        Ok(Session {
            id: session_id.into(),
            req: from_json(&req)?,
            principal: from_json(&principal)?,
            state,
            expires_at,
            utc_offset_minutes,
        })
    }

    /// The session, refused when it is no longer open.
    fn open_session(&self, session_id: &str) -> Result<Session> {
        let s = self.session(session_id)?;
        if s.state == "expired" || (s.state == "open" && s.expires_at <= ts(self.now)) {
            return Err(FilesError::UploadSessionExpired);
        }
        if s.state != "open" {
            return Err(FilesError::InvalidRequest(format!("upload session is {}", s.state)));
        }
        Ok(s)
    }

    fn session_refs(&self, session_id: &str) -> Result<Vec<ChunkRef>> {
        let mut st = self.c.prepare("SELECT hash, offset, size FROM upload_chunks WHERE session_id = ?1 ORDER BY idx")?;
        let v = st
            .query_map(params![session_id], |r| {
                Ok(ChunkRef { hash: r.get(0)?, offset: r.get::<_, i64>(1)? as u64, size: r.get::<_, i64>(2)? as u64 })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    fn append_refs(&self, session_id: &str, start_idx: usize, refs: &[ChunkRef]) -> Result<()> {
        let mut st = self.c.prepare("INSERT INTO upload_chunks(session_id, idx, hash, offset, size) VALUES(?1,?2,?3,?4,?5)")?;
        for (i, r) in refs.iter().enumerate() {
            st.execute(params![session_id, (start_idx + i) as i64, r.hash, r.offset as i64, r.size as i64])?;
        }
        Ok(())
    }

    fn status_of(&self, s: &Session) -> Result<UploadStatus> {
        let refs = self.session_refs(&s.id)?;
        let mut need = Vec::new();
        let mut seen = HashSet::new();
        let mut received = 0u64;
        for r in &refs {
            if self.store.has(&r.hash) {
                received += r.size;
            } else if seen.insert(r.hash.clone()) {
                need.push(r.hash.clone());
            }
        }
        let state = match s.state.as_str() {
            "open" if s.expires_at <= ts(self.now) => UploadState::Expired,
            "open" => UploadState::Open,
            "committed" => UploadState::Committed,
            "failed" => UploadState::Failed,
            _ => UploadState::Expired,
        };
        Ok(UploadStatus {
            session_id: s.id.clone(),
            state,
            need,
            received_bytes: received,
            total_bytes: s.req.size,
            expires_at: s.expires_at.clone(),
            max_request_bytes: self.opts.max_request_bytes,
        })
    }
}

impl Drive {
    /// `POST /uploads`. Validates the target and the first page of chunk refs.
    /// `req.utc_offset_minutes` (the uploading device's UTC offset) and
    /// `req.device_name` name a conflict copy at commit, so its `HHmm` is the
    /// device's local time (SPEC section 4).
    pub fn create_upload(&self, req: UploadCreateRequest, by: &PrincipalRef) -> Result<UploadStatus> {
        let device_utc_offset_minutes = utc_offset(req.utc_offset_minutes)?;
        chunker::check_params(&req.chunker)?;
        if !chunker::is_hash(&req.content_hash) {
            return Err(FilesError::InvalidRequest("content_hash is not a blake3 hex hash".into()));
        }
        if req.chunks.len() > MANIFEST_PAGE {
            return Err(FilesError::InvalidRequest(format!("at most {MANIFEST_PAGE} chunk refs per request")));
        }
        if (req.chunks.len() as u64) > req.total_chunks {
            return Err(FilesError::ManifestInvalid("more chunks than total_chunks".into()));
        }
        let end = chunker::check_refs(&req.chunks, 0)?;
        if end > req.size {
            return Err(FilesError::ManifestInvalid(format!("chunks run to {end} bytes, past size {}", req.size)));
        }
        if req.item_id.is_some() != req.base_rev.is_some() {
            return Err(FilesError::InvalidRequest("item_id and base_rev go together".into()));
        }
        validate_name(&req.name)?;
        if let Some(m) = &req.modified_at {
            DateTime::parse_from_rfc3339(m).map_err(|_| FilesError::InvalidRequest("modified_at is not RFC 3339".into()))?;
        }
        self.with_tx(|ctx| {
            ctx.writable_folder(&req.root_id, &req.parent_id)?;
            if let Some(id) = &req.item_id {
                let it = db::live_item(ctx.c, id)?;
                if it.root_id != req.root_id {
                    return Err(FilesError::InvalidRequest(format!("{id} is not in root {}", req.root_id)));
                }
            }
            let id = new_id("up_");
            let expires = ts(ctx.now + ctx.opts.upload_ttl);
            let mut stored = req.clone();
            stored.chunks = Vec::new();
            ctx.c.execute(
                "INSERT INTO uploads(session_id, request, principal, state, created_at, expires_at, device_utc_offset) VALUES(?1,?2,?3,'open',?4,?5,?6)",
                params![id, to_json(&stored), to_json(by), ts(ctx.now), expires, device_utc_offset_minutes],
            )?;
            ctx.append_refs(&id, 0, &req.chunks)?;
            let s = ctx.session(&id)?;
            ctx.status_of(&s)
        })
    }

    /// `POST /uploads/:id/chunks`: the next page of refs, continuing the offsets.
    pub fn append_upload_chunks(&self, session_id: &str, chunks: &[ChunkRef]) -> Result<UploadStatus> {
        if chunks.len() > MANIFEST_PAGE {
            return Err(FilesError::InvalidRequest(format!("at most {MANIFEST_PAGE} chunk refs per request")));
        }
        self.with_tx(|ctx| {
            let s = ctx.open_session(session_id)?;
            let refs = ctx.session_refs(session_id)?;
            let start = refs.last().map(|r| r.offset + r.size).unwrap_or(0);
            let end = chunker::check_refs(chunks, start)?;
            if (refs.len() + chunks.len()) as u64 > s.req.total_chunks || end > s.req.size {
                return Err(FilesError::ManifestInvalid("chunks go past total_chunks or size".into()));
            }
            ctx.append_refs(session_id, refs.len(), chunks)?;
            ctx.status_of(&s)
        })
    }

    /// `PUT /uploads/:id/chunks/:hash`. The bytes must hash to `hash` and be listed in
    /// the session. Hashing and writing happen outside the index lock.
    pub fn put_upload_chunk(&self, session_id: &str, hash: &str, data: &[u8]) -> Result<ChunkPutResponse> {
        let listed: Option<i64> = self.with_tx(|ctx| {
            ctx.open_session(session_id)?;
            Ok(ctx
                .c
                .query_row("SELECT size FROM upload_chunks WHERE session_id = ?1 AND hash = ?2 LIMIT 1", params![session_id, hash], |r| r.get(0))
                .optional()?)
        })?;
        let size = listed.ok_or_else(|| FilesError::InvalidRequest(format!("chunk {hash} is not part of this upload")))?;
        if data.len() as i64 != size {
            return Err(FilesError::ChunkHashMismatch(format!("chunk {hash} is {} bytes; the manifest says {size}", data.len())));
        }
        let stored = self.store.put(hash, data)?;
        self.with_tx(|ctx| {
            ctx.c.execute("INSERT OR IGNORE INTO chunks(hash, size, refcount) VALUES(?1, ?2, 0)", params![hash, size])?;
            let s = ctx.session(session_id)?;
            let st = ctx.status_of(&s)?;
            Ok(ChunkPutResponse { hash: hash.to_string(), stored, remaining: st.need.len() as u64 })
        })
    }

    /// `GET /uploads/:id`: what is still needed (resume).
    pub fn upload_status(&self, session_id: &str) -> Result<UploadStatus> {
        self.with_tx(|ctx| {
            let s = ctx.session(session_id)?;
            if s.state == "expired" || (s.state == "open" && s.expires_at <= ts(ctx.now)) {
                return Err(FilesError::UploadSessionExpired);
            }
            ctx.status_of(&s)
        })
    }

    /// `POST /uploads/:id/commit`: checks every chunk and the whole-file hash, then
    /// places the file with the same conflict rules as a small write.
    pub fn commit_upload(&self, session_id: &str) -> Result<Mutation> {
        self.with_tx(|ctx| {
            let s = ctx.open_session(session_id)?;
            let refs = ctx.session_refs(session_id)?;
            if refs.len() as u64 != s.req.total_chunks {
                return Err(FilesError::ManifestInvalid(format!("{} of {} chunk refs were sent", refs.len(), s.req.total_chunks)));
            }
            let end = chunker::check_refs(&refs, 0)?;
            if end != s.req.size {
                return Err(FilesError::ManifestInvalid(format!("chunks add up to {end} bytes, not {}", s.req.size)));
            }
            let mut missing: Vec<String> = Vec::new();
            for r in &refs {
                if !ctx.store.has(&r.hash) && !missing.contains(&r.hash) {
                    missing.push(r.hash.clone());
                }
            }
            if !missing.is_empty() {
                return Err(FilesError::ChunkMissing { missing });
            }
            let parent = ctx.writable_folder(&s.req.root_id, &s.req.parent_id)?;
            let temp = item_path(ctx.c, &parent)?.join(format!("{TEMP_PREFIX}upload-{session_id}.part"));
            ctx.assemble(&refs, s.req.size, &s.req.content_hash, &temp)?;
            let chunked = Chunked { chunks: refs, size: s.req.size, content_hash: s.req.content_hash.clone() };
            let device = EditingDevice { name: s.req.device_name.clone(), utc_offset_minutes: s.utc_offset_minutes };
            let out = ctx.place_content(
                &parent,
                &s.req.name,
                s.req.item_id.as_deref(),
                s.req.base_rev.as_deref(),
                s.req.on_conflict,
                &s.principal,
                &device,
                Incoming { temp: &temp, chunked: &chunked, from_store: true, modified_at: s.req.modified_at.clone() },
            );
            if out.is_err() {
                let _ = fs::remove_file(&temp);
            }
            let out = out?;
            ctx.c.execute("UPDATE uploads SET state = 'committed' WHERE session_id = ?1", params![session_id])?;
            ctx.c.execute("DELETE FROM upload_chunks WHERE session_id = ?1", params![session_id])?;
            Ok(out)
        })
    }

    /// `DELETE /uploads/:id`. Chunks nothing else holds go at the next `gc`.
    pub fn abort_upload(&self, session_id: &str) -> Result<()> {
        self.with_tx(|ctx| {
            ctx.session(session_id)?;
            ctx.c.execute("DELETE FROM upload_chunks WHERE session_id = ?1", params![session_id])?;
            ctx.c.execute("DELETE FROM uploads WHERE session_id = ?1", params![session_id])?;
            Ok(())
        })
    }
}

/// A device's UTC offset in minutes, within the schema's -840..=840 (±14 h).
pub fn utc_offset(minutes: Option<i64>) -> Result<Option<i32>> {
    match minutes {
        None => Ok(None),
        Some(m) if (-840..=840).contains(&m) => Ok(Some(m as i32)),
        Some(m) => Err(FilesError::InvalidRequest(format!("utc_offset_minutes {m} is out of range (-840..=840)"))),
    }
}
