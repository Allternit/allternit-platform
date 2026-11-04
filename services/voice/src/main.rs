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
    if warm_on_boot(std::env::var("ALLTERNIT_VOICE_WARM").ok().as_deref()) {
        // A server that takes calls loads the default speech models now, so
        // the first call after a restart doesn't wait seconds for them (the
        // call worker gives a session a few seconds to be ready). Desktop
        // leaves this off and loads on first use.
        let (stt, tts) = (state.stt(), state.tts());
        tokio::task::spawn_blocking(move || {
            let t = std::time::Instant::now();
            if let Err(e) = stt.prepare(voice_service::stt::SttModel::Moonshine) {
                tracing::warn!("warm-up: STT not loaded: {e}");
            }
            if let Err(e) = tts.prepare() {
                tracing::warn!("warm-up: TTS not loaded: {e}");
            }
            tts.prewarm_fixed_phrases(None);
            info!("warm-up done in {} ms", t.elapsed().as_millis());
        });
    }
    // The Desktop app's UI (http://127.0.0.1:8013) reads /v1/models and
    // /v1/voices straight from here. Without CORS the browser blocks those
    // reads, so Settings and live mode said "voice engine not installed"
    // while every pack was ready. Only loopback pages may read.
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(|origin, _| {
            origin.to_str().ok().is_some_and(is_loopback_origin)
        }))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);
    let app = create_router(state).layer(cors);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("Voice API Service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;

    Ok(())
}

/// `ALLTERNIT_VOICE_WARM=1|true` loads the default models at startup.
fn warm_on_boot(v: Option<&str>) -> bool {
    matches!(v.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("1" | "true" | "yes"))
}

#[cfg(test)]
mod warm_tests {
    #[test]
    fn warm_only_when_asked() {
        assert!(super::warm_on_boot(Some("1")));
        assert!(super::warm_on_boot(Some("true")));
        assert!(!super::warm_on_boot(None));
        assert!(!super::warm_on_boot(Some("0")));
    }
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

/// `http://127.0.0.1:<port>`, `http://localhost:<port>` or `http://[::1]:<port>`.
fn is_loopback_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else { return false };
    let host = rest.rsplit_once(':').map_or(rest, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) { h } else { rest }
    });
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::is_loopback_origin;

    #[test]
    fn only_loopback_origins_read() {
        assert!(is_loopback_origin("http://127.0.0.1:8013"));
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(is_loopback_origin("http://[::1]:8013"));
        assert!(!is_loopback_origin("https://evil.example"));
        assert!(!is_loopback_origin("http://127.0.0.1.evil.example:80"));
        assert!(!is_loopback_origin("http://localhost.evil.example"));
    }
}
