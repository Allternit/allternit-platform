//! Human-only clipboard and safe Downloads upload, under computer ownership and control.
use crate::{
    auth::AuthUser,
    computer_control_lease::HolderKind,
    computer_toolset::{Target, Toolset},
    AppState,
};
use allternit_driver_interface::CommandSpec;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Deserialize;
use serde_json::json;
use std::{process::Stdio, sync::Arc};
use tokio::io::AsyncWriteExt;
const CLIP_LIMIT: usize = 256 * 1024;
const FILE_LIMIT: usize = 25 * 1024 * 1024;
fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"message":message.into()}))).into_response()
}
async fn controlled(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
    headers: &HeaderMap,
) -> Result<Target, Response> {
    let computer = crate::computer_toolset::resolve_computer(state, user, id, headers)
        .await
        .map_err(|(s, m)| failure(s, m))?;
    let bot_controlled = if let Some(bot) = &computer.bot_id {
        state
            .bot_desktop_sessions
            .read()
            .await
            .get(bot)
            .is_some_and(|session| {
                matches!(
                    session.control_state,
                    crate::BotDesktopControlState::HumanControls
                ) && session.taken_over_by_user_id.as_deref() == Some(&user.user_id)
            })
    } else {
        false
    };
    if !bot_controlled {
        let conn = state
            .db
            .connect()
            .map_err(|_| failure(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't check control"))?;
        let lease = crate::computer_control_lease::current(
            &conn,
            &computer.id,
            chrono::Utc::now().timestamp(),
        )
        .map_err(|_| failure(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't check control"))?;
        let device = headers
            .get(crate::cowork_devices_routes::DEVICE_ID_HEADER)
            .and_then(|v| v.to_str().ok());
        if !lease.is_some_and(|lease| {
            lease.holder.kind == HolderKind::User
                && lease.holder.id == user.user_id
                && (lease.holder.device_id.is_none() || lease.holder.device_id.as_deref() == device)
        }) {
            return Err(failure(
                StatusCode::LOCKED,
                "Take over this computer to transfer clipboard text or files.",
            ));
        }
    }
    crate::computer_toolset::build_target(state, user, &computer, Toolset::Computer, None)
        .await
        .map_err(|(s, m)| failure(s, m))
}
async fn clipboard(target: &Target, text: Option<&str>) -> Result<String, String> {
    let write = text.is_some();
    let encoded = B64.encode(text.unwrap_or_default());
    let win = if write {
        format!("[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')) | Set-Clipboard")
    } else {
        "Get-Clipboard -Raw".into()
    };
    let linux = if write {
        format!("printf %s '{encoded}' | base64 -d | xclip -selection clipboard -in")
    } else {
        "xclip -selection clipboard -out".into()
    };
    let output = match target {
        Target::ThisDevice => {
            let mut command = if cfg!(target_os = "macos") {
                tokio::process::Command::new(if write { "pbcopy" } else { "pbpaste" })
            } else if cfg!(target_os = "windows") {
                let mut c = tokio::process::Command::new("powershell");
                c.args(["-NoProfile", "-STA", "-Command", &win]);
                c
            } else {
                let mut c = tokio::process::Command::new("sh");
                c.args(["-c", &linux]);
                c
            };
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            if cfg!(target_os = "macos") && write {
                command.stdin(Stdio::piped());
            }
            let mut child = command.spawn().map_err(|e| e.to_string())?;
            if cfg!(target_os = "macos") && write {
                if let Some(mut input) = child.stdin.take() {
                    input
                        .write_all(text.unwrap_or_default().as_bytes())
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err("The computer's clipboard is unavailable.".into());
            }
            out.stdout
        }
        Target::Guest {
            driver,
            handle,
            os,
            display,
        } => {
            let command = if os == "windows" {
                vec![
                    "powershell".into(),
                    "-NoProfile".into(),
                    "-STA".into(),
                    "-Command".into(),
                    win,
                ]
            } else {
                vec!["sh".into(), "-c".into(), linux]
            };
            let out = driver
                .exec(
                    handle,
                    CommandSpec {
                        command,
                        env_vars: [("DISPLAY".into(), display.to_string())].into(),
                        working_dir: None,
                        stdin_data: None,
                        capture_stdout: true,
                        capture_stderr: true,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            if out.exit_code != 0 {
                return Err("The computer's clipboard is unavailable.".into());
            }
            out.stdout.unwrap_or_default()
        }
        Target::Browser { .. } => return Err("Clipboard transfer needs a desktop computer.".into()),
    };
    if output.len() > CLIP_LIMIT {
        return Err("Clipboard text exceeds 256 KB.".into());
    }
    String::from_utf8(output).map_err(|_| "Clipboard doesn't contain UTF-8 text.".into())
}
async fn get_clipboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let target = match controlled(&state, &user, &id, &headers).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match clipboard(&target, None).await {
        Ok(text) => Json(json!({"text":text})).into_response(),
        Err(e) => failure(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
#[derive(Deserialize)]
struct Clip {
    text: String,
}
async fn put_clipboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Clip>,
) -> Response {
    if body.text.len() > CLIP_LIMIT {
        return failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Clipboard text exceeds 256 KB.",
        );
    }
    let target = match controlled(&state, &user, &id, &headers).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match clipboard(&target, Some(&body.text)).await {
        Ok(_) => Json(json!({"success":true})).into_response(),
        Err(e) => failure(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
#[derive(Deserialize)]
struct Upload {
    name: String,
}
fn file_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 180
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
    {
        return Err("Choose a valid file name.".into());
    }
    Ok(format!(
        "{}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..16],
        name
    ))
}
async fn upload(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<Upload>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > FILE_LIMIT {
        return failure(StatusCode::PAYLOAD_TOO_LARGE, "Files are limited to 25 MB.");
    }
    let name = match file_name(&query.name) {
        Ok(n) => n,
        Err(e) => return failure(StatusCode::BAD_REQUEST, e),
    };
    let target = match controlled(&state, &user, &id, &headers).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let result: Result<String, String> = async {
        match target {
            Target::ThisDevice => {
                let dir = std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(std::path::PathBuf::from)
                    .ok_or("Downloads is unavailable")?
                    .join("Downloads")
                    .join("Allternit");
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|e| e.to_string())?;
                let path = dir.join(name);
                tokio::fs::write(&path, &body)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(path.display().to_string())
            }
            Target::Guest {
                driver, handle, os, ..
            } => {
                let path = if os == "windows" {
                    format!("C:/Users/Public/Downloads/{name}")
                } else {
                    format!("/tmp/{name}")
                };
                driver
                    .push_file(&handle, &path, body.to_vec())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(path)
            }
            Target::Browser { .. } => Err("File transfer needs a desktop computer.".into()),
        }
    }
    .await;
    match result {
        Ok(path) => Json(json!({"success":true,"path":path})).into_response(),
        Err(e) => failure(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/computers/:id/clipboard",
            get(get_clipboard).put(put_clipboard),
        )
        .route(
            "/computers/:id/transfer/file",
            post(upload).layer(DefaultBodyLimit::max(FILE_LIMIT)),
        )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_names_cannot_escape_downloads() {
        for name in ["../evil", "a/b", "a\\b", "..", ""] {
            assert!(file_name(name).is_err());
        }
        assert!(file_name("Report.pdf").unwrap().ends_with("-Report.pdf"));
    }
}
