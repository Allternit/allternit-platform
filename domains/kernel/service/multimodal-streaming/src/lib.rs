//! Multimodal Streaming Execution Module
//!
//! WebRTC-based audio and video streaming for allternit agents.
//!
//! ## Architecture
//!
//! ### Terminal 2 - Agent Assignments:
//! - **T2-A1** (GAP-36, GAP-37, GAP-40): WebRTC setup, AudioChannel, WebAudio API
//! - **T2-A2** (GAP-38): VisionChannel, video decoding
//! - **T2-A3** (GAP-39): FullDuplexController, stream coordination
//! - **T2-A4** (GAP-41, GAP-42): Transport layer (ICE, DTLS, SRTP)
//! - **T2-A5** (GAP-43, GAP-44): Signaling, room management
//!
//! ## Modules
//!
//! - `types`: Shared types for all agents (AudioFrame, VideoFrame, etc.)
//! - `audio`: Audio pipeline (AudioChannel, decoder, buffer)
//! - `video`: Video pipeline (VisionChannel, decoder, buffer)
//! - `webaudio`: WebAudio API integration (AudioContext, nodes)
//! - `controller`: FullDuplexController for stream coordination
//! - `transport`: WebRTC transport (ICE, DTLS, SRTP, data channels)
//! - `signaling`: Signaling protocol (WebSocket, SDP, rooms)
//!
//! ## SYSTEM_LAW Compliance
//!
//! All unimplemented features marked with `STUB_APPROVED`.

#![warn(missing_docs)]
#![allow(dead_code)] // STUB_APPROVED: Some code for future features

pub mod audio;
pub mod controller;
pub mod signaling;
pub mod transport;
pub mod types;
pub mod video;
pub mod webaudio;

// Re-export commonly used types
pub use audio::AudioChannel;
pub use types::*;
pub use webaudio::AudioContext;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Multimodal streaming engine
///
/// Main entry point for audio/video streaming functionality.
/// Coordinates between transport, signaling, and media processing.
pub struct StreamingEngine {
    /// Engine identifier
    engine_id: String,
    /// Active audio channels
    audio_channels: Arc<RwLock<Vec<Arc<audio::AudioChannel>>>>,
    /// Active video channels
    video_channels: Arc<RwLock<Vec<Arc<video::VisionChannel>>>>,
    /// Audio contexts (WebAudio)
    audio_contexts: Arc<RwLock<Vec<Arc<webaudio::AudioContext>>>>,
    /// Transport manager
    transport: Option<Arc<transport::TransportManager>>,
    /// Signaling coordinator
    signaling: Option<Arc<signaling::PeerCoordinator>>,
    /// Full-duplex controller
    controller: Option<Arc<controller::FullDuplexController>>,
}

impl StreamingEngine {
    /// Create a new streaming engine
    pub fn new(engine_id: impl Into<String>) -> Self {
        let engine_id = engine_id.into();
        info!("Creating StreamingEngine {}", engine_id);

        Self {
            engine_id,
            audio_channels: Arc::new(RwLock::new(Vec::new())),
            video_channels: Arc::new(RwLock::new(Vec::new())),
            audio_contexts: Arc::new(RwLock::new(Vec::new())),
            transport: None,
            signaling: None,
            controller: None,
        }
    }

    /// Get engine ID
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Initialize with transport layer
    pub async fn with_transport(mut self, transport: Arc<transport::TransportManager>) -> Self {
        debug!("StreamingEngine {} adding transport layer", self.engine_id);
        self.transport = Some(transport);
        self
    }

    /// Initialize with signaling
    pub async fn with_signaling(mut self, signaling: Arc<signaling::PeerCoordinator>) -> Self {
        debug!("StreamingEngine {} adding signaling", self.engine_id);
        self.signaling = Some(signaling);
        self
    }

    /// Initialize with full-duplex controller
    pub async fn with_controller(
        mut self,
        controller: Arc<controller::FullDuplexController>,
    ) -> Self {
        debug!(
            "StreamingEngine {} adding full-duplex controller",
            self.engine_id
        );
        self.controller = Some(controller);
        self
    }

    /// Create a new audio channel
    pub async fn create_audio_channel(
        &self,
        channel_id: impl Into<String>,
        stream_id: impl Into<String>,
        config: types::AudioConfig,
    ) -> (
        Arc<audio::AudioChannel>,
        tokio::sync::mpsc::UnboundedReceiver<types::AudioFrame>,
    ) {
        let channel_id = channel_id.into();
        let stream_id = stream_id.into();

        let (channel, receiver) =
            audio::AudioChannel::new(channel_id.clone(), stream_id.clone(), config);
        let channel = Arc::new(channel);

        let mut channels = self.audio_channels.write().await;
        channels.push(channel.clone());

        info!(
            "Created audio channel {} for stream {}",
            channel_id, stream_id
        );

        (channel, receiver)
    }

    /// Create a new audio context (WebAudio)
    pub async fn create_audio_context(
        &self,
        context_id: impl Into<String>,
    ) -> (
        Arc<webaudio::AudioContext>,
        tokio::sync::mpsc::UnboundedReceiver<types::AudioFrame>,
    ) {
        let context_id = context_id.into();

        let (context, receiver) = webaudio::AudioContext::new(context_id.clone());
        let context = Arc::new(context);

        let mut contexts = self.audio_contexts.write().await;
        contexts.push(context.clone());

        info!("Created audio context {}", context_id);

        (context, receiver)
    }

    /// Get audio channel count
    pub async fn audio_channel_count(&self) -> usize {
        self.audio_channels.read().await.len()
    }

    /// Get audio context count
    pub async fn audio_context_count(&self) -> usize {
        self.audio_contexts.read().await.len()
    }

    /// Shutdown the engine
    pub async fn shutdown(&self) {
        warn!("Shutting down StreamingEngine {}", self.engine_id);

        // Close all audio channels
        let channels = self.audio_channels.read().await;
        for channel in channels.iter() {
            let _ = channel.close().await;
        }
        drop(channels);

        // Close all audio contexts
        let contexts = self.audio_contexts.read().await;
        for context in contexts.iter() {
            let _ = context.close().await;
        }

        info!("StreamingEngine {} shutdown complete", self.engine_id);
    }
}

/// Initialize the multimodal streaming module
///
/// Call this once at application startup
pub fn init() {
    info!("Initializing multimodal-streaming-execution module");
}

/// Module version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_creation() {
        let engine = StreamingEngine::new("test_engine");
        assert_eq!(engine.engine_id(), "test_engine");
        assert_eq!(engine.audio_channel_count().await, 0);
    }

    #[tokio::test]
    async fn test_create_audio_channel() {
        let engine = StreamingEngine::new("test_engine");
        let config = types::AudioConfig::default();

        let (channel, _receiver) = engine.create_audio_channel("ch1", "stream1", config).await;

        assert_eq!(channel.channel_id(), "ch1");
        assert_eq!(channel.stream_id(), "stream1");
        assert_eq!(engine.audio_channel_count().await, 1);
    }

    #[tokio::test]
    async fn test_create_audio_context() {
        let engine = StreamingEngine::new("test_engine");

        let (context, _receiver) = engine.create_audio_context("ctx1").await;

        assert_eq!(context.context_id(), "ctx1");
        assert_eq!(engine.audio_context_count().await, 1);
    }

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

}
