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

/// Files (relative path → bytes) for one skill.
async fn collect_files(source: &Source) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
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
            Ok(out)
        }
        Source::GitHub { repo, git_ref, path } => {
            let client = reqwest::Client::builder()
                .user_agent("allternit-desktop")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())?;
            let tree: Value = client
                .get(format!("https://api.github.com/repos/{repo}/git/trees/{git_ref}?recursive=1"))
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
                    .get(format!("https://raw.githubusercontent.com/{repo}/{git_ref}/{blob}"))
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
            Ok(out)
        }
    }
}

fn marker_of(dir: &Path) -> Option<Value> {
    std::fs::read_to_string(dir.join(MARKER)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// Write the files into `<root>/<name>`, replacing a previous managed install.
pub fn write_skill(root: &Path, id: &str, name: &str, files: &[(PathBuf, Vec<u8>)]) -> Result<PathBuf, (StatusCode, String)> {
    if !files.iter().any(|(p, _)| p == Path::new("SKILL.md")) {
        return Err((StatusCode::BAD_GATEWAY, "source has no SKILL.md".into()));
    }
    let dest = root.join(name);
    if dest.exists() {
        if marker_of(&dest).is_none() {
            return Err((
                StatusCode::CONFLICT,
                format!("{} already exists and wasn't installed by Allternit; leaving it alone", dest.display()),
            ));
        }
        std::fs::remove_dir_all(&dest).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    // Stage then rename, so a failed download never leaves half a skill.
    let staging = root.join(format!(".{name}.installing"));
    let _ = std::fs::remove_dir_all(&staging);
    let io = |e: std::io::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    for (rel, bytes) in files {
        let rel_str = rel.to_string_lossy();
        let rel = safe_relative(&rel_str).ok_or((StatusCode::BAD_GATEWAY, format!("unsafe path in source: {rel_str}")))?;
        let target = staging.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&target, bytes).map_err(io)?;
    }
    let marker = json!({ "id": id, "installedAt": chrono::Utc::now().to_rfc3339(), "files": files.len() });
    std::fs::write(staging.join(MARKER), marker.to_string()).map_err(io)?;
    std::fs::rename(&staging, &dest).map_err(io)?;
    Ok(dest)
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
    let files = match collect_files(&source).await {
        Ok(f) => f,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let id = body.id.clone();
    let name_for_write = name.clone();
    let written = tokio::task::spawn_blocking(move || write_skill(&root, &id, &name_for_write, &files)).await;
    match written {
        Ok(Ok(path)) => {
            let reloaded = reload_gizzi_skills(&headers).await;
            (
                StatusCode::OK,
                Json(json!({ "installed": true, "id": body.id, "name": name, "path": path, "agentReloaded": reloaded })),
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

pub fn skill_catalog_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/skills/catalog/install", post(install))
        .route("/skills/catalog/installed", get(installed))
        .route("/skills/catalog/:id", delete(uninstall))
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
        let Some((name, source)) = resolve("codex-clone-website") else { panic!() };
        let files = collect_files(&source).await.unwrap();

        let dest = write_skill(root, "codex-clone-website", &name, &files).unwrap();
        assert!(dest.join("SKILL.md").exists());
        assert!(marker_of(&dest).is_some());
        // Reinstall replaces the managed copy.
        write_skill(root, "codex-clone-website", &name, &files).unwrap();

        // A folder the user made by hand is never overwritten.
        std::fs::create_dir_all(root.join("mine")).unwrap();
        std::fs::write(root.join("mine/SKILL.md"), "# mine").unwrap();
        let err = write_skill(root, "x", "mine", &files).unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(std::fs::read_to_string(root.join("mine/SKILL.md")).unwrap(), "# mine");
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_relative("../x").is_none());
        assert!(safe_relative("/etc/passwd").is_none());
        assert!(safe_relative("scripts/run.py").is_some());
    }
}
