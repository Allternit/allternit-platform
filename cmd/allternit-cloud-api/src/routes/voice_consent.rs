//! Custom voices with consent on file.
//!
//! * `GET  /api/v1/voice/custom-voices/consent-text` (Clerk user): the exact
//!   consent statement and its version.
//! * `POST /api/v1/voice/custom-voices` (Clerk user): record the consent
//!   attestation and the reference clip. Creates an active voice.
//! * `GET  /api/v1/voice/custom-voices` (Clerk user): the user's active voices.
//! * `DELETE /api/v1/voice/custom-voices/{id}` (Clerk user): revoke. Deletes the
//!   clip, and bots that used the voice fall back to the default voice.
//! * `GET /api/v1/voice/custom-voices/{id}/consent?owner=` and `/clip?owner=`
//!   (voice service, bearer `ALLTERNIT_VOICE_WORKER_TOKEN`): the consent check
//!   and the clip. 404 unless the voice is active and owned by `owner`.
//!
//! Hard rule (CLAUDE.md): never clone a voice without documented, on-file
//! permission. The voice service asks these routes before every session and
//! every 20 s, so a revoke takes effect within seconds.

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    auth,
    error::ApiError,
    routes::voice_tickets::require_worker,
    services::{
        r2::{ObjectStore, R2Client, R2Error},
        user_files::BUCKET,
    },
    ApiState,
};

fn clip_key(user_id: &str, id: Uuid) -> String {
    format!("voices/{user_id}/{id}.wav")
}

fn storage_unavailable() -> Response {
    coded(StatusCode::SERVICE_UNAVAILABLE, "storage-unavailable", "Voice storage is not available right now.")
}

/// Bumped whenever the statement changes; stored with every record.
pub const CONSENT_VERSION: &str = "2026-10-03";
pub const CONSENT_TEXT: &str = "I confirm that the voice in this recording is my own, or that the person whose voice it is has agreed that Allternit may copy it for this account's bots to speak with. That person understands the copy will be used to generate speech, and that they (or I) can revoke this at any time in Settings > Voice, which deletes the recording and stops the voice being used. I will not use a voice to impersonate anyone or to deceive anyone.";

pub const MIN_CLIP_SECONDS: f32 = 8.0;
pub const MAX_CLIP_SECONDS: f32 = 24.0;
/// 24 s of 16-bit mono at 24 kHz is 1.15 MB; leave room for 32 kHz clips.
pub const MAX_CLIP_BYTES: usize = 1_600_000;
const MAX_ACTIVE_VOICES: i64 = 10;
/// Body limit for the create route: the clip as base64 plus the form.
const CREATE_BODY_LIMIT: usize = 2_400_000;
pub const BOT_DEFAULT_VOICE: &str = "allternit-default";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v1/voice/custom-voices",
            get(list_voices).post(create_voice).layer(DefaultBodyLimit::max(CREATE_BODY_LIMIT)),
        )
        .route("/api/v1/voice/custom-voices/consent-text", get(consent_text))
        .route("/api/v1/voice/custom-voices/:id", axum::routing::delete(revoke_voice))
        .route("/api/v1/voice/custom-voices/:id/consent", get(worker_consent))
        .route("/api/v1/voice/custom-voices/:id/clip", get(worker_clip))
        .route("/api/v1/admin/voice/custom-voices/backfill-r2", axum::routing::post(backfill_r2))
}

fn coded(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "code": code, "message": message }))).into_response()
}

/// What a WAV header says about a clip.
#[derive(Debug, PartialEq)]
pub struct ClipInfo {
    pub sample_rate: u32,
    pub seconds: f32,
    pub rms: f32,
}

/// Validate a reference clip: RIFF/WAVE, PCM16, mono, 16-48 kHz, 8-24 s, not silence.
pub fn inspect_clip(wav: &[u8]) -> Result<ClipInfo, String> {
    if wav.len() < 44 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("The recording must be a WAV file.".into());
    }
    let (mut i, mut fmt, mut data) = (12usize, None, None);
    while i + 8 <= wav.len() {
        let id = &wav[i..i + 4];
        let len = u32::from_le_bytes([wav[i + 4], wav[i + 5], wav[i + 6], wav[i + 7]]) as usize;
        let body = i + 8;
        let end = body.saturating_add(len).min(wav.len());
        if id == b"fmt " && end - body >= 16 {
            let f = &wav[body..end];
            fmt = Some((
                u16::from_le_bytes([f[0], f[1]]),
                u16::from_le_bytes([f[2], f[3]]),
                u32::from_le_bytes([f[4], f[5], f[6], f[7]]),
                u16::from_le_bytes([f[14], f[15]]),
            ));
        } else if id == b"data" {
            data = Some(&wav[body..end]);
            break;
        }
        i = body.saturating_add(len).saturating_add(len & 1);
    }
    let ((format, channels, rate, bits), data) = fmt.zip(data).ok_or("The WAV file is incomplete.")?;
    if format != 1 || bits != 16 || channels != 1 {
        return Err("The recording must be 16-bit mono PCM.".into());
    }
    if !(16_000..=48_000).contains(&rate) {
        return Err("The recording's sample rate must be between 16 and 48 kHz.".into());
    }
    let samples = data.len() / 2;
    let seconds = samples as f32 / rate as f32;
    if seconds < MIN_CLIP_SECONDS {
        return Err(format!("The recording is too short ({seconds:.1} s). Record at least {MIN_CLIP_SECONDS:.0} s."));
    }
    if seconds > MAX_CLIP_SECONDS {
        return Err(format!("The recording is too long ({seconds:.1} s). Keep it under {MAX_CLIP_SECONDS:.0} s."));
    }
    let energy: f64 = data
        .chunks_exact(2)
        .map(|c| {
            let s = i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0;
            s * s
        })
        .sum();
    let rms = (energy / samples as f64).sqrt() as f32;
    if rms < 0.005 {
        return Err("The recording is silent or too quiet. Check the microphone and try again.".into());
    }
    Ok(ClipInfo { sample_rate: rate, seconds, rms })
}

fn client_ip(headers: &HeaderMap) -> Option<String> {
    let first = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    first("x-forwarded-for").or_else(|| first("x-real-ip"))
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(300).collect())
}

async fn consent_text(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiError> {
    auth::resolve_user_id(&state.db, &headers).await?;
    Ok(Json(json!({
        "version": CONSENT_VERSION,
        "text": CONSENT_TEXT,
        "minSeconds": MIN_CLIP_SECONDS,
        "maxSeconds": MAX_CLIP_SECONDS,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVoice {
    name: String,
    speaker_name: String,
    /// `self` or `authorized`.
    relationship: String,
    consent_version: String,
    consent_accepted: bool,
    /// Standard base64 of the WAV file.
    clip_base64: String,
}

pub(crate) async fn create_for_user(
    state: &ApiState,
    user_id: &str,
    headers: &HeaderMap,
    body: CreateVoice,
    store: Option<&dyn ObjectStore>,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let name = body.name.trim();
    let speaker = body.speaker_name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError::BadRequest("Give the voice a name (60 characters max).".into()));
    }
    if speaker.is_empty() || speaker.chars().count() > 120 {
        return Err(ApiError::BadRequest("Say whose voice this is (120 characters max).".into()));
    }
    if !matches!(body.relationship.as_str(), "self" | "authorized") {
        return Err(ApiError::BadRequest("relationship must be \"self\" or \"authorized\".".into()));
    }
    if !body.consent_accepted {
        return Ok(coded(StatusCode::UNPROCESSABLE_ENTITY, "consent-required", "The consent statement must be accepted."));
    }
    if body.consent_version != CONSENT_VERSION {
        return Ok(coded(
            StatusCode::CONFLICT,
            "consent-text-changed",
            "The consent statement has been updated. Reload and review it again.",
        ));
    }
    if body.clip_base64.len() > MAX_CLIP_BYTES * 4 / 3 + 16 {
        return Ok(coded(StatusCode::PAYLOAD_TOO_LARGE, "clip-too-large", "The recording is too large."));
    }
    let wav = STANDARD
        .decode(body.clip_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("clipBase64 is not valid base64.".into()))?;
    if wav.len() > MAX_CLIP_BYTES {
        return Ok(coded(StatusCode::PAYLOAD_TOO_LARGE, "clip-too-large", "The recording is too large."));
    }
    let info = match inspect_clip(&wav) {
        Ok(i) => i,
        Err(message) => return Ok(coded(StatusCode::UNPROCESSABLE_ENTITY, "bad-clip", &message)),
    };
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM voice_custom_voices WHERE user_id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    if active >= MAX_ACTIVE_VOICES {
        return Ok(coded(
            StatusCode::CONFLICT,
            "too-many-voices",
            "You have reached the limit of custom voices. Revoke one to add another.",
        ));
    }
    let Some(store) = store else {
        return Ok(storage_unavailable());
    };
    let id = Uuid::new_v4();
    let sha = hex::encode(Sha256::digest(&wav));
    let key = clip_key(user_id, id);
    match store.put(BUCKET, &key, wav.clone(), "audio/wav").await {
        Ok(()) => {}
        Err(R2Error::Unavailable) => return Ok(storage_unavailable()),
        Err(e) => {
            tracing::warn!(error = %e, "custom voice: clip upload failed");
            return Ok(coded(StatusCode::BAD_GATEWAY, "storage-error", "The recording could not be saved. Try again."));
        }
    }
    let inserted = sqlx::query(
        r#"INSERT INTO voice_custom_voices
           (id, user_id, name, speaker_name, relationship, consent_version, consent_text,
            consent_accepted_at, consent_ip, consent_user_agent,
            clip_sha256, clip_bytes, clip_seconds, clip_key)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(speaker)
    .bind(&body.relationship)
    .bind(CONSENT_VERSION)
    .bind(CONSENT_TEXT)
    .bind(now)
    .bind(client_ip(headers))
    .bind(user_agent(headers))
    .bind(&sha)
    .bind(wav.len() as i32)
    .bind(info.seconds)
    .bind(&key)
    .execute(&state.db)
    .await
    .map_err(|e| {
        // Do not leave an unrecorded object behind; the delete is best effort.
        tracing::warn!(error = %e, "custom voice: row insert failed after upload");
        e
    });
    if let Err(e) = inserted {
        let _ = store.delete(BUCKET, &key).await;
        return Err(e.into());
    }
    Ok((StatusCode::CREATED, Json(voice_json(id, name, speaker, &body.relationship, info.seconds, &sha, now))).into_response())
}

fn voice_json(id: Uuid, name: &str, speaker: &str, relationship: &str, seconds: f32, sha: &str, at: DateTime<Utc>) -> serde_json::Value {
    json!({
        "id": id,
        "voiceId": format!("custom:{id}"),
        "name": name,
        "speakerName": speaker,
        "relationship": relationship,
        "clipSeconds": seconds,
        "clipSha256": sha,
        "consentVersion": CONSENT_VERSION,
        "createdAt": at.to_rfc3339(),
    })
}

async fn create_voice(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<CreateVoice>,
) -> Result<Response, ApiError> {
    let user_id = auth::resolve_user_id(&state.db, &headers).await?;
    let r2 = R2Client::from_env();
    create_for_user(&state, &user_id, &headers, body, r2.as_ref().ok().map(|c| c as &dyn ObjectStore), Utc::now()).await
}

type ListRow = (Uuid, String, String, String, f32, String, DateTime<Utc>);

pub(crate) async fn list_for_user(state: &ApiState, user_id: &str) -> Result<serde_json::Value, ApiError> {
    let rows: Vec<ListRow> = sqlx::query_as(
        "SELECT id, name, speaker_name, relationship, clip_seconds, clip_sha256, created_at \
         FROM voice_custom_voices WHERE user_id = $1 AND status = 'active' ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    let voices: Vec<_> = rows
        .into_iter()
        .map(|(id, name, speaker, rel, secs, sha, at)| voice_json(id, &name, &speaker, &rel, secs, &sha, at))
        .collect();
    Ok(json!({ "voices": voices }))
}

async fn list_voices(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = auth::resolve_user_id(&state.db, &headers).await?;
    Ok(Json(list_for_user(&state, &user_id).await?))
}

pub(crate) async fn revoke_for_user(state: &ApiState, user_id: &str, id: Uuid, store: Option<&dyn ObjectStore>, now: DateTime<Utc>) -> Result<Response, ApiError> {
    // Revoking deletes the audio in the same statement. Idempotent: an
    // already-revoked voice of this user answers 204 as well.
    let owned: Option<String> = sqlx::query_scalar("SELECT status FROM voice_custom_voices WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    if owned.is_none() {
        return Ok(coded(StatusCode::NOT_FOUND, "not-found", "No such custom voice."));
    }
    sqlx::query(
        "UPDATE voice_custom_voices SET status = 'revoked', clip = NULL, revoked_at = COALESCE(revoked_at, $3) \
         WHERE id = $1 AND user_id = $2 AND status = 'active'",
    )
    .bind(id)
    .bind(user_id)
    .bind(now)
    .execute(&state.db)
    .await?;
    // Bots that spoke with it fall back to the default voice. Best effort:
    // the consent check above already stops the voice being used.
    if let Err(e) = sqlx::query("UPDATE voice_bot_config SET voice_id = $3 WHERE user_id = $1 AND voice_id = $2")
        .bind(user_id)
        .bind(format!("custom:{id}"))
        .bind(BOT_DEFAULT_VOICE)
        .execute(&state.db)
        .await
    {
        tracing::warn!(error = %e, "revoke: could not reset bot voice settings");
    }
    // Delete the stored clip, then drop the reference. A failed delete keeps
    // clip_key so a repeated revoke retries it; the voice is already unusable.
    let key: Option<String> = sqlx::query_scalar("SELECT clip_key FROM voice_custom_voices WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if let Some(key) = key {
        let Some(store) = store else {
            return Ok(storage_unavailable());
        };
        if let Err(e) = store.delete(BUCKET, &key).await {
            tracing::warn!(error = %e, "revoke: could not delete the clip object");
            return Ok(coded(StatusCode::BAD_GATEWAY, "storage-error", "The voice is revoked but its recording could not be deleted yet. Try again."));
        }
        sqlx::query("UPDATE voice_custom_voices SET clip_key = NULL WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&state.db)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn revoke_voice(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user_id = auth::resolve_user_id(&state.db, &headers).await?;
    let r2 = R2Client::from_env();
    revoke_for_user(&state, &user_id, id, r2.as_ref().ok().map(|c| c as &dyn ObjectStore), Utc::now()).await
}

#[derive(Debug, Deserialize)]
struct OwnerQuery {
    owner: String,
}

/// `Some((sha256, name))` only for an active voice owned by `owner`.
async fn active_grant(state: &ApiState, owner: &str, id: Uuid) -> Result<Option<(String, String)>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT clip_sha256, name FROM voice_custom_voices WHERE id = $1 AND user_id = $2 AND status = 'active'",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&state.db)
    .await?)
}

fn not_on_file() -> Response {
    coded(StatusCode::NOT_FOUND, "no-consent", "No consent on file.")
}

pub(crate) async fn worker_consent_for(state: &ApiState, owner: &str, id: Uuid) -> Result<Response, ApiError> {
    Ok(match active_grant(state, owner, id).await? {
        Some((sha, name)) => Json(json!({ "clipSha256": sha, "name": name })).into_response(),
        None => not_on_file(),
    })
}

async fn worker_consent(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<OwnerQuery>,
) -> Result<Response, ApiError> {
    if let Err(response) = require_worker(&headers) {
        return Ok(response);
    }
    worker_consent_for(&state, &q.owner, id).await
}

pub(crate) async fn worker_clip_for(state: &ApiState, owner: &str, id: Uuid, store: Option<&dyn ObjectStore>) -> Result<Response, ApiError> {
    let row: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
        "SELECT clip, clip_key FROM voice_custom_voices WHERE id = $1 AND user_id = $2 AND status = 'active' \
         AND (clip IS NOT NULL OR clip_key IS NOT NULL)",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&state.db)
    .await?;
    let clip = match row {
        Some((Some(clip), _)) => clip,
        Some((None, Some(key))) => {
            let Some(store) = store else {
                return Ok(storage_unavailable());
            };
            match store.get(BUCKET, &key).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => return Ok(not_on_file()),
                Err(R2Error::Unavailable) => return Ok(storage_unavailable()),
                Err(e) => {
                    tracing::warn!(error = %e, "custom voice: could not read the clip object");
                    return Ok(coded(StatusCode::BAD_GATEWAY, "storage-error", "The recording could not be read."));
                }
            }
        }
        _ => return Ok(not_on_file()),
    };
    Ok(([(header::CONTENT_TYPE, "audio/wav"), (header::CACHE_CONTROL, "no-store")], clip).into_response())
}

async fn worker_clip(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<OwnerQuery>,
) -> Result<Response, ApiError> {
    if let Err(response) = require_worker(&headers) {
        return Ok(response);
    }
    let r2 = R2Client::from_env();
    worker_clip_for(&state, &q.owner, id, r2.as_ref().ok().map(|c| c as &dyn ObjectStore)).await
}

/// Moves every remaining bytea clip to R2. Idempotent: rows already holding a
/// `clip_key` are skipped, and each row is only changed after its upload.
pub(crate) async fn backfill_clips(state: &ApiState, store: &dyn ObjectStore) -> Result<serde_json::Value, ApiError> {
    let rows: Vec<(Uuid, String, Vec<u8>)> = sqlx::query_as(
        "SELECT id, user_id, clip FROM voice_custom_voices WHERE clip IS NOT NULL AND clip_key IS NULL AND status = 'active'",
    )
    .fetch_all(&state.db)
    .await?;
    let (mut moved, mut failed) = (0, 0);
    for (id, user, clip) in rows {
        let key = clip_key(&user, id);
        if store.put(BUCKET, &key, clip, "audio/wav").await.is_err() {
            failed += 1;
            continue;
        }
        sqlx::query("UPDATE voice_custom_voices SET clip_key = $2, clip = NULL WHERE id = $1 AND clip_key IS NULL")
            .bind(id)
            .bind(&key)
            .execute(&state.db)
            .await?;
        moved += 1;
    }
    Ok(json!({ "moved": moved, "failed": failed }))
}

async fn backfill_r2(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    if !auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admin only.".to_string()));
    }
    let Ok(r2) = R2Client::from_env() else {
        return Ok(storage_unavailable());
    };
    Ok(Json(backfill_clips(&state, &r2).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use axum::body::to_bytes;
    use serial_test::serial;

    /// A mono PCM16 WAV of `secs` seconds of a 220 Hz tone.
    fn wav(secs: f32, rate: u32, amp: f32) -> Vec<u8> {
        let n = (secs * rate as f32) as usize;
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin() * amp;
            data.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        let mut out = b"RIFF".to_vec();
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    fn body(clip: &[u8]) -> CreateVoice {
        CreateVoice {
            name: "Ada".into(),
            speaker_name: "Ada Lovelace".into(),
            relationship: "self".into(),
            consent_version: CONSENT_VERSION.into(),
            consent_accepted: true,
            clip_base64: STANDARD.encode(clip),
        }
    }

    #[derive(Default)]
    struct FakeStore(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);
    #[async_trait::async_trait]
    impl ObjectStore for FakeStore {
        fn presign_get(&self, _b: &str, _k: &str, _t: std::time::Duration) -> Result<String, R2Error> {
            Ok(String::new())
        }
        async fn head(&self, _b: &str, k: &str) -> Result<Option<u64>, R2Error> {
            Ok(self.0.lock().unwrap().get(k).map(|v| v.len() as u64))
        }
        async fn put(&self, b: &str, k: &str, bytes: Vec<u8>, _c: &str) -> Result<(), R2Error> {
            assert_eq!(b, BUCKET);
            self.0.lock().unwrap().insert(k.to_string(), bytes);
            Ok(())
        }
        async fn get(&self, _b: &str, k: &str) -> Result<Option<Vec<u8>>, R2Error> {
            Ok(self.0.lock().unwrap().get(k).cloned())
        }
        async fn delete(&self, _b: &str, k: &str) -> Result<(), R2Error> {
            self.0.lock().unwrap().remove(k);
            Ok(())
        }
    }

    async fn state() -> Arc<ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/032_voice_custom_voices.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::raw_sql(&include_str!("../../migrations_pg/041_voice_clip_key.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE voice_bot_config (bot_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, voice_id TEXT NOT NULL)")
            .execute(&state.db)
            .await
            .unwrap();
        state
    }

    async fn json_of(r: Response) -> (StatusCode, serde_json::Value) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), 4 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(json!(null)))
    }

    fn headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.9, 10.0.0.1".parse().unwrap());
        h.insert(header::USER_AGENT, "Allternit/1.0 test".parse().unwrap());
        h
    }

    #[test]
    fn inspect_accepts_a_good_clip_and_rejects_bad_ones() {
        let ok = inspect_clip(&wav(12.0, 24_000, 0.3)).unwrap();
        assert_eq!(ok.sample_rate, 24_000);
        assert!((ok.seconds - 12.0).abs() < 0.01);
        assert!(inspect_clip(&wav(3.0, 24_000, 0.3)).unwrap_err().contains("too short"));
        assert!(inspect_clip(&wav(40.0, 24_000, 0.3)).unwrap_err().contains("too long"));
        assert!(inspect_clip(&wav(12.0, 24_000, 0.0)).unwrap_err().contains("silent"));
        assert!(inspect_clip(&wav(12.0, 8_000, 0.3)).unwrap_err().contains("sample rate"));
        assert!(inspect_clip(b"not a wav at all, definitely not a wav file really").unwrap_err().contains("WAV"));
        let mut stereo = wav(12.0, 24_000, 0.3);
        stereo[22] = 2;
        assert!(inspect_clip(&stereo).unwrap_err().contains("mono"));
    }

    #[tokio::test]
    #[serial]
    async fn consent_is_recorded_with_hash_ip_and_the_exact_text_then_revoke_deletes_the_clip() {
        let state = state().await;
        let clip = wav(12.0, 24_000, 0.3);
        let store = FakeStore::default();
        let (status, created) = json_of(create_for_user(&state, "u1", &headers(), body(&clip), Some(&store), Utc::now()).await.unwrap()).await;
        assert_eq!(status, StatusCode::CREATED);
        let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
        let key = clip_key("u1", id);
        assert_eq!(store.0.lock().unwrap().get(&key), Some(&clip), "the clip is in R2 at voices/<user>/<id>.wav");
        let (bytea, stored_key): (Option<Vec<u8>>, Option<String>) = sqlx::query_as("SELECT clip, clip_key FROM voice_custom_voices WHERE id = $1").bind(id).fetch_one(&state.db).await.unwrap();
        assert_eq!((bytea, stored_key), (None, Some(key.clone())));
        assert_eq!(created["voiceId"], format!("custom:{id}"));
        let sha = hex::encode(Sha256::digest(&clip));
        assert_eq!(created["clipSha256"], sha);

        let row: (String, String, String, Option<String>, Option<String>, i32) = sqlx::query_as(
            "SELECT clip_sha256, consent_text, consent_version, consent_ip, consent_user_agent, clip_bytes FROM voice_custom_voices WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(row.0, sha);
        assert_eq!(row.1, CONSENT_TEXT);
        assert_eq!(row.2, CONSENT_VERSION);
        assert_eq!(row.3.as_deref(), Some("203.0.113.9"));
        assert_eq!(row.4.as_deref(), Some("Allternit/1.0 test"));
        assert_eq!(row.5 as usize, clip.len());

        // The voice service's view: consent + the identical clip.
        let (s, g) = json_of(worker_consent_for(&state, "u1", id).await.unwrap()).await;
        assert_eq!((s, g["clipSha256"].as_str()), (StatusCode::OK, Some(sha.as_str())));
        let resp = worker_clip_for(&state, "u1", id, Some(&store)).await.unwrap();
        assert_eq!(to_bytes(resp.into_body(), 4 << 20).await.unwrap().to_vec(), clip);

        // Only the owner can use or list it.
        assert_eq!(worker_consent_for(&state, "u2", id).await.unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(worker_clip_for(&state, "u2", id, Some(&store)).await.unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(list_for_user(&state, "u2").await.unwrap()["voices"].as_array().unwrap().len(), 0);
        assert_eq!(list_for_user(&state, "u1").await.unwrap()["voices"].as_array().unwrap().len(), 1);

        // A bot uses the voice, then the owner revokes.
        sqlx::query("INSERT INTO voice_bot_config (bot_id, user_id, voice_id) VALUES ('b1','u1',$1), ('b2','u1','af_heart'), ('b3','u2',$1)")
            .bind(format!("custom:{id}"))
            .execute(&state.db)
            .await
            .unwrap();
        assert_eq!(revoke_for_user(&state, "u2", id, Some(&store), Utc::now()).await.unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(revoke_for_user(&state, "u1", id, Some(&store), Utc::now()).await.unwrap().status(), StatusCode::NO_CONTENT);
        assert_eq!(revoke_for_user(&state, "u1", id, Some(&store), Utc::now()).await.unwrap().status(), StatusCode::NO_CONTENT, "idempotent");

        // Revoke propagated: consent and clip are gone, the audio is deleted,
        // the list is empty, and this user's bots are back on the default.
        assert_eq!(worker_consent_for(&state, "u1", id).await.unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(worker_clip_for(&state, "u1", id, Some(&store)).await.unwrap().status(), StatusCode::NOT_FOUND);
        let left: (Option<Vec<u8>>, String) = sqlx::query_as("SELECT clip, status FROM voice_custom_voices WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(left, (None, "revoked".to_string()));
        assert!(store.0.lock().unwrap().is_empty(), "revoke deleted the object");
        let key_left: Option<String> = sqlx::query_scalar("SELECT clip_key FROM voice_custom_voices WHERE id = $1").bind(id).fetch_one(&state.db).await.unwrap();
        assert_eq!(key_left, None);
        assert_eq!(list_for_user(&state, "u1").await.unwrap()["voices"].as_array().unwrap().len(), 0);
        let bots: Vec<(String, String)> = sqlx::query_as("SELECT bot_id, voice_id FROM voice_bot_config ORDER BY bot_id")
            .fetch_all(&state.db)
            .await
            .unwrap();
        assert_eq!(bots[0], ("b1".into(), BOT_DEFAULT_VOICE.into()));
        assert_eq!(bots[1], ("b2".into(), "af_heart".into()));
        assert_eq!(bots[2].1, format!("custom:{id}"), "another user's bot is untouched");
    }

    #[tokio::test]
    #[serial]
    async fn creating_without_consent_or_with_a_bad_clip_is_refused_and_stores_nothing() {
        let state = state().await;
        let clip = wav(12.0, 24_000, 0.3);

        let mut b = body(&clip);
        b.consent_accepted = false;
        let (s, j) = json_of(create_for_user(&state, "u1", &headers(), b, Some(&FakeStore::default()), Utc::now()).await.unwrap()).await;
        assert_eq!((s, j["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("consent-required")));

        let mut b = body(&clip);
        b.consent_version = "1999-01-01".into();
        let (s, j) = json_of(create_for_user(&state, "u1", &headers(), b, Some(&FakeStore::default()), Utc::now()).await.unwrap()).await;
        assert_eq!((s, j["code"].as_str()), (StatusCode::CONFLICT, Some("consent-text-changed")));

        let (s, j) = json_of(create_for_user(&state, "u1", &headers(), body(&wav(2.0, 24_000, 0.3)), Some(&FakeStore::default()), Utc::now()).await.unwrap()).await;
        assert_eq!((s, j["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("bad-clip")));

        let mut b = body(&clip);
        b.speaker_name = "  ".into();
        assert!(create_for_user(&state, "u1", &headers(), b, Some(&FakeStore::default()), Utc::now()).await.is_err());
        let mut b = body(&clip);
        b.relationship = "friend-of-a-friend".into();
        assert!(create_for_user(&state, "u1", &headers(), b, Some(&FakeStore::default()), Utc::now()).await.is_err());

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM voice_custom_voices").fetch_one(&state.db).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    #[serial]
    async fn a_user_cannot_hold_more_than_the_voice_limit() {
        let state = state().await;
        let clip = wav(9.0, 24_000, 0.3);
        for _ in 0..MAX_ACTIVE_VOICES {
            let r = create_for_user(&state, "u1", &headers(), body(&clip), Some(&FakeStore::default()), Utc::now()).await.unwrap();
            assert_eq!(r.status(), StatusCode::CREATED);
        }
        let (s, j) = json_of(create_for_user(&state, "u1", &headers(), body(&clip), Some(&FakeStore::default()), Utc::now()).await.unwrap()).await;
        assert_eq!((s, j["code"].as_str()), (StatusCode::CONFLICT, Some("too-many-voices")));
    }

    #[test]
    fn the_worker_routes_demand_the_worker_token() {
        std::env::remove_var("ALLTERNIT_VOICE_WORKER_TOKEN");
        let r = require_worker(&HeaderMap::new()).unwrap_err();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    #[serial]
    async fn backfill_moves_old_bytea_clips_and_is_idempotent() {
        let state = state().await;
        let clip = wav(12.0, 24_000, 0.3);
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO voice_custom_voices (id, user_id, name, speaker_name, relationship, consent_version, consent_text, consent_accepted_at, clip_sha256, clip_bytes, clip_seconds, clip) VALUES ($1,'u1','v','s','self','x','t',now(),'sha',$2,12,$3)")
            .bind(id).bind(clip.len() as i32).bind(&clip).execute(&state.db).await.unwrap();
        let store = FakeStore::default();
        assert_eq!(backfill_clips(&state, &store).await.unwrap(), json!({ "moved": 1, "failed": 0 }));
        assert_eq!(backfill_clips(&state, &store).await.unwrap(), json!({ "moved": 0, "failed": 0 }));
        let resp = worker_clip_for(&state, "u1", id, Some(&store)).await.unwrap();
        assert_eq!(to_bytes(resp.into_body(), 4 << 20).await.unwrap().to_vec(), clip);
        let none: Option<Vec<u8>> = sqlx::query_scalar("SELECT clip FROM voice_custom_voices WHERE id = $1").bind(id).fetch_one(&state.db).await.unwrap();
        assert!(none.is_none());
        assert_eq!(create_for_user(&state, "u1", &headers(), body(&clip), None, Utc::now()).await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
