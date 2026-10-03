//! Voice API Service binary
//!
//! HTTP API service for voice synthesis and recognition.
//! Runs on port 8001.

use std::net::SocketAddr;
use tracing::info;
use voice_service::server::{create_router, VoiceServiceState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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
