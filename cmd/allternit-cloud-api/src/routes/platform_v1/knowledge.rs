//! `/v1/agents/{id}/knowledge`: knowledge files for a hosted agent (spec §5,
//! the `knowledge_search` tool). Scope `agents`; an account-bound key sees only
//! its own account's agents, so only their files.
//!
//! * `POST   /v1/agents/{id}/knowledge`            upload one file (JSON: `name`, `content` or `content_base64`, `content_type`)
//! * `GET    /v1/agents/{id}/knowledge`            list the agent's files
//! * `DELETE /v1/agents/{id}/knowledge/{file_id}`  remove one
//!
//! Limits (documented in the Agents guide): text formats only ([`CONTENT_TYPES`]),
//! UTF-8, at most [`MAX_FILE_BYTES`] per file, [`MAX_FILES`] files and
//! [`MAX_AGENT_BYTES`] in total per agent. The original bytes are kept in R2;
//! the text is split into chunks of about [`CHUNK_CHARS`] characters with a
//! Postgres full-text index, which [`search`] ranks with `ts_rank_cd`.
//!
//! The agent searches its own files through the runtime tool route
//! (`agent_tools.rs`); this module only owns storage and the search itself.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use super::{agents, build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable};
use crate::services::r2::{ObjectStore, R2Client};
use crate::ApiState;

pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_FILES: i64 = 20;
pub const MAX_AGENT_BYTES: i64 = 5 * 1024 * 1024;
pub const CHUNK_CHARS: usize = 1200;
const MAX_NAME: usize = 200;
/// Results one search returns at most.
pub const MAX_RESULTS: i64 = 10;
pub const CONTENT_TYPES: [&str; 5] = ["text/plain", "text/markdown", "text/csv", "text/html", "application/json"];
const BUCKET: &str = crate::services::user_files::BUCKET;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/agents/:id/knowledge", &["GET", "POST"], get(list_files).post(upload_file))
        .add("/v1/agents/:id/knowledge/:file_id", &["DELETE"], delete(delete_file))
}

/// The object store for a request: a test's fake when one is layered, else R2 (if configured).
pub type StoreExt = Option<Extension<Arc<dyn ObjectStore>>>;

fn store_for(layered: StoreExt) -> Result<Arc<dyn ObjectStore>, PlatformError> {
    if let Some(Extension(s)) = layered {
        return Ok(s);
    }
    R2Client::from_env()
        .map(|c| Arc::new(c) as Arc<dyn ObjectStore>)
        .map_err(|_| PlatformError::api_error("knowledge_storage_unavailable", "Knowledge file storage isn't configured on this deployment."))
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct KnowledgeFile {
    pub id: String,
    #[sqlx(skip)]
    pub object: &'static str,
    pub agent_id: String,
    pub account_id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: i32,
    pub chunk_count: i32,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, agent_id, account_id, name, content_type, bytes, chunk_count, created_at";

fn with_object(mut f: KnowledgeFile) -> KnowledgeFile {
    f.object = "knowledge_file";
    f
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadBody {
    name: String,
    content_type: Option<String>,
    /// The file as text.
    content: Option<String>,
    /// Or the file's bytes, base64 (must decode to UTF-8 text).
    content_base64: Option<String>,
}

fn invalid(code: &str, message: impl Into<String>, param: &str) -> PlatformError {
    PlatformError::invalid_request(code, message.into()).with_param(param)
}

/// The content type from the field or the file name's extension.
pub fn content_type_for(name: &str, given: Option<&str>) -> Result<String, PlatformError> {
    let given = given.map(|t| t.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).filter(|t| !t.is_empty());
    let t = match given {
        Some(t) => t,
        None => match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
            Some("txt") | Some("text") => "text/plain".into(),
            Some("md") | Some("markdown") => "text/markdown".into(),
            Some("csv") => "text/csv".into(),
            Some("html") | Some("htm") => "text/html".into(),
            Some("json") => "application/json".into(),
            _ => "text/plain".into(),
        },
    };
    if !CONTENT_TYPES.contains(&t.as_str()) {
        return Err(invalid(
            "unsupported_content_type",
            format!("Knowledge files must be one of {}. Convert PDFs and Office files to text first.", CONTENT_TYPES.join(", ")),
            "content_type",
        ));
    }
    Ok(t)
}

/// HTML to readable text: drop scripts, styles and tags, decode the common entities.
fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            for skip in ["script", "style"] {
                if lower[i + 1..].starts_with(skip) {
                    if let Some(end) = lower[i..].find(&format!("</{skip}")) {
                        i += end;
                    }
                }
            }
            match html[i..].find('>') {
                Some(end) => {
                    out.push(' ');
                    i += end + 1;
                }
                None => break,
            }
        } else {
            let next = html[i..].find('<').map(|n| i + n).unwrap_or(html.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
}

/// Split text into chunks of about `CHUNK_CHARS` characters, on paragraph,
/// then line, then word boundaries. Whitespace-only pieces are dropped.
pub fn chunk(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, chunks: &mut Vec<String>| {
        let t = current.trim();
        if !t.is_empty() {
            chunks.push(t.to_string());
        }
        current.clear();
    };
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if current.chars().count() + para.chars().count() + 2 > CHUNK_CHARS {
            flush(&mut current, &mut chunks);
        }
        if para.chars().count() <= CHUNK_CHARS {
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(para);
            continue;
        }
        // A long paragraph: split on words.
        for word in para.split_whitespace() {
            if current.chars().count() + word.chars().count() + 1 > CHUNK_CHARS {
                flush(&mut current, &mut chunks);
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }
    flush(&mut current, &mut chunks);
    chunks
}

async fn upload_file(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    store: StoreExt,
    Path(agent_id): Path<String>,
    ApiJson(body): ApiJson<UploadBody>,
) -> Result<(StatusCode, Json<KnowledgeFile>), PlatformError> {
    caller.require("agents")?;
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(invalid("invalid_name", format!("name must be 1 to {MAX_NAME} characters."), "name"));
    }
    let content_type = content_type_for(&name, body.content_type.as_deref())?;
    let raw: Vec<u8> = match (body.content, body.content_base64) {
        (Some(text), None) => text.into_bytes(),
        (None, Some(b64)) => {
            use base64::Engine as _;
            if b64.len() / 4 * 3 > MAX_FILE_BYTES + 3 {
                return Err(file_too_large());
            }
            let compact: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(compact.as_bytes())
                .map_err(|_| invalid("invalid_content", "content_base64 is not valid base64.", "content_base64"))?
        }
        _ => return Err(invalid("invalid_content", "Send exactly one of content (text) or content_base64.", "content")),
    };
    if raw.is_empty() {
        return Err(invalid("invalid_content", "The file is empty.", "content"));
    }
    if raw.len() > MAX_FILE_BYTES {
        return Err(file_too_large());
    }
    let text = String::from_utf8(raw.clone()).map_err(|_| invalid("invalid_content", "Knowledge files must be UTF-8 text.", "content"))?;
    let text = if content_type == "text/html" { html_text(&text) } else { text };
    let chunks = chunk(&text);
    if chunks.is_empty() {
        return Err(invalid("invalid_content", "The file has no text to search.", "content"));
    }

    let mut tx = state.db.begin().await?;
    // One upload at a time per agent, so the limits can't be raced past.
    sqlx::query("SELECT id FROM platform_agents WHERE id = $1 FOR UPDATE").bind(&agent.id).execute(&mut *tx).await?;
    let (files, total): (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(bytes), 0)::bigint FROM platform_knowledge_files WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(&agent.id)
    .fetch_one(&mut *tx)
    .await?;
    if files >= MAX_FILES {
        return Err(PlatformError::permission("knowledge_limit_reached", format!("An agent can have {MAX_FILES} knowledge files. Delete one first.")));
    }
    if total + raw.len() as i64 > MAX_AGENT_BYTES {
        return Err(PlatformError::permission("knowledge_limit_reached", "An agent's knowledge files can total 5 MB. Delete a file first."));
    }

    let id = new_id("kf_");
    let storage_key = format!("platform/knowledge/{}/{}/{}", caller.project_id, agent.id, id);
    let store = store_for(store)?;
    store
        .put(BUCKET, &storage_key, raw.clone(), &content_type)
        .await
        .map_err(|e| {
            tracing::warn!(file = %id, "platform knowledge: storing the file failed: {e}");
            PlatformError::api_error("knowledge_storage_unavailable", "The file couldn't be stored. Retry shortly.")
        })?;
    let row = sqlx::query_as::<_, KnowledgeFile>(&format!(
        "INSERT INTO platform_knowledge_files (id, project_id, account_id, agent_id, name, content_type, bytes, sha256, storage_key, chunk_count) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&agent.account_id)
    .bind(&agent.id)
    .bind(&name)
    .bind(&content_type)
    .bind(raw.len() as i32)
    .bind(hex::encode(Sha256::digest(&raw)))
    .bind(&storage_key)
    .bind(chunks.len() as i32)
    .fetch_one(&mut *tx)
    .await?;
    for (ord, content) in chunks.iter().enumerate() {
        sqlx::query("INSERT INTO platform_knowledge_chunks (file_id, agent_id, ord, content) VALUES ($1, $2, $3, $4)")
            .bind(&id)
            .bind(&agent.id)
            .bind(ord as i32)
            .bind(content)
            .execute(&mut *tx)
            .await?;
    }
    if let Err(e) = tx.commit().await {
        let _ = store.delete(BUCKET, &storage_key).await;
        return Err(e.into());
    }
    Ok((StatusCode::CREATED, Json(with_object(row))))
}

fn file_too_large() -> PlatformError {
    invalid("file_too_large", "Knowledge files can be at most 1 MB.", "content")
}

async fn list_files(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(agent_id): Path<String>,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Page<KnowledgeFile>>, PlatformError> {
    caller.require("agents")?;
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let limit = page.limit()?;
    let (after_at, after_id) = match page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, KnowledgeFile>(&format!(
        "SELECT {COLUMNS} FROM platform_knowledge_files WHERE agent_id = $1 AND deleted_at IS NULL \
           AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3)) ORDER BY created_at, id LIMIT $4"
    ))
    .bind(&agent.id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let files: Vec<KnowledgeFile> = rows.into_iter().map(with_object).collect();
    Ok(Json(build_page(files, limit, |f| (f.created_at, f.id.clone()))))
}

async fn delete_file(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    store: StoreExt,
    Path((agent_id, file_id)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let gone: Option<(String,)> = sqlx::query_as(
        "UPDATE platform_knowledge_files SET deleted_at = NOW() WHERE id = $1 AND agent_id = $2 AND deleted_at IS NULL RETURNING storage_key",
    )
    .bind(&file_id)
    .bind(&agent.id)
    .fetch_optional(&state.db)
    .await?;
    let Some((storage_key,)) = gone else {
        return Err(PlatformError::not_found("knowledge_file_not_found", "No such knowledge file."));
    };
    sqlx::query("DELETE FROM platform_knowledge_chunks WHERE file_id = $1").bind(&file_id).execute(&state.db).await?;
    // Best effort: the row is gone either way, so the agent can't search it.
    match store_for(store) {
        Ok(s) => {
            if let Err(e) = s.delete(BUCKET, &storage_key).await {
                tracing::warn!(file = %file_id, "platform knowledge: deleting the stored file failed: {e}");
            }
        }
        Err(_) => tracing::warn!(file = %file_id, "platform knowledge: no object store to delete the file from"),
    }
    Ok(Json(json!({ "id": file_id, "object": "knowledge_file", "deleted": true })))
}

/// One search hit.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Hit {
    pub file_id: String,
    pub file_name: String,
    pub content: String,
    pub score: f32,
}

/// Words for an any-word query: letters and digits only, so nothing reaches `to_tsquery` unescaped.
fn or_query(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .take(32)
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Search one agent's files: all words first (`websearch_to_tsquery`), then
/// any word when that finds nothing. Ranked by `ts_rank_cd`.
pub async fn search(db: &PgPool, agent_id: &str, query: &str, limit: i64) -> Result<Vec<Hit>, sqlx::Error> {
    let limit = limit.clamp(1, MAX_RESULTS);
    let sql = |tsquery: &str| {
        format!(
            "SELECT f.id AS file_id, f.name AS file_name, c.content, ts_rank_cd(c.tsv, q)::real AS score \
             FROM platform_knowledge_chunks c JOIN platform_knowledge_files f ON f.id = c.file_id, {tsquery} q \
             WHERE c.agent_id = $1 AND f.agent_id = $1 AND f.deleted_at IS NULL AND c.tsv @@ q \
             ORDER BY score DESC, f.id, c.ord LIMIT $3"
        )
    };
    let hits = sqlx::query_as::<_, Hit>(&sql("websearch_to_tsquery('english', $2)"))
        .bind(agent_id)
        .bind(query)
        .bind(limit)
        .fetch_all(db)
        .await?;
    if !hits.is_empty() {
        return Ok(hits);
    }
    let any = or_query(query);
    if any.is_empty() {
        return Ok(vec![]);
    }
    sqlx::query_as::<_, Hit>(&sql("to_tsquery('english', $2)")).bind(agent_id).bind(any).bind(limit).fetch_all(db).await
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn chunks_respect_the_size_and_keep_paragraphs() {
        assert_eq!(chunk("one\n\ntwo"), vec!["one\n\ntwo"]);
        let long = "word ".repeat(1000);
        let parts = chunk(&long);
        assert!(parts.len() > 3 && parts.iter().all(|p| p.chars().count() <= CHUNK_CHARS));
        assert!(chunk("  \n\n  ").is_empty());
    }

    #[test]
    fn only_text_types_are_accepted() {
        assert_eq!(content_type_for("faq.md", None).unwrap(), "text/markdown");
        assert_eq!(content_type_for("x", Some("text/plain; charset=utf-8")).unwrap(), "text/plain");
        assert!(matches!(content_type_for("a.pdf", Some("application/pdf")), Err(e) if e.code == "unsupported_content_type"));
    }

    #[test]
    fn html_becomes_text_and_queries_are_sanitized() {
        let t = html_text("<html><style>p{}</style><p>Open &amp; ready</p><script>evil()</script></html>");
        assert!(t.contains("Open & ready") && !t.contains("evil") && !t.contains("p{}"));
        assert_eq!(or_query("hours? (Sunday) & 'x' | !"), "hours | Sunday");
    }
}
