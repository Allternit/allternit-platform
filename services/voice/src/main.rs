//! Voice API Service binary
//!
//! - `allternit-voice-service` (or `serve`): HTTP API for voice synthesis and
//!   recognition on port 8001.
//! - `allternit-voice-service worker`: the phone call worker (LiveKit agent
//!   `allternit-voice`); needs a build with `--features call-worker`. See
//!   `services/voice/spec/CALL_WORKER.md`.

use std::net::SocketAddr;
use tracing::info;
use voice_service::server::{VoiceServiceState, create_router};

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
    axum::serve(listener, app).await?;

    Ok(())
}
