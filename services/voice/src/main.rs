//! Voice API Service binary
//!
//! - `allternit-voice-service` (or `serve`): HTTP API for voice synthesis and
//!   recognition on port 8001.
//! - `allternit-voice-service worker`: the phone call worker (LiveKit agent
//!   `allternit-voice`); needs a build with `--features call-worker`. See
//!   `services/voice/spec/CALL_WORKER.md`.

use std::net::SocketAddr;
use tracing::info;
use voice_service::server::{create_router, VoiceServiceState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => {}
        Some("worker") => return voice_service::call_worker::run_worker().await,
        Some(other) => anyhow::bail!("unknown subcommand `{other}` (expected `serve` or `worker`)"),
    }

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8001);

    info!("Starting Voice API Service on port {port}...");

    let state = VoiceServiceState::new();
    let app = create_router(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("Voice API Service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Keep espeak-ng (GPL-3.0-or-later) out of this binary.
///
/// TTS runs in the separate GPL program `allternit-tts`. But sherpa-onnx's
/// prebuilt static C API puts the TTS and recogniser entry points in one
/// `.text` section on Linux (and similar on Windows), so `--gc-sections`
/// cannot drop the TTS path, and the linker would pull `libespeak-ng.a` in
/// for the three espeak functions it references. Defining those three here
/// (in the binary's own objects, which are linked before any static
/// library) means no espeak-ng archive member is ever pulled. If sherpa-onnx
/// ever needs another espeak symbol, that member gets pulled, clashes with
/// these definitions, and the link fails loudly instead of silently
/// shipping GPL code. They are never called: this binary never creates a
/// TTS engine. Verified with `nm` (see services/voice/README.md).
mod no_espeak {
    use std::os::raw::{c_char, c_int, c_void};

    const EE_INTERNAL_ERROR: c_int = -1;

    #[no_mangle]
    pub extern "C" fn espeak_Initialize(
        _output: c_int,
        _buflength: c_int,
        _path: *const c_char,
        _options: c_int,
    ) -> c_int {
        EE_INTERNAL_ERROR
    }

    #[no_mangle]
    pub extern "C" fn espeak_SetVoiceByName(_name: *const c_char) -> c_int {
        EE_INTERNAL_ERROR
    }

    /// # Safety
    /// Called only by C code with a valid (or null) `textptr`.
    #[no_mangle]
    pub unsafe extern "C" fn espeak_TextToPhonemesWithTerminator(
        textptr: *mut *const c_void,
        _textmode: c_int,
        _phonememode: c_int,
        _terminator: *mut c_int,
    ) -> *const c_char {
        if !textptr.is_null() {
            // End any caller loop that walks the text.
            unsafe { *textptr = std::ptr::null() };
        }
        std::ptr::null()
    }
}
