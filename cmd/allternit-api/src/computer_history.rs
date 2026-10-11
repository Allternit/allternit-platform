//! Bounded, authenticated visual history of executed computer steps.
//! Only redacted screenshots and action names are stored; never tool input text.
use crate::{auth::AuthUser, computer_safety::SafetySettings, computer_toolset::Target, AppState};
use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const MAX_STEPS: usize = 500;
const RETENTION_DAYS: i64 = 30;
#[derive(Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub run_id: String,
    pub at: String,
    pub label: String,
    pub ok: bool,
    pub thumbnail: Option<String>,
    pub snapshot: bool,
}
static ACTIVE: once_cell::sync::Lazy<Mutex<std::collections::HashMap<String, serde_json::Value>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(std::collections::HashMap::new()));
pub fn start(computer: &str, run: &str, member: &str) {
    ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).insert(
        computer.into(),
        json!({"run_id":run,"label":member.replace('_', " "),"at":chrono::Utc::now().to_rfc3339()}),
    );
}
fn directory(root: &FsPath, computer: &str) -> PathBuf {
    root.join("computer-history")
        .join(hex::encode(Sha256::digest(computer.as_bytes())))
}
fn valid_step(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}
fn read_steps(dir: &FsPath) -> Vec<Step> {
    let mut steps: Vec<Step> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| serde_json::from_slice::<Step>(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    steps.retain(|step| valid_step(&step.id));
    steps.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    steps
}
fn png_at_size(png: &[u8], edge: u32) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(png)
        .map_err(|e| e.to_string())?
        .thumbnail(edge, edge);
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}
fn save(root: &FsPath, computer: &str, mut step: Step, png: Option<Vec<u8>>) -> Result<(), String> {
    let dir = directory(root, computer);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    if let Some(png) = png {
        let full = png_at_size(&png, 1280)?;
        if full.len() <= 2 * 1024 * 1024 {
            std::fs::write(dir.join(format!("{}.png", step.id)), full)
                .map_err(|e| e.to_string())?;
            step.thumbnail = Some(format!(
                "data:image/png;base64,{}",
                B64.encode(png_at_size(&png, 192)?)
            ));
            step.snapshot = true;
        }
    }
    std::fs::write(
        dir.join(format!("{}.json", step.id)),
        serde_json::to_vec(&step).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    prune(&dir)?;
    Ok(())
}
fn prune(dir: &FsPath) -> Result<Vec<Step>, String> {
    let steps = read_steps(dir);
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
    let extra = steps.len().saturating_sub(MAX_STEPS);
    let mut kept = Vec::new();
    for (index, step) in steps.into_iter().enumerate() {
        if index < extra || step.at < cutoff {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", step.id)));
            let _ = std::fs::remove_file(dir.join(format!("{}.png", step.id)));
        } else {
            kept.push(step);
        }
    }
    let mut total: u64 = kept
        .iter()
        .filter_map(|s| std::fs::metadata(dir.join(format!("{}.png", s.id))).ok())
        .map(|m| m.len())
        .sum();
    for step in &mut kept {
        if total <= 100 * 1024 * 1024 {
            break;
        }
        let path = dir.join(format!("{}.png", step.id));
        if let Ok(meta) = std::fs::metadata(&path) {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            total = total.saturating_sub(meta.len());
            step.snapshot = false;
            std::fs::write(
                dir.join(format!("{}.json", step.id)),
                serde_json::to_vec(step).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(kept)
}
/// Captures after the action, with a short bound. Capture/redaction failure still records the step.
pub async fn record(
    state: &Arc<AppState>,
    target: &Target,
    computer: &str,
    run: &str,
    member: &str,
    ok: bool,
    settings: &SafetySettings,
    secrets: &[String],
) {
    let mut settings = settings.clone();
    // History is always protected, even when live-screen redaction is switched off.
    settings.redaction = Some("required".into());
    settings.redact = None;
    let png = tokio::time::timeout(Duration::from_secs(4), async {
        let png = crate::computer_toolset::capture(target, run).await?;
        crate::computer_safety::redact_png(&png, &settings, target, secrets, &state.data_dir)
            .await
            .map(|(png, _)| png)
    })
    .await
    .ok()
    .and_then(Result::ok);
    let step = Step {
        id: uuid::Uuid::new_v4().to_string(),
        run_id: run.to_string(),
        at: chrono::Utc::now().to_rfc3339(),
        label: member.replace('_', " "),
        ok,
        thumbnail: None,
        snapshot: false,
    };
    ACTIVE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(computer);
    let (root, computer) = (state.data_dir.clone(), computer.to_string());
    match tokio::task::spawn_blocking(move || save(&root, &computer, step, png)).await {
        Ok(Ok(())) => {}
        other => tracing::warn!(?other, "computer history could not be saved"),
    }
}
async fn authorize(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
    headers: &HeaderMap,
) -> Result<String, Response> {
    crate::computer_toolset::resolve_computer(state, user, id, headers)
        .await
        .map(|c| c.id)
        .map_err(|(status, message)| (status, Json(json!({"message": message}))).into_response())
}
async fn history(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let id = match authorize(&state, &user, &id, &headers).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let dir = directory(&state.data_dir, &id);
    let active = ACTIVE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&id)
        .cloned();
    match tokio::task::spawn_blocking(move || prune(&dir)).await {
        Ok(Ok(steps)) => {
            Json(json!({"steps": steps, "active": active, "retention_days": RETENTION_DAYS, "max_steps": MAX_STEPS}))
                .into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
async fn snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, step)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let id = match authorize(&state, &user, &id, &headers).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !valid_step(&step) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dir = directory(&state.data_dir, &id);
    let metadata = tokio::fs::read(dir.join(format!("{step}.json")))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Step>(&bytes).ok());
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
    if !metadata.is_some_and(|s| s.at >= cutoff && s.snapshot) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(dir.join(format!("{step}.png"))).await {
        Ok(png) => (
            [
                ("content-type", "image/png"),
                ("cache-control", "private, no-store"),
            ],
            png,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/computers/:id/history", get(history))
        .route("/computers/:id/history/:step/snapshot", get(snapshot))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stores_bounded_metadata_without_trusting_identifiers() {
        let tmp = tempfile::tempdir().unwrap();
        let step = Step {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: "run-1".into(),
            at: chrono::Utc::now().to_rfc3339(),
            label: "left click".into(),
            ok: true,
            thumbnail: None,
            snapshot: false,
        };
        save(tmp.path(), "../../outside", step, None).unwrap();
        assert_eq!(read_steps(&directory(tmp.path(), "../../outside")).len(), 1);
        assert!(!valid_step("../snapshot"));
    }
    #[test]
    fn prunes_expired_frames_and_keeps_only_recent_bounded_steps() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = directory(tmp.path(), "computer");
        std::fs::create_dir_all(&dir).unwrap();
        let old = Step {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: "old".into(),
            at: (chrono::Utc::now() - chrono::Duration::days(31)).to_rfc3339(),
            label: "click".into(),
            ok: true,
            thumbnail: None,
            snapshot: true,
        };
        std::fs::write(
            dir.join(format!("{}.json", old.id)),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join(format!("{}.png", old.id)), b"old frame").unwrap();
        for n in 0..501 {
            let step = Step {
                id: uuid::Uuid::new_v4().to_string(),
                run_id: "current".into(),
                at: (chrono::Utc::now() + chrono::Duration::milliseconds(n)).to_rfc3339(),
                label: "click".into(),
                ok: true,
                thumbnail: None,
                snapshot: false,
            };
            std::fs::write(
                dir.join(format!("{}.json", step.id)),
                serde_json::to_vec(&step).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(prune(&dir).unwrap().len(), 500);
        assert!(!dir.join(format!("{}.png", old.id)).exists());
        assert!(!dir.join(format!("{}.json", old.id)).exists());
    }
}
