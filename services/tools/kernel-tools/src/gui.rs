//! GUI Automation Tools
//!
//! These tools provide computer-use automation capabilities:
//! - Screenshot capture
//! - Mouse control (click)
//! - Keyboard input (type)
//! - Scrolling
//!
//! Integration:
//! - Uses platform-specific APIs for automation (macOS: `screencapture`,
//!   `cliclick`, `osascript`)
//! - Exposed via tool gateway for brain access
//! - Requires explicit user approval (safety)

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// GUI tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    pub execution_time_ms: Option<u64>,
}

impl GuiToolResult {
    fn ok(output: String, started: Instant) -> Self {
        Self {
            success: true,
            output: Some(output),
            error: None,
            execution_time_ms: Some(started.elapsed().as_millis() as u64),
        }
    }
}

/// Screenshot capture result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub base64_image: String,
    pub width: u32,
    pub height: u32,
    pub timestamp: u64,
}

/// Click action parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickParams {
    pub x: u32,
    pub y: u32,
    pub button: Option<String>, // "left", "right"
    pub count: Option<u32>,
}

/// Type action parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeParams {
    pub text: String,
    /// Delay between keystrokes; `None` or 0 types the text in one go.
    pub delay_ms: Option<u64>,
}

/// Scroll action parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollParams {
    pub dx: i32,
    pub dy: i32,
    pub amount: Option<u32>,
}

#[async_trait]
pub trait GuiTool {
    async fn screenshot(&self) -> Result<ScreenshotResult, String>;

    async fn click(&self, params: ClickParams) -> Result<GuiToolResult, String>;

    async fn type_text(&self, params: TypeParams) -> Result<GuiToolResult, String>;

    async fn scroll(&self, params: ScrollParams) -> Result<GuiToolResult, String>;

    async fn run_task(&self, task: String, dry_run: bool) -> Result<Vec<GuiToolResult>, String>;
}

/// macOS GUI tool implementation
#[derive(Debug, Default)]
pub struct MacGuiTool;

impl MacGuiTool {
    pub fn new() -> Self {
        Self
    }
}

/// Read width and height from a PNG's IHDR chunk.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// Quote a string as an AppleScript string literal.
fn applescript_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

/// Build the `cliclick` arguments for a click.
fn cliclick_args(params: &ClickParams) -> Result<Vec<String>, String> {
    let point = format!("{},{}", params.x, params.y);
    let count = params.count.unwrap_or(1).max(1);
    match params.button.as_deref().unwrap_or("left") {
        "left" => Ok(match count {
            1 => vec![format!("c:{}", point)],
            2 => vec![format!("dc:{}", point)],
            3 => vec![format!("tc:{}", point)],
            n => (0..n).map(|_| format!("c:{}", point)).collect(),
        }),
        "right" => Ok((0..count).map(|_| format!("rc:{}", point)).collect()),
        other => Err(format!("unsupported mouse button: {}", other)),
    }
}

/// Build the AppleScript that types `text`.
fn keystroke_script(text: &str, delay_ms: u64) -> String {
    if delay_ms == 0 {
        return format!(
            "tell application \"System Events\" to keystroke {}",
            applescript_string(text)
        );
    }
    let delay_secs = delay_ms as f64 / 1000.0;
    let lines: Vec<String> = text
        .chars()
        .map(|c| {
            format!(
                "keystroke {}\ndelay {}",
                applescript_string(&c.to_string()),
                delay_secs
            )
        })
        .collect();
    format!(
        "tell application \"System Events\"\n{}\nend tell",
        lines.join("\n")
    )
}

/// Build the AppleScript that scrolls with the arrow keys.
fn scroll_script(dx: i32, dy: i32, amount: u32) -> Option<String> {
    let mut keys = Vec::new();
    if dy > 0 {
        keys.push(125); // down arrow
    } else if dy < 0 {
        keys.push(126); // up arrow
    }
    if dx > 0 {
        keys.push(124); // right arrow
    } else if dx < 0 {
        keys.push(123); // left arrow
    }
    if keys.is_empty() || amount == 0 {
        return None;
    }
    let presses: Vec<String> = keys.iter().map(|k| format!("key code {}", k)).collect();
    Some(format!(
        "tell application \"System Events\"\nrepeat {} times\n{}\nend repeat\nend tell",
        amount,
        presses.join("\n")
    ))
}

async fn run_command(cmd: &mut tokio::process::Command, name: &str) -> Result<(), String> {
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to execute {}: {}", name, e))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed ({}): {}",
            name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[async_trait]
impl GuiTool for MacGuiTool {
    async fn screenshot(&self) -> Result<ScreenshotResult, String> {
        let path = std::env::temp_dir().join(format!("allternit-screenshot-{}.png", uuid::Uuid::new_v4()));
        run_command(
            tokio::process::Command::new("screencapture")
                .args(["-x", "-t", "png"])
                .arg(&path),
            "screencapture",
        )
        .await?;

        let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string());
        let _ = tokio::fs::remove_file(&path).await;
        let bytes = bytes?;
        let (width, height) =
            png_dimensions(&bytes).ok_or_else(|| "screencapture did not write a PNG".to_string())?;

        Ok(ScreenshotResult {
            base64_image: STANDARD.encode(&bytes),
            width,
            height,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs(),
        })
    }

    async fn click(&self, params: ClickParams) -> Result<GuiToolResult, String> {
        let started = Instant::now();
        let args = cliclick_args(&params)?;
        run_command(tokio::process::Command::new("cliclick").args(&args), "cliclick").await?;
        Ok(GuiToolResult::ok(
            format!("Clicked at ({}, {})", params.x, params.y),
            started,
        ))
    }

    async fn type_text(&self, params: TypeParams) -> Result<GuiToolResult, String> {
        let started = Instant::now();
        let script = keystroke_script(&params.text, params.delay_ms.unwrap_or(0));
        run_command(
            tokio::process::Command::new("osascript").arg("-e").arg(&script),
            "osascript",
        )
        .await?;
        Ok(GuiToolResult::ok(format!("Typed: {}", params.text), started))
    }

    async fn scroll(&self, params: ScrollParams) -> Result<GuiToolResult, String> {
        let started = Instant::now();
        let amount = params.amount.unwrap_or(3);
        let script = scroll_script(params.dx, params.dy, amount)
            .ok_or_else(|| "scroll needs a non-zero dx or dy and amount".to_string())?;
        run_command(
            tokio::process::Command::new("osascript").arg("-e").arg(&script),
            "osascript",
        )
        .await?;
        Ok(GuiToolResult::ok(
            format!("Scrolled {} times (dx={}, dy={})", amount, params.dx, params.dy),
            started,
        ))
    }

    async fn run_task(&self, task: String, dry_run: bool) -> Result<Vec<GuiToolResult>, String> {
        let started = Instant::now();

        // Records the task only. A full implementation would:
        // 1. Call UI-TARS operator to propose actions
        // 2. Execute each proposed action
        // 3. Verify with screenshot after each action
        let output = if dry_run {
            format!("[DRY RUN] Task: {}", task)
        } else {
            format!("Task: {}", task)
        };

        Ok(vec![GuiToolResult::ok(output, started)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_dimensions_reads_ihdr() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&2880u32.to_be_bytes());
        png.extend_from_slice(&1800u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((2880, 1800)));
        assert_eq!(png_dimensions(b"not a png at all, really not"), None);
    }

    #[test]
    fn keystroke_script_escapes_quotes_and_backslashes() {
        let script = keystroke_script(r#"say "hi" \ bye"#, 0);
        assert_eq!(
            script,
            r#"tell application "System Events" to keystroke "say \"hi\" \\ bye""#
        );
    }

    #[test]
    fn cliclick_args_map_button_and_count() {
        let params = |button: &str, count| ClickParams {
            x: 10,
            y: 20,
            button: Some(button.to_string()),
            count: Some(count),
        };
        assert_eq!(cliclick_args(&params("left", 2)).unwrap(), vec!["dc:10,20"]);
        assert_eq!(
            cliclick_args(&params("right", 2)).unwrap(),
            vec!["rc:10,20", "rc:10,20"]
        );
        assert!(cliclick_args(&params("middle", 1)).is_err());
    }

    #[test]
    fn scroll_script_needs_a_direction() {
        assert!(scroll_script(0, 0, 3).is_none());
        let script = scroll_script(0, -1, 2).unwrap();
        assert!(script.contains("repeat 2 times\nkey code 126"));
    }
}
