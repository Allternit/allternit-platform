//! Shared Types for Multimodal Streaming
//!
//! WIH: GAP-37, Owner: T2-A1
//! Coordination interface for:
//! - T2-A2: VisionChannel (GAP-38)
//! - T2-A3: FullDuplexController
//!
//! This module defines shared types that all agents depend on.
//! DO NOT modify without coordinating with dependent agents.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// Core Frame Types - Shared Across All Channels
// ============================================================================

/// Audio frame containing decoded PCM data
///
/// Shared with T2-A2 for synchronization and T2-A3 for duplex control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub frame_id: String,
    pub stream_id: String,
    pub timestamp: DateTime<Utc>,
    /// Decoded PCM audio samples (interleaved for multi-channel)
    pub samples: Vec<f32>,
    /// Sample rate in Hz (e.g., 48000, 44100)
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u16,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Frame metadata
    pub metadata: FrameMetadata,
}

/// Video frame containing image data
///
/// Defined here for T2-A2 to use - coordinate changes with T2-A2
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrame {
    pub frame_id: String,
    pub stream_id: String,
    pub timestamp: DateTime<Utc>,
    /// Raw frame data (format specified in metadata)
    pub data: Vec<u8>,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Frame metadata
    pub metadata: FrameMetadata,
}

/// Synchronized multimodal frame (audio + video)
///
/// Used by T2-A3 FullDuplexController
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalFrame {
    pub sync_id: String,
    pub timestamp: DateTime<Utc>,
    /// Audio component (optional - may be audio-only stream)
    pub audio: Option<AudioFrame>,
    /// Video component (optional - may be audio-only stream)
    pub video: Option<VideoFrame>,
    /// Synchronization offset in milliseconds
    pub sync_offset_ms: i64,
}

// ============================================================================
// Frame Metadata
// ============================================================================

/// Metadata common to all frame types
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrameMetadata {
    /// Encoding format (e.g., "opus", "pcm_f32", "h264", "vp8")
    pub encoding: Option<String>,
    /// Source identifier (e.g., "microphone", "screen", "file")
    pub source: Option<String>,
    /// Sequence number for ordering
    pub sequence: Option<u64>,
    /// Additional key-value metadata
    pub extra: std::collections::HashMap<String, String>,
}

// ============================================================================
// Audio Specific Types
// ============================================================================

/// Audio codec types supported
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    /// Opus codec (primary for WebRTC)
    Opus,
    /// PCM 16-bit signed
    PcmI16,
    /// PCM 32-bit float
    PcmF32,
    /// AAC codec (for compatibility)
    Aac,
}

impl AudioCodec {
    /// Get the MIME type for this codec
    pub fn mime_type(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "audio/opus",
            AudioCodec::PcmI16 => "audio/L16",
            AudioCodec::PcmF32 => "audio/L32",
            AudioCodec::Aac => "audio/aac",
        }
    }
}

/// Audio stream configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: AudioCodec,
    pub bitrate: Option<u32>,
    pub buffer_ms: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            codec: AudioCodec::Opus,
            bitrate: Some(128000),
            buffer_ms: 20,
        }
    }
}

/// Audio processing settings for WebAudio API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAudioSettings {
    /// Sample rate (must match AudioContext)
    pub sample_rate: u32,
    /// Buffer size for processing
    pub buffer_size: u32,
    /// Number of input channels
    pub input_channels: u16,
    /// Number of output channels
    pub output_channels: u16,
    /// Enable echo cancellation
    pub echo_cancellation: bool,
    /// Enable noise suppression
    pub noise_suppression: bool,
    /// Enable automatic gain control
    pub auto_gain_control: bool,
}

impl Default for WebAudioSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            buffer_size: 128,
            input_channels: 2,
            output_channels: 2,
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain_control: true,
        }
    }
}

// ============================================================================
// Stream Control Types
// ============================================================================

/// Stream direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamDirection {
    /// Send only
    Send,
    /// Receive only
    Receive,
    /// Bidirectional
    Bidirectional,
}

/// Stream state
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamState {
    /// Initializing
    Initializing,
    /// Connected and active
    Active,
    /// Paused
    Paused,
    /// Error state
    Error,
    /// Closed
    Closed,
}

/// Stream identifier and info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    pub stream_id: String,
    pub direction: StreamDirection,
    pub state: StreamState,
    pub created_at: DateTime<Utc>,
    pub peer_id: Option<String>,
}

// ============================================================================
// Error Types
// ============================================================================

/// Multimodal streaming errors
#[derive(Debug, thiserror::Error)]
pub enum StreamingError {
    #[error("Codec error: {0}")]
    Codec(String),

    #[error("WebRTC error: {0}")]
    WebRtc(String),

    #[error("Audio processing error: {0}")]
    AudioProcessing(String),

    #[error("Invalid frame format: {0}")]
    InvalidFrameFormat(String),

    #[error("Stream not found: {0}")]
    StreamNotFound(String),

    #[error("Buffer overflow")]
    BufferOverflow,

    #[error("Invalid state transition: {from} -> {to}")]
    InvalidStateTransition {
        /// State the transition started from
        from: String,
        /// State the transition was attempted to
        to: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type alias
pub type StreamingResult<T> = Result<T, StreamingError>;

// ============================================================================
// Traits for Channel Coordination
// ============================================================================

/// Trait for audio frame consumers
///
/// Implemented by T2-A3 FullDuplexController
pub trait AudioFrameConsumer: Send + Sync {
    /// Process an audio frame
    fn on_audio_frame(&self, frame: AudioFrame);
    /// Called when stream ends
    fn on_stream_end(&self, stream_id: &str);
}

/// Trait for video frame consumers
///
/// Implemented by T2-A3 FullDuplexController
pub trait VideoFrameConsumer: Send + Sync {
    /// Process a video frame
    fn on_video_frame(&self, frame: VideoFrame);
    /// Called when stream ends
    fn on_stream_end(&self, stream_id: &str);
}

/// Shared frame sender type
pub type FrameSender = tokio::sync::mpsc::UnboundedSender<MultimodalFrame>;

/// Shared frame receiver type
pub type FrameReceiver = tokio::sync::mpsc::UnboundedReceiver<MultimodalFrame>;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_codec_mime_types() {
        assert_eq!(AudioCodec::Opus.mime_type(), "audio/opus");
        assert_eq!(AudioCodec::PcmI16.mime_type(), "audio/L16");
        assert_eq!(AudioCodec::PcmF32.mime_type(), "audio/L32");
        assert_eq!(AudioCodec::Aac.mime_type(), "audio/aac");
    }

    #[test]
    fn test_audio_config_default() {
        let config = AudioConfig::default();
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.codec, AudioCodec::Opus);
        assert_eq!(config.bitrate, Some(128000));
    }

    #[test]
    fn test_webaudio_settings_default() {
        let settings = WebAudioSettings::default();
        assert_eq!(settings.sample_rate, 48000);
        assert_eq!(settings.buffer_size, 128);
        assert!(settings.echo_cancellation);
        assert!(settings.noise_suppression);
        assert!(settings.auto_gain_control);
    }

    #[test]
    fn test_audio_frame_creation() {
        let frame = AudioFrame {
            frame_id: "test_audio_1".to_string(),
            stream_id: "stream_1".to_string(),
            timestamp: Utc::now(),
            samples: vec![0.0, 0.5, 1.0, 0.5],
            sample_rate: 48000,
            channels: 2,
            duration_ms: 20,
            metadata: FrameMetadata::default(),
        };

        assert_eq!(frame.frame_id, "test_audio_1");
        assert_eq!(frame.samples.len(), 4);
    }

    #[test]
    fn test_multimodal_frame_creation() {
        let frame = MultimodalFrame {
            sync_id: "sync_1".to_string(),
            timestamp: Utc::now(),
            audio: None,
            video: None,
            sync_offset_ms: 0,
        };

        assert_eq!(frame.sync_id, "sync_1");
        assert!(frame.audio.is_none());
        assert!(frame.video.is_none());
    }

    #[test]
    fn test_stream_state_serialization() {
        let states = vec![
            StreamState::Initializing,
            StreamState::Active,
            StreamState::Paused,
            StreamState::Error,
            StreamState::Closed,
        ];

        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let deserialized: StreamState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, deserialized);
        }
    }
}
