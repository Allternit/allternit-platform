//! Ask before waking (SPEC section 9, plan decision D7).
//!
//! Listing and the change feed never come here: the runtime answers them from the
//! asleep snapshot. Only content (open, upload, write) can get
//! `wake_consent_required`; then the computer's setting decides:
//! - `ask` (default): prompt the user once; "Not now" is remembered for a minute so
//!   a burst of reads does not stack prompts.
//! - `auto`: get a ticket without asking.
//! - `never`: fail.
//! Background readers (indexer, antivirus, preview hosts) never prompt and never wake.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use allternit_files_core::{ErrorCode, WakeHint, WakeReason, WakeSetting};

use crate::client::{ApiError, FilesClient};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeAnswer {
    Wake,
    NotNow,
}

/// Shows the wake prompt. The Windows build uses a message box; tests answer directly.
pub trait WakePrompt: Send + Sync {
    fn ask(&self, computer_name: &str, what: &str) -> WakeAnswer;
}

/// How long "Not now" holds before the next read may ask again.
pub const DECLINE_HOLD: Duration = Duration::from_secs(60);
/// The longest the helper waits for a `waking` computer (the relay waits 30 s per request).
pub const MAX_WAKING_WAIT: Duration = Duration::from_secs(120);

#[derive(Default)]
struct GateState {
    ticket: Option<(String, Instant)>,
    declined_until: Option<Instant>,
}

pub struct WakeGate {
    computer_name: String,
    prompt: Box<dyn WakePrompt>,
    state: Mutex<GateState>,
    asking: Mutex<()>,
    decline_hold: Duration,
    /// Times the consent call ran (each one may wake the computer).
    pub consent_calls: AtomicU64,
    /// Times the user was asked.
    pub prompts: AtomicU64,
}

/// What the request is for, in the prompt's words.
pub struct WakeContext<'a> {
    pub reason: WakeReason,
    pub item_id: Option<&'a str>,
    /// e.g. `Opening "Lease.pdf"`.
    pub what: String,
    /// A background reader: never prompt, never wake.
    pub background: bool,
}

impl WakeGate {
    pub fn new(computer_name: &str, prompt: Box<dyn WakePrompt>) -> Self {
        WakeGate {
            computer_name: computer_name.to_string(),
            prompt,
            state: Mutex::new(GateState::default()),
            asking: Mutex::new(()),
            decline_hold: DECLINE_HOLD,
            consent_calls: AtomicU64::new(0),
            prompts: AtomicU64::new(0),
        }
    }

    /// Changes how long "Not now" holds (tests use zero).
    pub fn with_decline_hold(mut self, hold: Duration) -> Self {
        self.decline_hold = hold;
        self
    }

    /// A ticket still inside its lifetime.
    pub fn current_ticket(&self) -> Option<String> {
        let s = self.state.lock().unwrap();
        s.ticket.as_ref().filter(|(_, exp)| *exp > Instant::now()).map(|(t, _)| t.clone())
    }

    fn declined(&self) -> bool {
        self.state.lock().unwrap().declined_until.is_some_and(|t| t > Instant::now())
    }

    /// A ticket, or None when the user (or the setting) said no.
    fn obtain(&self, client: &FilesClient, hint: Option<&WakeHint>, ctx: &WakeContext) -> Option<String> {
        let setting = hint.map(|h| h.setting).unwrap_or(WakeSetting::Ask);
        if setting == WakeSetting::Never || ctx.background {
            return None;
        }
        // One prompt at a time; whoever waited may find a ticket already there.
        let _one = self.asking.lock().unwrap();
        if let Some(t) = self.current_ticket() {
            return Some(t);
        }
        if setting == WakeSetting::Ask {
            if self.declined() {
                return None;
            }
            self.prompts.fetch_add(1, Ordering::SeqCst);
            if self.prompt.ask(&self.computer_name, &ctx.what) == WakeAnswer::NotNow {
                self.state.lock().unwrap().declined_until = Some(Instant::now() + self.decline_hold);
                return None;
            }
        }
        self.consent_calls.fetch_add(1, Ordering::SeqCst);
        let resp = client.wake_consent(ctx.reason, ctx.item_id).ok()?;
        let life = chrono::DateTime::parse_from_rfc3339(&resp.expires_at)
            .ok()
            .and_then(|exp| (exp.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().ok())
            .unwrap_or(Duration::from_secs(120))
            .saturating_sub(Duration::from_secs(10));
        let mut s = self.state.lock().unwrap();
        s.ticket = Some((resp.ticket.clone(), Instant::now() + life));
        s.declined_until = None;
        Some(resp.ticket)
    }

    /// Runs `f`, adding a wake ticket when the server asks for consent, and waiting
    /// while the computer is `waking`.
    pub fn run<T>(
        &self,
        client: &FilesClient,
        ctx: &WakeContext,
        mut f: impl FnMut(Option<&str>) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let mut ticket = self.current_ticket();
        let mut asked = false;
        let started = Instant::now();
        loop {
            match f(ticket.as_deref()) {
                Err(ApiError::Files { status, body }) if body.code == ErrorCode::WakeConsentRequired && !asked => {
                    asked = true;
                    match self.obtain(client, body.wake.as_ref(), ctx) {
                        Some(t) => ticket = Some(t),
                        None => return Err(ApiError::Files { status, body }),
                    }
                }
                Err(ApiError::Files { status, body }) if body.code == ErrorCode::Waking => {
                    if started.elapsed() > MAX_WAKING_WAIT {
                        return Err(ApiError::Files { status, body });
                    }
                    std::thread::sleep(Duration::from_secs(body.retry_after_s.unwrap_or(5).clamp(1, 10)));
                }
                other => return other,
            }
        }
    }
}

/// Programs that read files without the user asking: they never wake a computer.
pub fn is_background_reader(image_path: &str) -> bool {
    let name = image_path.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    matches!(
        name.as_str(),
        "searchprotocolhost.exe"
            | "searchindexer.exe"
            | "searchfilterhost.exe"
            | "msmpeng.exe"
            | "mpdefendercoreservice.exe"
            | "prevhost.exe"
            | "dllhost.exe"
            | "thumbnailextractionhost.exe"
    )
}

/// The Windows prompt: a top-most Yes/No box, same words as the design (pick 3).
#[cfg(windows)]
pub struct MessageBoxPrompt;

#[cfg(windows)]
impl WakePrompt for MessageBoxPrompt {
    fn ask(&self, computer_name: &str, what: &str) -> WakeAnswer {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let title = HSTRING::from(format!("Wake {computer_name}?"));
        let text = HSTRING::from(format!(
            "{computer_name} is asleep. {what} needs it awake.\n\n\
             Waking takes about 40 seconds and bills credits at your plan's awake rate while it is awake. \
             Browsing the saved list is free.\n\n\
             Wake it now? To stop asking, set this computer to wake automatically in the Files tab."
        ));
        let style = MESSAGEBOX_STYLE(MB_YESNO.0 | MB_ICONQUESTION.0 | MB_SETFOREGROUND.0 | MB_TOPMOST.0);
        let r = unsafe { MessageBoxW(HWND::default(), &text, &title, style) };
        if r == IDYES {
            WakeAnswer::Wake
        } else {
            WakeAnswer::NotNow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_background_reader;

    #[test]
    fn indexer_and_antivirus_are_background_but_apps_are_not() {
        assert!(is_background_reader(r"C:\Windows\System32\SearchProtocolHost.exe"));
        assert!(is_background_reader(r"C:\ProgramData\Microsoft\Windows Defender\MsMpEng.exe"));
        assert!(!is_background_reader(r"C:\Windows\explorer.exe"));
        assert!(!is_background_reader(r"C:\Program Files\Microsoft Office\WINWORD.EXE"));
    }
}
