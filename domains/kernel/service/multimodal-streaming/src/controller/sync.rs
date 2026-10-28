//! Audio/Video Synchronization
//!
//! GAP-39: Stream synchronization logic for FullDuplexController
//! WIH: GAP-39, Owner: T2-A3, Dependencies: T2-A1, T2-A2
//!
//! Implements timestamp-based synchronization between audio and video streams
//! to maintain lip-sync and temporal alignment.

use crate::types::{AudioFrame, MultimodalFrame, VideoFrame};
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use tracing::{debug, trace, warn};

/// Maximum allowed drift between audio and video before resync (milliseconds)
pub(crate) const MAX_SYNC_DRIFT_MS: i64 = 40;

/// Target synchronization window (milliseconds)
const SYNC_WINDOW_MS: i64 = 20;

/// Audio/video synchronizer for maintaining temporal alignment
///
/// Uses a sliding window buffer to align incoming audio and video frames
/// based on their presentation timestamps.
#[derive(Debug)]
pub struct StreamSynchronizer {
    /// Audio frame buffer (sorted by timestamp)
    audio_buffer: VecDeque<AudioFrame>,
    /// Video frame buffer (sorted by timestamp)
    video_buffer: VecDeque<VideoFrame>,
    /// Maximum buffer size before dropping old frames
    max_buffer_size: usize,
    /// Last synchronization point
    last_sync_time: Option<DateTime<Utc>>,
    /// Total frames synchronized
    sync_count: u64,
    /// Total frames dropped due to timeout
    dropped_count: u64,
}

impl StreamSynchronizer {
    /// Create a new stream synchronizer
    pub fn new() -> Self {
        Self {
            audio_buffer: VecDeque::new(),
            video_buffer: VecDeque::new(),
            max_buffer_size: 30, // ~1 second at 30fps
            last_sync_time: None,
            sync_count: 0,
            dropped_count: 0,
        }
    }

    /// Create with custom buffer size
    pub fn with_buffer_size(max_buffer_size: usize) -> Self {
        Self {
            audio_buffer: VecDeque::with_capacity(max_buffer_size),
            video_buffer: VecDeque::with_capacity(max_buffer_size),
            max_buffer_size,
            last_sync_time: None,
            sync_count: 0,
            dropped_count: 0,
        }
    }

    /// Add an audio frame to the synchronization buffer
    pub fn add_audio_frame(&mut self, frame: AudioFrame) {
        trace!(
            "Adding audio frame {} at {}",
            frame.frame_id,
            frame.timestamp
        );

        // Maintain sorted order by timestamp
        let pos = self
            .audio_buffer
            .iter()
            .position(|f| f.timestamp > frame.timestamp)
            .unwrap_or(self.audio_buffer.len());
        self.audio_buffer.insert(pos, frame);

        // Enforce buffer limit
        while self.audio_buffer.len() > self.max_buffer_size {
            self.audio_buffer.pop_front();
            self.dropped_count += 1;
        }
    }

    /// Add a video frame to the synchronization buffer
    pub fn add_video_frame(&mut self, frame: VideoFrame) {
        trace!(
            "Adding video frame {} at {}",
            frame.frame_id,
            frame.timestamp
        );

        // Maintain sorted order by timestamp
        let pos = self
            .video_buffer
            .iter()
            .position(|f| f.timestamp > frame.timestamp)
            .unwrap_or(self.video_buffer.len());
        self.video_buffer.insert(pos, frame);

        // Enforce buffer limit
        while self.video_buffer.len() > self.max_buffer_size {
            self.video_buffer.pop_front();
            self.dropped_count += 1;
        }
    }

    /// Attempt to create a synchronized multimodal frame
    ///
    /// Returns a MultimodalFrame if matching audio and video frames are found
    /// within the synchronization window. Returns None if no match is available.
    pub fn try_sync(&mut self) -> Option<MultimodalFrame> {
        // Adaptive sync algorithm that finds closest matching frames
        
        if self.audio_buffer.is_empty() && self.video_buffer.is_empty() {
            return None;
        }

        // Determine sync timestamp
        let sync_timestamp = if let (Some(audio), Some(video)) =
            (self.audio_buffer.front(), self.video_buffer.front())
        {
            // Use the later timestamp to ensure both streams have data
            audio.timestamp.max(video.timestamp)
        } else if let Some(audio) = self.audio_buffer.front() {
            audio.timestamp
        } else if let Some(video) = self.video_buffer.front() {
            video.timestamp
        } else {
            return None;
        };

        // Find best matching audio frame
        let audio_match = self.find_best_audio_match(sync_timestamp);

        // Find best matching video frame
        let video_match = self.find_best_video_match(sync_timestamp);

        // Calculate sync offset
        let sync_offset_ms = match (&audio_match, &video_match) {
            (Some(audio), Some(video)) => {
                let diff = video.timestamp.signed_duration_since(audio.timestamp);
                diff.num_milliseconds()
            }
            _ => 0,
        };

        // Check if sync is within acceptable drift
        if sync_offset_ms.abs() > MAX_SYNC_DRIFT_MS {
            warn!(
                "Sync drift {}ms exceeds threshold ({}ms), forcing sync",
                sync_offset_ms, MAX_SYNC_DRIFT_MS
            );
            // Still sync but mark as forced
        }

        // Create sync ID
        let sync_id = format!(
            "sync_{}_{}",
            Utc::now().timestamp_millis(),
            self.sync_count
        );

        // Remove matched frames from buffers
        if audio_match.is_some() {
            self.remove_audio_frame_by_timestamp(sync_timestamp);
        }
        if video_match.is_some() {
            self.remove_video_frame_by_timestamp(sync_timestamp);
        }

        self.sync_count += 1;
        self.last_sync_time = Some(Utc::now());

        debug!(
            "Created synchronized frame {} (offset: {}ms, audio: {}, video: {})",
            sync_id,
            sync_offset_ms,
            audio_match.is_some(),
            video_match.is_some()
        );

        Some(MultimodalFrame {
            sync_id,
            timestamp: sync_timestamp,
            audio: audio_match,
            video: video_match,
            sync_offset_ms,
        })
    }

    /// Find the best matching audio frame for a target timestamp
    fn find_best_audio_match(&self, target: DateTime<Utc>) -> Option<AudioFrame> {
        self.audio_buffer
            .iter()
            .min_by_key(|f| (f.timestamp.signed_duration_since(target)).num_milliseconds().abs())
            .cloned()
            .filter(|f| {
                let diff = f.timestamp.signed_duration_since(target).num_milliseconds().abs();
                diff <= SYNC_WINDOW_MS * 2
            })
    }

    /// Find the best matching video frame for a target timestamp
    fn find_best_video_match(&self, target: DateTime<Utc>) -> Option<VideoFrame> {
        self.video_buffer
            .iter()
            .min_by_key(|f| (f.timestamp.signed_duration_since(target)).num_milliseconds().abs())
            .cloned()
            .filter(|f| {
                let diff = f.timestamp.signed_duration_since(target).num_milliseconds().abs();
                diff <= SYNC_WINDOW_MS * 2
            })
    }

    /// Remove audio frame closest to the given timestamp
    fn remove_audio_frame_by_timestamp(&mut self, target: DateTime<Utc>) {
        if let Some(pos) = self
            .audio_buffer
            .iter()
            .enumerate()
            .min_by_key(|(_, f)| {
                (f.timestamp.signed_duration_since(target)).num_milliseconds().abs()
            })
            .map(|(i, _)| i)
        {
            self.audio_buffer.remove(pos);
        }
    }

    /// Remove video frame closest to the given timestamp
    fn remove_video_frame_by_timestamp(&mut self, target: DateTime<Utc>) {
        if let Some(pos) = self
            .video_buffer
            .iter()
            .enumerate()
            .min_by_key(|(_, f)| {
                (f.timestamp.signed_duration_since(target)).num_milliseconds().abs()
            })
            .map(|(i, _)| i)
        {
            self.video_buffer.remove(pos);
        }
    }

    /// Flush all pending frames and return them as best-effort synchronized pairs
    pub fn flush(&mut self) -> Vec<MultimodalFrame> {
        let mut result = Vec::new();

        while !self.audio_buffer.is_empty() || !self.video_buffer.is_empty() {
            if let Some(frame) = self.try_sync() {
                result.push(frame);
            } else {
                // Create partial frames for remaining data
                if let Some(audio) = self.audio_buffer.pop_front() {
                    result.push(MultimodalFrame {
                        sync_id: format!("sync_audio_{}", audio.frame_id),
                        timestamp: audio.timestamp,
                        audio: Some(audio),
                        video: None,
                        sync_offset_ms: 0,
                    });
                }
                if let Some(video) = self.video_buffer.pop_front() {
                    result.push(MultimodalFrame {
                        sync_id: format!("sync_video_{}", video.frame_id),
                        timestamp: video.timestamp,
                        audio: None,
                        video: Some(video),
                        sync_offset_ms: 0,
                    });
                }
            }
        }

        result
    }

    /// Clear all buffers
    pub fn clear(&mut self) {
        self.audio_buffer.clear();
        self.video_buffer.clear();
        self.last_sync_time = None;
    }

    /// Get current buffer statistics
    pub fn stats(&self) -> SyncStats {
        SyncStats {
            audio_buffered: self.audio_buffer.len(),
            video_buffered: self.video_buffer.len(),
            sync_count: self.sync_count,
            dropped_count: self.dropped_count,
            last_sync_time: self.last_sync_time,
        }
    }

    /// Get the number of audio frames buffered
    pub fn audio_buffered(&self) -> usize {
        self.audio_buffer.len()
    }

    /// Get the number of video frames buffered
    pub fn video_buffered(&self) -> usize {
        self.video_buffer.len()
    }
}

impl Default for StreamSynchronizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Synchronization statistics
#[derive(Debug, Clone)]
pub struct SyncStats {
    pub audio_buffered: usize,
    pub video_buffered: usize,
    pub sync_count: u64,
    pub dropped_count: u64,
    pub last_sync_time: Option<DateTime<Utc>>,
}

impl std::fmt::Display for SyncStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SyncStats: {} audio, {} video buffered, {} synced, {} dropped",
            self.audio_buffered, self.video_buffered, self.sync_count, self.dropped_count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FrameMetadata, AudioCodec, AudioConfig};
    use chrono::Duration;

    fn create_test_audio_frame(frame_id: &str, timestamp: DateTime<Utc>) -> AudioFrame {
        AudioFrame {
            frame_id: frame_id.to_string(),
            stream_id: "test_stream".to_string(),
            timestamp,
            samples: vec![0.0; 960], // 20ms at 48kHz
            sample_rate: 48000,
            channels: 2,
            duration_ms: 20,
            metadata: FrameMetadata {
                encoding: Some("opus".to_string()),
                source: Some("test".to_string()),
                sequence: Some(0),
                extra: Default::default(),
            },
        }
    }

    fn create_test_video_frame(frame_id: &str, timestamp: DateTime<Utc>) -> VideoFrame {
        VideoFrame {
            frame_id: frame_id.to_string(),
            stream_id: "test_stream".to_string(),
            timestamp,
            data: vec![0u8; 1000],
            width: 1920,
            height: 1080,
            metadata: FrameMetadata {
                encoding: Some("h264".to_string()),
                source: Some("test".to_string()),
                sequence: Some(0),
                extra: Default::default(),
            },
        }
    }

    #[test]
    fn test_synchronizer_creation() {
        let sync = StreamSynchronizer::new();
        assert_eq!(sync.audio_buffered(), 0);
        assert_eq!(sync.video_buffered(), 0);
        assert_eq!(sync.stats().sync_count, 0);
    }

    #[test]
    fn test_add_audio_frame() {
        let mut sync = StreamSynchronizer::new();
        let frame = create_test_audio_frame("audio_1", Utc::now());
        sync.add_audio_frame(frame);
        assert_eq!(sync.audio_buffered(), 1);
    }

    #[test]
    fn test_add_video_frame() {
        let mut sync = StreamSynchronizer::new();
        let frame = create_test_video_frame("video_1", Utc::now());
        sync.add_video_frame(frame);
        assert_eq!(sync.video_buffered(), 1);
    }

    #[test]
    fn test_try_sync_matched_frames() {
        let mut sync = StreamSynchronizer::new();
        let now = Utc::now();

        let audio = create_test_audio_frame("audio_1", now);
        let video = create_test_video_frame("video_1", now);

        sync.add_audio_frame(audio);
        sync.add_video_frame(video);

        let result = sync.try_sync();
        assert!(result.is_some());

        let frame = result.unwrap();
        assert!(frame.audio.is_some());
        assert!(frame.video.is_some());
        assert_eq!(frame.sync_offset_ms, 0);
    }

    #[test]
    fn test_try_sync_drift_tolerance() {
        let mut sync = StreamSynchronizer::new();
        let base = Utc::now();

        // Audio at t=0, video at t=30ms (within 40ms threshold)
        let audio = create_test_audio_frame("audio_1", base);
        let video = create_test_video_frame("video_1", base + Duration::milliseconds(30));

        sync.add_audio_frame(audio);
        sync.add_video_frame(video);

        let result = sync.try_sync();
        assert!(result.is_some());
        assert_eq!(result.unwrap().sync_offset_ms, 30);
    }

    #[test]
    fn test_buffer_limits() {
        let mut sync = StreamSynchronizer::with_buffer_size(5);
        let base = Utc::now();

        // Add more frames than buffer size
        for i in 0..10 {
            let frame = create_test_audio_frame(&format!("audio_{}", i), base + Duration::milliseconds(i * 10));
            sync.add_audio_frame(frame);
        }

        // Buffer should only hold max_buffer_size frames
        assert_eq!(sync.audio_buffered(), 5);
        assert!(sync.stats().dropped_count > 0);
    }

    #[test]
    fn test_flush() {
        let mut sync = StreamSynchronizer::new();
        let base = Utc::now();

        let audio = create_test_audio_frame("audio_1", base);
        let video = create_test_video_frame("video_1", base + Duration::milliseconds(100));

        sync.add_audio_frame(audio);
        sync.add_video_frame(video);

        let flushed = sync.flush();
        assert!(!flushed.is_empty());
        assert_eq!(sync.audio_buffered(), 0);
        assert_eq!(sync.video_buffered(), 0);
    }

    #[test]
    fn test_clear() {
        let mut sync = StreamSynchronizer::new();
        let now = Utc::now();

        sync.add_audio_frame(create_test_audio_frame("audio_1", now));
        sync.add_video_frame(create_test_video_frame("video_1", now));

        sync.clear();
        assert_eq!(sync.audio_buffered(), 0);
        assert_eq!(sync.video_buffered(), 0);
    }

    #[test]
    fn test_sync_stats_display() {
        let stats = SyncStats {
            audio_buffered: 5,
            video_buffered: 3,
            sync_count: 100,
            dropped_count: 2,
            last_sync_time: Some(Utc::now()),
        };

        let display = format!("{}", stats);
        assert!(display.contains("5 audio"));
        assert!(display.contains("3 video"));
        assert!(display.contains("100 synced"));
    }
}
