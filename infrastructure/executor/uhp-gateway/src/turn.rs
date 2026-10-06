//! Turn execution: drive one CLI-agent turn in an engine PTY pane and watch it
//! to completion. One-shot headless command per turn; the prompt is an argv
//! flag. The per-session workspace directory on disk is the checkpoint.
//!
//! Output is parsed from the transcript tee file (`ALLTERNIT_FACTORY_TRANSCRIPT`, the
//! P1 additive raw-byte pane log), NOT from the rendered pane screen: headless
//! panes render at a few columns wide, so screen reads wrap every NDJSON line
//! into unparseable fragments (verified live 2026-09-10 — a claude turn
//! "completed" with empty output in 1.3 s). The tee writes raw PTY bytes:
//! real newlines, no wrapping.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::drivers::{DriverKind, DriverState, ParsedEvent};
use crate::engine::EngineClient;
use crate::protocol::{
    ContentPart, ErrorBody, OutputItem, Response, ResponseStatus, StreamEvent, Usage,
};
use crate::store::Store;
use crate::AppState;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// How long to wait for the engine workspace to appear after layout.apply.
const APPEAR_GRACE: Duration = Duration::from_secs(15);
/// Consecutive engine RPC failures before declaring the engine down.
const MAX_ENGINE_ERRORS: u32 = 25;
/// Settle delay after the workspace closes, so the transcript tee can flush
/// the process's final bytes before the last drain.
const FINAL_DRAIN_DELAY: Duration = Duration::from_millis(400);
pub const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(900);

/// Shared cancel flag for one live turn.
#[derive(Debug, Default)]
pub struct TurnControl {
    pub cancel_requested: AtomicBool,
}

pub struct TurnContext {
    pub store: Store,
    pub engine: EngineClient,
    pub data_dir: std::path::PathBuf,
}

impl From<&Arc<AppState>> for TurnContext {
    fn from(state: &Arc<AppState>) -> Self {
        Self {
            store: state.store.clone(),
            engine: state.engine.clone(),
            data_dir: state.data_dir.clone(),
        }
    }
}

/// Run one turn to a terminal state, streaming events as they happen.
/// `response` is the already-persisted in_progress response; the returned
/// response is the final persisted state.
#[allow(clippy::too_many_arguments)]
pub async fn run_turn(
    ctx: TurnContext,
    session_id: String,
    mut response: Response,
    prompt: String,
    cli_model: Option<String>,
    timeout: Duration,
    driver: DriverKind,
    wih_id: Option<String>,
    control: Arc<TurnControl>,
    events: mpsc::UnboundedSender<StreamEvent>,
) -> Response {
    let _ = events.send(StreamEvent::response_created(&response));

    let deadline = Instant::now() + timeout;

    let final_response = match run_turn_inner(
        &ctx,
        &session_id,
        &mut response,
        &prompt,
        cli_model,
        driver,
        wih_id.as_deref(),
        &control,
        &events,
        deadline,
    )
    .await
    {
        Ok(()) => response,
        Err(message) => {
            response.status = ResponseStatus::Failed;
            response.error = Some(ErrorBody {
                error_type: "harness_error".into(),
                code: "harness_error".into(),
                message: sanitize_error(&message),
                param: None,
                detail: None,
            });
            response
        }
    };

    let _ = ctx.store.update_response(&final_response).await;
    let terminal_type = match final_response.status {
        ResponseStatus::Completed => "response.completed",
        ResponseStatus::Failed => "response.failed",
        ResponseStatus::Incomplete => "response.incomplete",
        ResponseStatus::Cancelled => "response.cancelled",
        ResponseStatus::InProgress => "response.failed",
    };
    let _ = events.send(StreamEvent::terminal(terminal_type, &final_response));
    final_response
}

#[allow(clippy::too_many_arguments)]
async fn run_turn_inner(
    ctx: &TurnContext,
    session_id: &str,
    response: &mut Response,
    prompt: &str,
    cli_model: Option<String>,
    driver: DriverKind,
    wih_id: Option<&str>,
    control: &Arc<TurnControl>,
    events: &mpsc::UnboundedSender<StreamEvent>,
    deadline: Instant,
) -> Result<(), String> {
    let session = ctx
        .store
        .get_session(session_id)
        .await
        .map_err(|err| format!("database error: {err}"))?
        .ok_or_else(|| "session vanished from the store".to_string())?;

    let session_dir = ctx.data_dir.join("sessions").join(session_id);
    let workspace_dir = session_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir)
        .map_err(|err| format!("could not create session workspace: {err}"))?;
    let cwd = workspace_dir.display().to_string();

    // ── engine workspace (one per UHP session, created on first turn) ────────
    let label = if session.engine_label.is_empty() {
        format!("uhp-{session_id}")
    } else {
        session.engine_label.clone()
    };
    let (workspace_id, tab_id) = match ctx.engine.find_workspace(&label).await {
        Ok(Some(workspace)) => {
            // Live workspace: its tab is the one recorded on the session row.
            let ws_id = workspace["workspace_id"]
                .as_str()
                .ok_or("engine workspace has no workspace_id")?
                .to_string();
            let tab_id = session.engine_tab_id.clone().unwrap_or_default();
            (ws_id, tab_id)
        }
        Ok(None) => {
            // New workspace: the fresh tab id wins — the session row's tab id
            // is stale the moment the previous workspace was removed.
            let (ws_id, tab_id) = ctx
                .engine
                .workspace_create(&cwd, &label)
                .await
                .map_err(|err| format!("engine refused workspace.create: {err}"))?;
            ctx.store
                .attach_session_engine(session_id, &ws_id, &tab_id)
                .await
                .map_err(|err| format!("database error: {err}"))?;
            (ws_id, tab_id)
        }
        Err(err) => return Err(format!("engine unreachable: {err}")),
    };

    // ── launch the one-shot CLI in a fresh pane ──────────────────────────────
    // Spawn gate: admission + hook settings before anything is launched.
    let gated = crate::spawn_gate::prepare(driver, &session_dir, &workspace_dir, wih_id).await?;
    let argv = driver.argv(
        prompt,
        cli_model.as_deref(),
        session.driver_session_ref.as_deref(),
        &workspace_dir,
        gated.claude_settings.as_deref(),
    );
    let transcript_path = session_dir.join(format!("turn-{}.log", response.id));
    let mut env = vec![(
        "ALLTERNIT_FACTORY_TRANSCRIPT".to_string(),
        transcript_path.display().to_string(),
    )];
    // A model override needs an isolated Codex home with an explicit provider
    // config; otherwise inherit the user's native-authed configuration.
    if matches!(driver, DriverKind::Codex) && cli_model.is_some() {
        write_codex_config(&session_dir).map_err(|err| format!("could not write codex config: {err}"))?;
        env.push((
            "CODEX_HOME".to_string(),
            session_dir.join(".codex").display().to_string(),
        ));
    }
    if tab_id.is_empty() {
        return Err("no engine tab for the session workspace".to_string());
    }
    ctx.engine
        .layout_apply(&tab_id, &cwd, argv, env)
        .await
        .map_err(|err| format!("engine refused layout.apply: {err}"))?;

    // ── watch loop ───────────────────────────────────────────────────────────
    // Liveness = workspace presence (the engine removes dead panes'
    // workspaces). Output = incremental reads of the transcript tee file.
    let mut driver_state = DriverState::default();
    let mut usage: Option<Usage> = None;
    let mut driver_error: Option<String> = None;
    let mut transcript_offset: u64 = 0;
    let mut leftover = String::new();
    let mut appeared = false;
    let mut close_sent = false;
    let mut engine_errors = 0u32;
    let grace_deadline = Instant::now() + APPEAR_GRACE;
    let mut status_override: Option<ResponseStatus> = None;
    let mut dirty = true;

    loop {
        if control.cancel_requested.load(Ordering::SeqCst) && !close_sent {
            let _ = ctx.engine.workspace_close(&workspace_id).await;
            close_sent = true;
        }

        match ctx.engine.find_workspace(&label).await {
            Ok(Some(_workspace)) => {
                appeared = true;
                engine_errors = 0;
                let new_events = drain_transcript(
                    &transcript_path,
                    &mut transcript_offset,
                    &mut leftover,
                    driver,
                    &mut driver_state,
                );
                handle_events(
                    ctx,
                    session_id,
                    &response.id,
                    new_events,
                    events,
                    &mut usage,
                    &mut driver_error,
                    &mut dirty,
                )
                .await;
            }
            Ok(None) => {
                if !appeared {
                    if Instant::now() > grace_deadline {
                        return Err("engine never started the turn workspace".to_string());
                    }
                } else {
                    break; // process exited
                }
            }
            Err(_) => {
                engine_errors += 1;
                if engine_errors >= MAX_ENGINE_ERRORS {
                    return Err("engine unreachable during turn".to_string());
                }
            }
        }

        if Instant::now() > deadline {
            status_override = Some(ResponseStatus::Incomplete);
            let _ = ctx.engine.workspace_close(&workspace_id).await;
            break;
        }

        if dirty {
            response.output = vec![output_item(&driver_state.text, &response.id)];
            let _ = ctx.store.update_response(response).await;
            dirty = false;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Final drain: the tee may flush the CLI's last bytes just after the
    // workspace closes; a trailing line without a newline counts too.
    tokio::time::sleep(FINAL_DRAIN_DELAY).await;
    let new_events = drain_transcript(
        &transcript_path,
        &mut transcript_offset,
        &mut leftover,
        driver,
        &mut driver_state,
    );
    handle_events(
        ctx,
        session_id,
        &response.id,
        new_events,
        events,
        &mut usage,
        &mut driver_error,
        &mut dirty,
    )
    .await;
    let trailing = std::mem::take(&mut leftover);
    if !trailing.trim().is_empty() {
        let new_events = driver.parse_line(trailing.trim_end_matches('\r'), &mut driver_state);
        handle_events(
            ctx,
            session_id,
            &response.id,
            new_events,
            events,
            &mut usage,
            &mut driver_error,
            &mut dirty,
        )
        .await;
    }

    if control.cancel_requested.load(Ordering::SeqCst) {
        status_override = Some(ResponseStatus::Cancelled);
    }

    let status = status_override.unwrap_or_else(|| {
        if driver_error.is_some() {
            ResponseStatus::Failed
        } else {
            ResponseStatus::Completed
        }
    });
    response.status = status;
    if let Some(message) = driver_error {
        response.error = Some(ErrorBody {
            error_type: "harness_error".into(),
            code: "harness_error".into(),
            message: sanitize_error(&message),
            param: None,
            detail: None,
        });
    }
    response.output = vec![output_item(&driver_state.text, &response.id)];
    response.usage = usage;
    let _ = ctx
        .store
        .set_driver_session_ref(session_id, driver_state.session_ref.as_deref())
        .await;
    Ok(())
}

/// Read the bytes appended to the transcript tee since the last poll, buffer
/// the trailing partial line, and feed complete lines to the driver parser.
fn drain_transcript(
    path: &std::path::Path,
    offset: &mut u64,
    leftover: &mut String,
    driver: DriverKind,
    state: &mut DriverState,
) -> Vec<ParsedEvent> {
    let chunk = read_appended(path, offset);
    if chunk.is_empty() {
        return Vec::new();
    }
    leftover.push_str(&chunk);
    let mut events = Vec::new();
    let mut start = 0usize;
    while let Some(relative) = leftover[start..].find('\n') {
        let line_end = start + relative;
        let line = leftover[start..line_end].trim_end_matches('\r').to_string();
        start = line_end + 1;
        if line.trim().is_empty() {
            continue;
        }
        events.extend(driver.parse_line(&line, state));
    }
    leftover.drain(..start.min(leftover.len()));
    events
}

/// Bytes appended to the file since `offset`; empty when the tee has not
/// created the file yet. Resets when the file shrinks (truncation/rewrite).
fn read_appended(path: &std::path::Path, offset: &mut u64) -> String {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if len < *offset {
        *offset = 0;
    }
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    *offset += bytes.len() as u64;
    String::from_utf8_lossy(&bytes).into_owned()
}

#[allow(clippy::too_many_arguments)]
async fn handle_events(
    ctx: &TurnContext,
    session_id: &str,
    response_id: &str,
    new_events: Vec<ParsedEvent>,
    sender: &mpsc::UnboundedSender<StreamEvent>,
    usage: &mut Option<Usage>,
    driver_error: &mut Option<String>,
    dirty: &mut bool,
) {
    for event in new_events {
        match event {
            ParsedEvent::TextDelta(delta) => {
                let item_id = format!("msg_{response_id}");
                let _ = sender.send(StreamEvent::text_delta(delta, &item_id));
                *dirty = true;
            }
            ParsedEvent::Usage(u) => *usage = Some(u),
            ParsedEvent::SessionRef(id) => {
                let _ = ctx
                    .store
                    .set_driver_session_ref(session_id, Some(&id))
                    .await;
            }
            ParsedEvent::Error(message) => *driver_error = Some(message),
            ParsedEvent::Done => {}
        }
    }
}

fn output_item(text: &str, response_id: &str) -> OutputItem {
    OutputItem {
        id: Some(format!("msg_{response_id}")),
        item_type: "message".into(),
        status: Some("completed".into()),
        role: Some("assistant".into()),
        content: Some(vec![ContentPart {
            part_type: "output_text".into(),
            text: text.to_string(),
        }]),
    }
}

fn write_codex_config(session_dir: &std::path::Path) -> std::io::Result<()> {
    let codex_home = session_dir.join(".codex");
    std::fs::create_dir_all(&codex_home)?;
    std::fs::write(
        codex_home.join("config.toml"),
        concat!(
            "model_provider = \"openai\"\n",
            "approval_policy = \"never\"\n",
            "sandbox_mode = \"workspace-write\"\n",
            "[sandbox_workspace_write]\n",
            "network_access = true\n",
        ),
    )
}

/// One sentence, safe to show a user: no paths, no newlines, no stack frames.
pub fn sanitize_error(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or("turn failed");
    let cleaned: String = first_line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    // Stack-frame markers leak internals (E-04): refuse to relay them at all.
    let lowered = trimmed.to_lowercase();
    if trimmed.is_empty()
        || lowered.contains("traceback")
        || trimmed.contains("  at ")
        || trimmed.contains("File \"")
    {
        return "turn failed".to_string();
    }
    trimmed.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_paths_and_newlines() {
        let message = "Traceback (most recent call last):\n  File \"/x/y.py\", line 1";
        let clean = sanitize_error(message);
        assert!(!clean.contains('\n'));
        assert!(!clean.contains("Traceback"));
        assert!(clean.len() <= 200);
        assert_eq!(sanitize_error(""), "turn failed");
    }

    #[test]
    fn drain_buffers_partial_lines() {
        let driver = DriverKind::Claude;
        let mut state = DriverState::default();
        let mut offset = 0u64;
        let mut leftover = String::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("turn.log");

        // A trailing partial line is held between polls.
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"He",
        )
        .expect("write");
        let events = drain_transcript(&path, &mut offset, &mut leftover, driver, &mut state);
        assert!(events.is_empty());
        assert!(!leftover.is_empty());

        // Appending completes the line; the next drain parses it.
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        file.write_all(
            b"llo\"}]}}\n{\"type\":\"result\",\"is_error\":false,\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}\n",
        )
        .expect("append");
        let events = drain_transcript(&path, &mut offset, &mut leftover, driver, &mut state);
        assert_eq!(state.text, "Hello");
        assert!(events.iter().any(|event| matches!(event, ParsedEvent::Usage(_))));
        assert!(events.iter().any(|event| matches!(event, ParsedEvent::Done)));
        assert!(leftover.is_empty());
    }

    #[test]
    fn read_appended_resets_when_file_shrinks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("turn.log");
        std::fs::write(&path, "a much longer first contents\n").expect("write");
        let mut offset = 0u64;
        let first = read_appended(&path, &mut offset);
        assert_eq!(first, "a much longer first contents\n");
        std::fs::write(&path, "short\n").expect("rewrite");
        let second = read_appended(&path, &mut offset);
        assert_eq!(second, "short\n");
    }

    #[test]
    fn read_appended_missing_file_is_empty() {
        let mut offset = 0u64;
        assert!(read_appended(std::path::Path::new("/nonexistent/turn.log"), &mut offset).is_empty());
        assert_eq!(offset, 0);
    }
}
