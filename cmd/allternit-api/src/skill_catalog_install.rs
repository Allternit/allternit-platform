//! One-click install for the skill directory in Customize → Skills.
//!
//! The directory lists skills by id; this installs the real skill folder
//! (SKILL.md plus its scripts and references) into `~/.agents/skills/<name>`,
//! a root gizzi-code scans, then asks gizzi to rescan so the skill is usable
//! in the next turn.
//!
//! Sources:
//! - Allternit's own skills ship inside this binary (`.agents/skills/…`).
//! - Anthropic's public skills are fetched from `anthropics/skills` when the
//!   user installs them (never bundled: several are source-available, not
//!   redistributable).
//!
//! Every install writes `.allternit-install.json`; uninstall only removes
//! folders carrying that marker, so a skill the user wrote by hand under the
//! same name is never overwritten or deleted.
//!
//! Versions: the marker records where the files came from (for GitHub, the
//! exact commit the ref resolved to), a content hash, and the SKILL.md
//! `version` when it has one. Installing over a managed copy with different
//! content moves the old copy to `~/.allternit/skill-versions/<name>/<id>`
//! (newest 10 kept), so Customize can list versions and roll back.
//! Reinstalling identical content changes nothing and reports `unchanged`.

use axum::{
    extract::Path as AxPath,
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::AppState;

static ALABS_COURSE_PIPELINE: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../.agents/skills/alabs-course-pipeline");
static CODEBASE_TO_COURSE: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../.agents/skills/allternit-codebase-to-course");
static CLONE_WEBSITE: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../.agents/skills/clone-website");
static MOBILE_APP_DESIGN: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../.agents/skills/mobile-app-design");

const MARKER: &str = ".allternit-install.json";
const ANTHROPIC_REPO: &str = "anthropics/skills";
const ANTHROPIC_REF: &str = "main";
/// Largest single file / whole skill we will write.
const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
const MAX_SKILL_BYTES: usize = 25 * 1024 * 1024;
/// Archived versions kept per skill (the current install is extra).
const MAX_ARCHIVED_VERSIONS: usize = 10;

enum Source {
    Embedded(&'static Dir<'static>),
    GitHub { repo: &'static str, git_ref: &'static str, path: String },
}

/// Directory id → (installed folder name, source).
fn resolve(id: &str) -> Option<(String, Source)> {
    let embedded = |name: &str, dir: &'static Dir<'static>| Some((name.to_string(), Source::Embedded(dir)));
    match id {
        "allternit-alabs-course-pipeline" => embedded("alabs-course-pipeline", &ALABS_COURSE_PIPELINE),
        "allternit-allternit-codebase-to-course" => embedded("allternit-codebase-to-course", &CODEBASE_TO_COURSE),
        "codex-clone-website" => embedded("clone-website", &CLONE_WEBSITE),
        "codex-mobile-app-design" => embedded("mobile-app-design", &MOBILE_APP_DESIGN),
        _ => {
            let name = id.strip_prefix("claude-")?;
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return None;
            }
            Some((
                name.to_string(),
                Source::GitHub { repo: ANTHROPIC_REPO, git_ref: ANTHROPIC_REF, path: format!("skills/{name}") },
            ))
        }
    }
}

fn skills_root() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ALLTERNIT_SKILLS_INSTALL_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(".agents").join("skills"))
}

/// A repo-relative path is safe to join under the skill folder.
fn safe_relative(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if rel.is_empty() || p.is_absolute() {
        return None;
    }
    p.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| p.to_path_buf())
}

/// Where an install's files came from, recorded in its marker.
fn source_info(source: &Source, commit: Option<&str>) -> Value {
    match source {
        Source::Embedded(_) => json!({ "kind": "embedded", "build": env!("CARGO_PKG_VERSION") }),
        Source::GitHub { repo, git_ref, path } => json!({ "kind": "github", "repo": repo, "ref": git_ref, "path": path, "commit": commit }),
    }
}

/// Files (relative path → bytes) for one skill, plus the commit a GitHub ref
/// resolved to (the listing and every download use that commit, so the files
/// are one consistent snapshot).
async fn collect_files(source: &Source) -> Result<(Vec<(PathBuf, Vec<u8>)>, Option<String>), String> {
    match source {
        Source::Embedded(dir) => {
            let mut out = Vec::new();
            fn walk(dir: &Dir<'_>, out: &mut Vec<(PathBuf, Vec<u8>)>) {
                for f in dir.files() {
                    out.push((f.path().to_path_buf(), f.contents().to_vec()));
                }
                for d in dir.dirs() {
                    walk(d, out);
                }
            }
            walk(dir, &mut out);
            // include_dir paths are relative to the embedded root already.
            Ok((out, None))
        }
        Source::GitHub { repo, git_ref, path } => {
            let client = reqwest::Client::builder()
                .user_agent("allternit-desktop")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())?;
            let commit: Value = client
                .get(format!("https://api.github.com/repos/{repo}/commits/{git_ref}"))
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| format!("GitHub unreachable: {e}"))?
                .error_for_status()
                .map_err(|e| format!("GitHub refused {git_ref}: {e}"))?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let sha = commit
                .get("sha")
                .and_then(|v| v.as_str())
                .filter(|s| s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit()))
                .ok_or_else(|| format!("GitHub returned no commit for {git_ref}"))?
                .to_string();
            let tree: Value = client
                .get(format!("https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1"))
                .send()
                .await
                .map_err(|e| format!("GitHub unreachable: {e}"))?
                .error_for_status()
                .map_err(|e| format!("GitHub refused the listing: {e}"))?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let prefix = format!("{path}/");
            let blobs: Vec<String> = tree
                .get("tree")
                .and_then(|t| t.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("blob"))
                        .filter_map(|i| i.get("path").and_then(|p| p.as_str()))
                        .filter(|p| p.starts_with(&prefix))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if !blobs.iter().any(|b| b == &format!("{path}/SKILL.md")) {
                return Err(format!("{repo} has no skill at {path}"));
            }
            let mut out = Vec::new();
            let mut total = 0usize;
            for blob in blobs {
                let bytes = client
                    .get(format!("https://raw.githubusercontent.com/{repo}/{sha}/{blob}"))
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("download failed for {blob}: {e}"))?
                    .bytes()
                    .await
                    .map_err(|e| e.to_string())?;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(format!("{blob} is larger than {MAX_FILE_BYTES} bytes"));
                }
                total += bytes.len();
                if total > MAX_SKILL_BYTES {
                    return Err("skill is larger than the install limit".into());
                }
                out.push((PathBuf::from(&blob[prefix.len()..]), bytes.to_vec()));
            }
            Ok((out, Some(sha)))
        }
    }
}

fn marker_of(dir: &Path) -> Option<Value> {
    std::fs::read_to_string(dir.join(MARKER)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn versions_root() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ALLTERNIT_SKILL_VERSIONS_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(".allternit").join("skill-versions"))
}

/// Stable hash of a skill's files (sorted paths + bytes).
pub fn content_hash(files: &[(PathBuf, Vec<u8>)]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<&(PathBuf, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    for (p, bytes) in sorted {
        h.update(p.to_string_lossy().as_bytes());
        h.update([0u8]);
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    hex::encode(h.finalize())
}

/// `version:` from SKILL.md frontmatter, when the skill declares one.
pub fn declared_version(files: &[(PathBuf, Vec<u8>)]) -> Option<String> {
    let (_, bytes) = files.iter().find(|(p, _)| p == Path::new("SKILL.md"))?;
    let text = std::str::from_utf8(bytes).ok()?;
    let body = text.strip_prefix("---")?;
    let front = &body[..body.find("\n---")?];
    front.lines().find_map(|line| {
        let v = line.trim().strip_prefix("version:")?.trim().trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Version ids are UTC timestamps; nothing else is accepted as a path part.
fn valid_version_id(v: &str) -> bool {
    !v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_ascii_digit() || c == 'T' || c == 'Z')
}

fn new_version_id() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string()
}

/// The version id recorded in a marker (older markers predate ids: derive one
/// from installedAt so they can still be archived and listed).
fn marker_version_id(marker: &Value) -> String {
    if let Some(v) = marker.get("versionId").and_then(|v| v.as_str()).filter(|v| valid_version_id(v)) {
        return v.to_string();
    }
    marker
        .get("installedAt")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc).format("%Y%m%dT%H%M%S%3fZ").to_string())
        .unwrap_or_else(new_version_id)
}

/// Move a managed install into the version archive and prune old versions.
fn archive_current(dest: &Path, versions: &Path, name: &str, marker: &Value) -> std::io::Result<()> {
    let dir = versions.join(name);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(marker_version_id(marker));
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(dest, &target)?;
    let mut ids: Vec<String> = std::fs::read_dir(&dir)?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| valid_version_id(n))
        .collect();
    ids.sort();
    while ids.len() > MAX_ARCHIVED_VERSIONS {
        let oldest = ids.remove(0);
        let _ = std::fs::remove_dir_all(dir.join(oldest));
    }
    Ok(())
}

#[derive(Debug)]
pub struct WriteOutcome {
    pub path: PathBuf,
    /// The managed copy already had exactly these files; nothing changed.
    pub unchanged: bool,
    pub marker: Value,
}

/// Write the files into `<root>/<name>`. A previous managed install with
/// different content is archived under `<versions>/<name>/`; identical
/// content is left as is.
pub fn write_skill(
    root: &Path,
    versions: &Path,
    id: &str,
    name: &str,
    files: &[(PathBuf, Vec<u8>)],
    source: Value,
) -> Result<WriteOutcome, (StatusCode, String)> {
    if !files.iter().any(|(p, _)| p == Path::new("SKILL.md")) {
        return Err((StatusCode::BAD_GATEWAY, "source has no SKILL.md".into()));
    }
    let io = |e: std::io::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let hash = content_hash(files);
    let dest = root.join(name);
    if dest.exists() {
        let Some(existing) = marker_of(&dest) else {
            return Err((
                StatusCode::CONFLICT,
                format!("{} already exists and wasn't installed by Allternit; leaving it alone", dest.display()),
            ));
        };
        if existing.get("contentHash").and_then(|v| v.as_str()) == Some(hash.as_str()) {
            return Ok(WriteOutcome { path: dest, unchanged: true, marker: existing });
        }
    }
    // Stage then swap, so a failed download never leaves half a skill.
    let staging = root.join(format!(".{name}.installing"));
    let _ = std::fs::remove_dir_all(&staging);
    for (rel, bytes) in files {
        let rel_str = rel.to_string_lossy();
        let rel = safe_relative(&rel_str).ok_or((StatusCode::BAD_GATEWAY, format!("unsafe path in source: {rel_str}")))?;
        let target = staging.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&target, bytes).map_err(io)?;
    }
    let marker = json!({
        "id": id,
        "versionId": new_version_id(),
        "installedAt": chrono::Utc::now().to_rfc3339(),
        "files": files.len(),
        "contentHash": hash,
        "version": declared_version(files),
        "source": source,
    });
    std::fs::write(staging.join(MARKER), marker.to_string()).map_err(io)?;
    if let Some(existing) = marker_of(&dest) {
        archive_current(&dest, versions, name, &existing).map_err(io)?;
    }
    std::fs::rename(&staging, &dest).map_err(io)?;
    Ok(WriteOutcome { path: dest, unchanged: false, marker })
}

/// Current install first, then archived versions newest first.
pub fn list_versions(root: &Path, versions: &Path, id: &str, name: &str) -> Vec<Value> {
    let entry = |marker: Value, current: bool| {
        json!({
            "versionId": marker_version_id(&marker),
            "version": marker.get("version").cloned().unwrap_or(Value::Null),
            "installedAt": marker.get("installedAt").cloned().unwrap_or(Value::Null),
            "source": marker.get("source").cloned().unwrap_or(Value::Null),
            "contentHash": marker.get("contentHash").cloned().unwrap_or(Value::Null),
            "current": current,
        })
    };
    let owned = |m: &Value| m.get("id").and_then(|v| v.as_str()) == Some(id);
    let mut out = Vec::new();
    if let Some(m) = marker_of(&root.join(name)).filter(owned) {
        out.push(entry(m, true));
    }
    let mut archived: Vec<(String, Value)> = std::fs::read_dir(versions.join(name))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let dir_name = e.file_name().to_str()?.to_string();
            let m = marker_of(&e.path()).filter(owned)?;
            valid_version_id(&dir_name).then_some((dir_name, m))
        })
        .collect();
    archived.sort_by(|a, b| b.0.cmp(&a.0));
    out.extend(archived.into_iter().map(|(_, m)| entry(m, false)));
    out
}

/// Make an archived version current again; the current copy is archived.
pub fn restore_version(root: &Path, versions: &Path, id: &str, name: &str, version_id: &str) -> Result<Value, (StatusCode, String)> {
    let io = |e: std::io::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    if !valid_version_id(version_id) {
        return Err((StatusCode::BAD_REQUEST, "bad version id".into()));
    }
    let source = versions.join(name).join(version_id);
    let Some(marker) = marker_of(&source).filter(|m| m.get("id").and_then(|v| v.as_str()) == Some(id)) else {
        return Err((StatusCode::NOT_FOUND, format!("{id} has no version {version_id}")));
    };
    let dest = root.join(name);
    match marker_of(&dest) {
        Some(current) if current.get("id").and_then(|v| v.as_str()) == Some(id) => {
            if marker_version_id(&current) == version_id {
                return Ok(current);
            }
            archive_current(&dest, versions, name, &current).map_err(io)?;
        }
        Some(_) | None if dest.exists() => {
            return Err((StatusCode::CONFLICT, format!("{} isn't managed by Allternit", dest.display())));
        }
        _ => {}
    }
    std::fs::rename(&source, &dest).map_err(io)?;
    Ok(marker)
}

/// Ask gizzi-code to rescan skill roots. Best effort: a restart also picks
/// the skill up.
async fn reload_gizzi_skills(headers: &HeaderMap) -> bool {
    crate::agent_session_routes::gizzi_client(headers)
        .post(format!("{}/v1/skill/reload", crate::v1_routes::gizzi_base()))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn fail(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": "skill_install_failed", "message": message.into() })))
}

#[derive(Deserialize)]
struct InstallBody {
    id: String,
}

async fn install(headers: HeaderMap, Json(body): Json<InstallBody>) -> (StatusCode, Json<Value>) {
    let Some((name, source)) = resolve(&body.id) else {
        return fail(StatusCode::NOT_FOUND, format!("no installable skill with id {}", body.id));
    };
    let Some(root) = skills_root() else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "no home directory");
    };
    if let Err(e) = std::fs::create_dir_all(&root) {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let Some(versions) = versions_root() else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "no home directory");
    };
    let (files, commit) = match collect_files(&source).await {
        Ok(f) => f,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let info = source_info(&source, commit.as_deref());
    let id = body.id.clone();
    let name_for_write = name.clone();
    let written = tokio::task::spawn_blocking(move || write_skill(&root, &versions, &id, &name_for_write, &files, info)).await;
    match written {
        Ok(Ok(outcome)) => {
            let reloaded = if outcome.unchanged { false } else { reload_gizzi_skills(&headers).await };
            (
                StatusCode::OK,
                Json(json!({
                    "installed": true,
                    "id": body.id,
                    "name": name,
                    "path": outcome.path,
                    "unchanged": outcome.unchanged,
                    "versionId": marker_version_id(&outcome.marker),
                    "version": outcome.marker.get("version").cloned().unwrap_or(Value::Null),
                    "agentReloaded": reloaded,
                })),
            )
        }
        Ok(Err((status, message))) => fail(status, message),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn uninstall(headers: HeaderMap, AxPath(id): AxPath<String>) -> (StatusCode, Json<Value>) {
    let (Some((name, _)), Some(root)) = (resolve(&id), skills_root()) else {
        return fail(StatusCode::NOT_FOUND, format!("no installable skill with id {id}"));
    };
    let dest = root.join(&name);
    match marker_of(&dest) {
        Some(m) if m.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) => {
            if let Err(e) = std::fs::remove_dir_all(&dest) {
                return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            // Removing a skill removes its history too.
            if let Some(versions) = versions_root() {
                let _ = std::fs::remove_dir_all(versions.join(&name));
            }
            let reloaded = reload_gizzi_skills(&headers).await;
            (StatusCode::OK, Json(json!({ "removed": true, "id": id, "agentReloaded": reloaded })))
        }
        _ => fail(StatusCode::NOT_FOUND, format!("{id} isn't installed by Allternit")),
    }
}

/// Skills installed through this endpoint (marker present): directory id and
/// the folder name it landed in.
async fn installed() -> Json<Value> {
    let mut skills: Vec<(String, String)> = Vec::new();
    if let Some(root) = skills_root() {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let Some(id) = marker_of(&entry.path()).and_then(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string)) else {
                    continue;
                };
                skills.push((id, entry.file_name().to_string_lossy().into_owned()));
            }
        }
    }
    skills.sort();
    Json(json!({
        "installed": skills.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        "skills": skills.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
    }))
}

/// Versions of an installed skill: the current install, then archived ones.
async fn versions(AxPath(id): AxPath<String>) -> (StatusCode, Json<Value>) {
    let (Some((name, _)), Some(root), Some(archive)) = (resolve(&id), skills_root(), versions_root()) else {
        return fail(StatusCode::NOT_FOUND, format!("no installable skill with id {id}"));
    };
    let list = tokio::task::spawn_blocking(move || list_versions(&root, &archive, &id, &name)).await.unwrap_or_default();
    (StatusCode::OK, Json(json!({ "versions": list })))
}

async fn restore(headers: HeaderMap, AxPath((id, version_id)): AxPath<(String, String)>) -> (StatusCode, Json<Value>) {
    let (Some((name, _)), Some(root), Some(archive)) = (resolve(&id), skills_root(), versions_root()) else {
        return fail(StatusCode::NOT_FOUND, format!("no installable skill with id {id}"));
    };
    let vid = version_id.clone();
    let id_for_task = id.clone();
    let restored = tokio::task::spawn_blocking(move || restore_version(&root, &archive, &id_for_task, &name, &vid)).await;
    match restored {
        Ok(Ok(marker)) => {
            let reloaded = reload_gizzi_skills(&headers).await;
            (
                StatusCode::OK,
                Json(json!({ "restored": true, "id": id, "versionId": version_id, "version": marker.get("version").cloned().unwrap_or(Value::Null), "agentReloaded": reloaded })),
            )
        }
        Ok(Err((status, message))) => fail(status, message),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Folders Customize lists skills and plugins from (mirrors the capability
/// scanner's roots). Downloads are limited to folders inside these.
fn download_roots() -> Vec<PathBuf> {
    if let Ok(dirs) = std::env::var("ALLTERNIT_SKILL_DOWNLOAD_ROOTS") {
        return std::env::split_paths(&dirs).collect();
    }
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    [".allternit/skills", ".agents/skills", ".codex/skills", ".allternit/plugins", ".agents/plugins", ".codex/plugins"]
        .iter()
        .map(|r| home.join(r))
        .collect()
}

/// Zip a skill/plugin folder (every file, binaries included; symlinks and the
/// install marker left out). The folder must sit inside one of `roots`.
pub fn zip_folder(roots: &[PathBuf], folder: &Path) -> Result<Vec<u8>, (StatusCode, String)> {
    let folder = folder
        .canonicalize()
        .map_err(|_| (StatusCode::NOT_FOUND, format!("{} doesn't exist", folder.display())))?;
    let allowed = roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| folder != r && folder.starts_with(&r));
    if !allowed || !folder.is_dir() {
        return Err((StatusCode::FORBIDDEN, "only skill and plugin folders can be downloaded".into()));
    }
    let top = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "skill".into());
    let io = |e: std::io::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut stack = vec![folder.clone()];
        let mut total = 0usize;
        while let Some(dir) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir).map_err(io)?.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let kind = entry.file_type().map_err(io)?;
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_dir() {
                    stack.push(path);
                    continue;
                }
                if entry.file_name() == MARKER {
                    continue;
                }
                let bytes = std::fs::read(&path).map_err(io)?;
                total += bytes.len();
                if total > MAX_SKILL_BYTES * 4 {
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "folder is too large to download".into()));
                }
                let rel = path.strip_prefix(&folder).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                zip.start_file(format!("{top}/{rel}"), opts).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                std::io::Write::write_all(&mut zip, &bytes).map_err(io)?;
            }
        }
        zip.finish().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    Ok(buf.into_inner())
}

#[derive(Deserialize)]
struct ArchiveQuery {
    path: String,
}

/// `GET /skills/folder-archive?path=<folder>` → the folder as a .zip download.
async fn folder_archive(axum::extract::Query(q): axum::extract::Query<ArchiveQuery>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let folder = PathBuf::from(&q.path);
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "skill".into());
    let roots = download_roots();
    match tokio::task::spawn_blocking(move || zip_folder(&roots, &folder)).await {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "application/zip".to_string()),
                (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}.zip\"", name.replace('"', ""))),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err((status, message))) => fail(status, message).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub fn skill_catalog_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/skills/catalog/install", post(install))
        .route("/skills/catalog/installed", get(installed))
        .route("/skills/catalog/:id", delete(uninstall))
        .route("/skills/catalog/:id/versions", get(versions))
        .route("/skills/catalog/:id/versions/:version_id/restore", post(restore))
        .route("/skills/folder-archive", get(folder_archive))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_directory_ids() {
        assert!(matches!(resolve("claude-docx"), Some((n, Source::GitHub { .. })) if n == "docx"));
        assert!(matches!(resolve("codex-clone-website"), Some((n, Source::Embedded(_))) if n == "clone-website"));
        assert!(resolve("claude-../etc").is_none());
        assert!(resolve("unknown").is_none());
    }

    #[test]
    fn embedded_skills_ship_their_skill_md() {
        for id in ["allternit-alabs-course-pipeline", "allternit-allternit-codebase-to-course", "codex-clone-website", "codex-mobile-app-design"] {
            let Some((_, Source::Embedded(dir))) = resolve(id) else { panic!("{id} not embedded") };
            assert!(dir.get_file("SKILL.md").is_some(), "{id} has no SKILL.md");
        }
    }

    #[tokio::test]
    async fn installs_replaces_and_protects_hand_written_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let versions = tmp.path().join(".versions");
        let Some((name, source)) = resolve("codex-clone-website") else { panic!() };
        let (files, commit) = collect_files(&source).await.unwrap();
        assert!(commit.is_none());
        let info = source_info(&source, None);

        let out = write_skill(root, &versions, "codex-clone-website", &name, &files, info.clone()).unwrap();
        assert!(!out.unchanged);
        assert!(out.path.join("SKILL.md").exists());
        assert!(marker_of(&out.path).is_some());
        // Reinstalling identical content changes nothing.
        let again = write_skill(root, &versions, "codex-clone-website", &name, &files, info).unwrap();
        assert!(again.unchanged);

        // A folder the user made by hand is never overwritten.
        std::fs::create_dir_all(root.join("mine")).unwrap();
        std::fs::write(root.join("mine/SKILL.md"), "# mine").unwrap();
        let err = write_skill(root, &versions, "x", "mine", &files, json!({})).unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(std::fs::read_to_string(root.join("mine/SKILL.md")).unwrap(), "# mine");
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_relative("../x").is_none());
        assert!(safe_relative("/etc/passwd").is_none());
        assert!(safe_relative("scripts/run.py").is_some());
    }

    fn skill(body: &str) -> Vec<(PathBuf, Vec<u8>)> {
        vec![(PathBuf::from("SKILL.md"), body.as_bytes().to_vec())]
    }

    #[test]
    fn reads_the_declared_version() {
        assert_eq!(declared_version(&skill("---\nname: x\nversion: 1.2.0\n---\n# x")).as_deref(), Some("1.2.0"));
        assert_eq!(declared_version(&skill("---\nname: x\n---\n# x")), None);
        assert_eq!(declared_version(&skill("# no frontmatter")), None);
    }

    #[test]
    fn updates_archive_the_previous_copy_and_restore_brings_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        let versions = tmp.path().join("versions");
        std::fs::create_dir_all(&root).unwrap();
        let src = json!({ "kind": "github", "repo": "anthropics/skills", "ref": "main", "commit": "abc1234" });

        let v1 = write_skill(&root, &versions, "claude-x", "x", &skill("---\nversion: 1.0.0\n---\none"), src.clone()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let v2 = write_skill(&root, &versions, "claude-x", "x", &skill("---\nversion: 2.0.0\n---\ntwo"), src).unwrap();
        assert!(!v2.unchanged);

        let list = list_versions(&root, &versions, "claude-x", "x");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["current"], json!(true));
        assert_eq!(list[0]["version"], json!("2.0.0"));
        assert_eq!(list[1]["version"], json!("1.0.0"));
        assert_eq!(list[0]["source"]["commit"], json!("abc1234"));

        let old_id = marker_version_id(&v1.marker);
        restore_version(&root, &versions, "claude-x", "x", &old_id).unwrap();
        assert!(std::fs::read_to_string(root.join("x/SKILL.md")).unwrap().contains("one"));
        let after = list_versions(&root, &versions, "claude-x", "x");
        assert_eq!(after[0]["version"], json!("1.0.0"));
        assert_eq!(after[0]["current"], json!(true));
        assert_eq!(after.len(), 2);

        assert_eq!(restore_version(&root, &versions, "claude-x", "x", "../../etc").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(restore_version(&root, &versions, "claude-y", "x", &old_id).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn keeps_at_most_ten_archived_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        let versions = tmp.path().join("versions");
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..14 {
            write_skill(&root, &versions, "claude-x", "x", &skill(&format!("# v{i}")), json!({})).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        assert_eq!(list_versions(&root, &versions, "claude-x", "x").len(), 1 + MAX_ARCHIVED_VERSIONS);
    }

    #[test]
    fn zips_a_skill_folder_inside_the_roots_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        let skill = root.join("pdf");
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# pdf").unwrap();
        std::fs::write(skill.join("scripts/run.bin"), [0u8, 159, 146, 150]).unwrap();
        std::fs::write(skill.join(MARKER), "{}").unwrap();

        let bytes = zip_folder(&[root.clone()], &skill).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut names: Vec<String> = (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["pdf/SKILL.md", "pdf/scripts/run.bin"]);

        // Outside the roots, or the root itself: refused.
        let outside = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(zip_folder(&[root.clone()], &outside).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(zip_folder(&[root.clone()], &root).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(zip_folder(&[root.clone()], &root.join("../elsewhere")).unwrap_err().0, StatusCode::FORBIDDEN);
    }
}
