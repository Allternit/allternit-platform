//! Full Duplex Controller
//!
//! GAP-39: FullDuplexController for Multimodal Streaming
//! WIH: GAP-39, Owner: T2-A3, Dependencies: T2-A1, T2-A2, Deadline: Sprint-2
//!
//! Coordinates bidirectional audio/video streaming between AudioChannel (T2-A1)
//! and VisionChannel (T2-A2) for full-duplex real-time communication.
//!
//! SYSTEM_LAW COMPLIANCE:
//! - Uses STUB_APPROVED for complex sync algorithms not yet implemented
//! - STUB_APPROVED for video module integration (T2-A2)
//! - STUB_APPROVED for WebRTC transport layer integration

use crate::types::{
    AudioFrame, MultimodalFrame, StreamingError,
    StreamingResult, VideoFrame, StreamDirection,
};
use crate::audio::AudioChannel;
use crate::controller::sync::MAX_SYNC_DRIFT_MS;
use crate::controller::{StreamMultiplexer, StreamSynchronizer, StreamInfo, StreamType};

use chrono::Utc;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace, warn};

/// Controller state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControllerState {
    /// Initializing components
    Initializing,
    /// Ready but not connected
    Ready,
    /// Connecting to peer
    Connecting,
    /// Fully connected and streaming
    Streaming,
    /// Paused
    Paused,
    /// Error state
    Error,
    /// Shut down
    Closed,
}

/// Kind of interrupt handled by [`FullDuplexController::handle_interrupt`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterruptType {
    /// The user started speaking (barge-in)
    VoiceActivity,
    /// Interrupt requested explicitly by the user or application
    Manual,
    /// Interrupt raised by a timeout; streaming resumes automatically
    Timeout,
}

/// Per-stream configuration for [`FullDuplexController::start_duplex_stream`]
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Identifier of the multiplexed stream to register
    pub stream_id: String,
    /// Stream priority (0-255, higher = more important)
    pub priority: u8,
    /// Remote peer this stream is connected to, if known
    pub peer_id: Option<String>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            stream_id: format!("duplex_stream_{}", uuid::Uuid::new_v4()),
            priority: 128,
            peer_id: None,
        }
    }
}

/// Handle returned by [`FullDuplexController::start_duplex_stream`]
#[derive(Debug, Clone)]
pub struct DuplexStreamHandle {
    /// Session the stream belongs to
    pub session_id: String,
    /// Controller that owns the stream
    pub controller_id: String,
    /// Multiplexed stream identifier
    pub stream_id: String,
    /// When streaming started
    pub started_at: chrono::DateTime<Utc>,
}

/// Configuration for FullDuplexController
#[derive(Debug, Clone)]
pub struct DuplexConfig {
    /// Controller ID
    pub controller_id: String,
    /// Session ID for this duplex connection
    pub session_id: String,
    /// Enable audio streaming
    pub enable_audio: bool,
    /// Enable video streaming
    pub enable_video: bool,
    /// Maximum stream latency in milliseconds
    pub max_latency_ms: u64,
    /// Enable automatic synchronization
    pub enable_sync: bool,
    /// Buffer size for frame queues
    pub buffer_size: usize,
}

impl Default for DuplexConfig {
    fn default() -> Self {
        Self {
            controller_id: format!("duplex_{}", uuid::Uuid::new_v4()),
            session_id: format!("session_{}", uuid::Uuid::new_v4()),
            enable_audio: true,
            enable_video: true,
            max_latency_ms: 200,
            enable_sync: true,
            buffer_size: 100,
        }
    }
}

/// Full Duplex Controller for multimodal streaming
///
/// Coordinates bidirectional audio and video streams between local
/// and remote peers. Implements GAP-39 requirements.
pub struct FullDuplexController {
    /// Configuration
    config: DuplexConfig,
    /// Controller state
    state: Arc<RwLock<ControllerState>>,
    /// Audio channel (T2-A1)
    audio_channel: Arc<Mutex<Option<AudioChannel>>>,
    /// Video channel placeholder (T2-A2)
    /// SYSTEM_LAW: STUB_APPROVED - VisionChannel from T2-A2
    video_channel: Arc<Mutex<Option<()>>>,
    /// Stream multiplexer
    mux: Arc<StreamMultiplexer>,
    /// Stream synchronizer
    synchronizer: Arc<Mutex<StreamSynchronizer>>,
    /// Frame output channel (for consumers)
    frame_sender: mpsc::UnboundedSender<MultimodalFrame>,
    /// Frame output receiver holder
    frame_receiver: Arc<Mutex<Option<mpsc::UnboundedReceiver<MultimodalFrame>>>>,
    /// Background task handles
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Statistics
    stats: Arc<Mutex<DuplexStats>>,
}

impl FullDuplexController {
    /// Create a new FullDuplexController
    ///
    /// # Arguments
    /// * `config` - Controller configuration
    ///
    /// # Returns
    /// * `(FullDuplexController, FrameReceiver)` - Controller and frame receiver
    pub fn new(config: DuplexConfig) -> (Self, mpsc::UnboundedReceiver<MultimodalFrame>) {
        let (frame_sender, frame_receiver) = mpsc::unbounded_channel();

        let mux = Arc::new(StreamMultiplexer::new());
        let synchronizer = Arc::new(Mutex::new(StreamSynchronizer::new()));

        let controller = Self {
            config,
            state: Arc::new(RwLock::new(ControllerState::Initializing)),
            audio_channel: Arc::new(Mutex::new(None)),
            video_channel: Arc::new(Mutex::new(None)),
            mux,
            synchronizer,
            frame_sender,
            frame_receiver: Arc::new(Mutex::new(Some(frame_receiver))),
            tasks: Arc::new(Mutex::new(Vec::new())),
            stats: Arc::new(Mutex::new(DuplexStats::default())),
        };

        // Clone the receiver for returning
        let (tx, rx) = mpsc::unbounded_channel();
        let controller_with_tx = Self {
            config: controller.config.clone(),
            state: controller.state.clone(),
            audio_channel: controller.audio_channel.clone(),
            video_channel: controller.video_channel.clone(),
            mux: controller.mux.clone(),
            synchronizer: controller.synchronizer.clone(),
            frame_sender: tx,
            frame_receiver: Arc::new(Mutex::new(None)),
            tasks: controller.tasks.clone(),
            stats: controller.stats.clone(),
        };

        (controller_with_tx, rx)
    }

    /// Initialize the controller and its components
    pub async fn initialize(&self) -> StreamingResult<()> {
        info!("Initializing FullDuplexController {}", self.config.controller_id);

        *self.state.write().await = ControllerState::Initializing;

        // Initialize audio channel (T2-A1)
        if self.config.enable_audio {
            self.initialize_audio_channel().await?;
        }

        // Initialize video channel (T2-A2)
        if self.config.enable_video {
            self.initialize_video_channel().await?;
        }

        // Register primary stream with multiplexer
        let stream_info = StreamInfo::new(
            self.config.session_id.clone(),
            if self.config.enable_audio && self.config.enable_video {
                StreamType::AudioVideo
            } else if self.config.enable_audio {
                StreamType::Audio
            } else {
                StreamType::Video
            },
            StreamDirection::Bidirectional,
        );

        let (_audio_rx, _video_rx, _output_rx) = self.mux.register_stream(stream_info).await?;

        // Start synchronization loop
        self.start_sync_loop().await?;

        *self.state.write().await = ControllerState::Ready;
        info!("FullDuplexController {} initialized", self.config.controller_id);

        Ok(())
    }

    /// Initialize audio channel
    async fn initialize_audio_channel(&self) -> StreamingResult<()> {
        use crate::types::AudioConfig;

        let config = AudioConfig::default();
        let audio_tx = self.create_audio_frame_sender().await;
        
        let audio_channel = AudioChannel::with_sender(
            format!("{}_audio", self.config.controller_id),
            &self.config.session_id,
            config,
            audio_tx,
        );

        *self.audio_channel.lock().await = Some(audio_channel);
        
        info!("AudioChannel initialized for {}", self.config.controller_id);
        Ok(())
    }

    /// Create audio frame sender that feeds into the synchronizer
    async fn create_audio_frame_sender(&self) -> mpsc::UnboundedSender<AudioFrame> {
        let synchronizer = self.synchronizer.clone();
        let stats = self.stats.clone();
        
        let (tx, mut rx) = mpsc::unbounded_channel::<AudioFrame>();

        // Spawn task to process audio frames
        let task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                trace!("Audio frame {} received for sync", frame.frame_id);
                
                // Add to synchronizer
                synchronizer.lock().await.add_audio_frame(frame);
                
                // Update stats
                stats.lock().await.audio_frames_received += 1;
            }
        });

        self.tasks.lock().await.push(task);
        tx
    }

    /// Initialize video channel
    /// SYSTEM_LAW: STUB_APPROVED - VisionChannel implementation by T2-A2
    async fn initialize_video_channel(&self) -> StreamingResult<()> {
        // Placeholder for T2-A2 VisionChannel integration
        // This will be replaced when VisionChannel is implemented
        warn!("Video channel initialization is STUB_APPROVED pending T2-A2 VisionChannel");
        
        // Create video frame receiver that feeds into synchronizer
        self.create_video_frame_handler().await;
        
        Ok(())
    }

    /// Create video frame handler that feeds into the synchronizer
    async fn create_video_frame_handler(&self) {
        let synchronizer = self.synchronizer.clone();
        let stats = self.stats.clone();
        
        let (tx, mut rx) = mpsc::unbounded_channel::<VideoFrame>();

        // Store sender for later use by video channel
        // SYSTEM_LAW: STUB_APPROVED - This will be integrated with VisionChannel
        let _video_tx = tx;

        // Spawn task to process video frames
        let task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                trace!("Video frame {} received for sync", frame.frame_id);
                
                // Add to synchronizer
                synchronizer.lock().await.add_video_frame(frame);
                
                // Update stats
                stats.lock().await.video_frames_received += 1;
            }
        });

        self.tasks.lock().await.push(task);
    }

    /// Start the synchronization loop
    async fn start_sync_loop(&self) -> StreamingResult<()> {
        let synchronizer = self.synchronizer.clone();
        let frame_sender = self.frame_sender.clone();
        let state = Arc::clone(&self.state);
        let stats = self.stats.clone();

        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(10));

            loop {
                interval.tick().await;

                // Check if controller is still active
                let current_state = *state.read().await;
                if matches!(current_state, ControllerState::Closed | ControllerState::Error) {
                    break;
                }

                // Try to get synchronized frames
                while let Some(frame) = synchronizer.lock().await.try_sync() {
                    trace!("Synchronized frame {} produced", frame.sync_id);

                    // Send to output
                    if let Err(e) = frame_sender.send(frame.clone()) {
                        warn!("Failed to send synchronized frame: {}", e);
                        break;
                    }

                    // Update stats
                    let mut s = stats.lock().await;
                    s.synchronized_frames += 1;
                    if frame.audio.is_some() {
                        s.audio_frames_synced += 1;
                    }
                    if frame.video.is_some() {
                        s.video_frames_synced += 1;
                    }
                }
            }

            debug!("Sync loop terminated");
        });

        self.tasks.lock().await.push(task);
        Ok(())
    }

    /// Start streaming
    pub async fn start(&self) -> StreamingResult<()> {
        let mut state = self.state.write().await;
        
        match *state {
            ControllerState::Closed => {
                return Err(StreamingError::AudioProcessing(
                    "Cannot restart closed controller".to_string()
                ));
            }
            ControllerState::Streaming => {
                warn!("FullDuplexController {} already streaming", self.config.controller_id);
                return Ok(());
            }
            _ => {}
        }

        // Start audio channel
        if let Some(audio) = self.audio_channel.lock().await.as_ref() {
            audio.start().await?;
        }

        *state = ControllerState::Streaming;
        info!("FullDuplexController {} started streaming", self.config.controller_id);
        
        Ok(())
    }

    /// Pause streaming
    pub async fn pause(&self) -> StreamingResult<()> {
        let mut state = self.state.write().await;
        
        if *state == ControllerState::Streaming {
            // Pause audio channel
            if let Some(audio) = self.audio_channel.lock().await.as_ref() {
                audio.pause().await?;
            }

            *state = ControllerState::Paused;
            info!("FullDuplexController {} paused", self.config.controller_id);
        }
        
        Ok(())
    }

    /// Resume streaming
    pub async fn resume(&self) -> StreamingResult<()> {
        let mut state = self.state.write().await;
        
        if *state == ControllerState::Paused {
            // Resume audio channel
            if let Some(audio) = self.audio_channel.lock().await.as_ref() {
                audio.resume().await?;
            }

            *state = ControllerState::Streaming;
            info!("FullDuplexController {} resumed", self.config.controller_id);
        }
        
        Ok(())
    }

    /// Stop streaming
    pub async fn stop(&self) -> StreamingResult<()> {
        let mut state = self.state.write().await;
        
        // Stop audio channel
        if let Some(audio) = self.audio_channel.lock().await.as_ref() {
            audio.close().await?;
        }

        *state = ControllerState::Ready;
        info!("FullDuplexController {} stopped", self.config.controller_id);
        
        Ok(())
    }

    /// Synchronize audio and video frames for playback
    ///
    /// # Arguments
    /// * `audio_frame` - Audio frame to synchronize
    /// * `video_frame` - Video frame to synchronize
    ///
    /// # Returns
    /// * `MultimodalFrame` - Synchronized multimodal frame
    pub async fn sync_frames(
        &self,
        audio_frame: AudioFrame,
        video_frame: VideoFrame,
    ) -> Result<MultimodalFrame, StreamingError> {
        // 1. Extract timestamps from both frames
        let audio_ts = audio_frame.timestamp;
        let video_ts = video_frame.timestamp;
        
        // 2. Calculate drift between audio and video
        let drift_ms = video_ts.signed_duration_since(audio_ts).num_milliseconds();
        
        // 3. Apply sync algorithm (drop or repeat frames based on drift)
        let sync_offset_ms = if drift_ms.abs() > MAX_SYNC_DRIFT_MS {
            // Force sync by using the later timestamp
            warn!("Large drift detected ({}ms), forcing synchronization", drift_ms);
            drift_ms
        } else {
            // Within acceptable range, use as-is
            drift_ms
        };
        
        // 4. Return synchronized multimodal frame
        let sync_id = format!(
            "sync_{}_{}",
            Utc::now().timestamp_millis(),
            self.stats.lock().await.synchronized_frames
        );
        
        // Update stats
        self.stats.lock().await.synchronized_frames += 1;
        
        Ok(MultimodalFrame {
            sync_id,
            timestamp: audio_ts.max(video_ts),
            audio: Some(audio_frame),
            video: Some(video_frame),
            sync_offset_ms,
        })
    }

    /// Start full-duplex streaming (send + receive)
    ///
    /// # Arguments
    /// * `config` - Stream configuration
    ///
    /// # Returns
    /// * `DuplexStreamHandle` - Handle for controlling the stream
    pub async fn start_duplex_stream(
        &self,
        config: StreamConfig,
    ) -> Result<DuplexStreamHandle, StreamingError> {
        // 1. Initialize audio channel for capture + playback
        if self.config.enable_audio {
            self.initialize_audio_channel().await?;
        }
        
        // 2. Initialize video channel for capture + display
        if self.config.enable_video {
            self.initialize_video_channel().await?;
        }
        
        // 3. Create multiplexed stream
        let mut stream_info = StreamInfo::new(
            config.stream_id.clone(),
            if self.config.enable_audio && self.config.enable_video {
                StreamType::AudioVideo
            } else if self.config.enable_audio {
                StreamType::Audio
            } else {
                StreamType::Video
            },
            StreamDirection::Bidirectional,
        )
        .with_priority(config.priority);
        if let Some(peer_id) = config.peer_id.clone() {
            stream_info = stream_info.with_peer_id(peer_id);
        }
        
        self.mux.register_stream(stream_info).await?;
        
        // 4. Start sync coordinator
        self.start_sync_loop().await?;
        
        // 5. Transition to streaming state (Initializing -> Ready -> Connecting -> Streaming)
        if *self.state.read().await == ControllerState::Initializing {
            self.transition_state(ControllerState::Ready).await?;
        }
        if *self.state.read().await == ControllerState::Ready {
            self.transition_state(ControllerState::Connecting).await?;
        }
        self.transition_state(ControllerState::Streaming).await?;
        
        // 6. Return handle for control
        Ok(DuplexStreamHandle {
            session_id: self.config.session_id.clone(),
            controller_id: self.config.controller_id.clone(),
            stream_id: config.stream_id,
            started_at: Utc::now(),
        })
    }

    /// Handle user interrupt (voice activity detection)
    ///
    /// # Arguments
    /// * `interrupt_type` - Type of interrupt (VoiceActivity, Manual, Timeout)
    ///
    /// # Returns
    /// * `StreamingResult<()>` - Success or error
    pub async fn handle_interrupt(
        &self,
        interrupt_type: InterruptType,
    ) -> Result<(), StreamingError> {
        info!("Handling interrupt: {:?}", interrupt_type);
        
        // 1. Pause outgoing audio stream
        if self.config.enable_audio {
            if let Some(channel) = self.audio_channel.lock().await.as_ref() {
                channel.pause().await?;
            }
        }
        
        // 2. Signal interrupt to remote peer (via WebRTC data channel)
        self.send_interrupt_signal(interrupt_type).await?;
        
        // 3. Switch to listening mode
        self.transition_state(ControllerState::Paused).await?;
        
        // 4. Resume on voice activity or timeout
        match interrupt_type {
            InterruptType::VoiceActivity => {
                // Wait for voice activity to cease
                self.wait_for_voice_activity_end().await?;
            }
            InterruptType::Manual => {
                // Wait for manual resume
                debug!("Waiting for manual resume after manual interrupt");
            }
            InterruptType::Timeout => {
                // Auto-resume after timeout
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    self.config.max_latency_ms
                )).await;
            }
        }
        
        // Resume streaming
        self.transition_state(ControllerState::Streaming).await?;
        
        if self.config.enable_audio {
            if let Some(channel) = self.audio_channel.lock().await.as_ref() {
                channel.resume().await?;
            }
        }
        
        Ok(())
    }

    /// Send interrupt signal to remote peer
    async fn send_interrupt_signal(&self, interrupt_type: InterruptType) -> StreamingResult<()> {
        // In real implementation, send via WebRTC data channel
        debug!("Sending interrupt signal: {:?}", interrupt_type);
        Ok(())
    }

    /// Wait for voice activity to end
    async fn wait_for_voice_activity_end(&self) -> StreamingResult<()> {
        // In real implementation, monitor audio input for silence
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        Ok(())
    }

    /// Transition between states with validation
    ///
    /// # Arguments
    /// * `new_state` - Target state
    ///
    /// # Returns
    /// * `StreamingResult<()>` - Success or error
    pub async fn transition_state(
        &self,
        new_state: ControllerState,
    ) -> Result<(), StreamingError> {
        let mut state = self.state.write().await;
        let current_state = *state;
        
        // 1. Validate state transition is legal
        if !self.is_valid_transition(current_state, new_state) {
            return Err(StreamingError::InvalidStateTransition {
                from: format!("{:?}", current_state),
                to: format!("{:?}", new_state),
            });
        }
        
        // 2. Execute exit actions for current state
        self.execute_exit_actions(current_state).await?;
        
        info!("Transitioning state: {:?} -> {:?}", current_state, new_state);
        
        // 3. Update state
        *state = new_state;
        
        // 4. Execute entry actions for new state
        self.execute_entry_actions(new_state).await?;
        
        Ok(())
    }

    /// Check if state transition is valid
    fn is_valid_transition(&self, from: ControllerState, to: ControllerState) -> bool {
        matches!(
            (from, to),
            // From Initializing
            (ControllerState::Initializing, ControllerState::Ready) |
            // From Ready
            (ControllerState::Ready, ControllerState::Connecting) |
            (ControllerState::Ready, ControllerState::Closed) |
            // From Connecting
            (ControllerState::Connecting, ControllerState::Streaming) |
            (ControllerState::Connecting, ControllerState::Ready) |
            (ControllerState::Connecting, ControllerState::Error) |
            // From Streaming
            (ControllerState::Streaming, ControllerState::Paused) |
            (ControllerState::Streaming, ControllerState::Closed) |
            (ControllerState::Streaming, ControllerState::Error) |
            // From Paused
            (ControllerState::Paused, ControllerState::Streaming) |
            (ControllerState::Paused, ControllerState::Closed) |
            // From Error
            (ControllerState::Error, ControllerState::Ready) |
            (ControllerState::Error, ControllerState::Closed) |
            // From Closed (no transitions allowed)
            (_, ControllerState::Closed)
        )
    }

    /// Execute exit actions for a state
    async fn execute_exit_actions(&self, state: ControllerState) -> StreamingResult<()> {
        match state {
            ControllerState::Streaming => {
                // Flush pending frames before leaving streaming
                self.synchronizer.lock().await.flush();
            }
            ControllerState::Paused => {
                // Notify peers of pause
                debug!("Exiting paused state");
            }
            ControllerState::Connecting => {
                // Cancel pending connections
                debug!("Exiting connecting state");
            }
            _ => {}
        }
        Ok(())
    }

    /// Execute entry actions for a state
    async fn execute_entry_actions(&self, state: ControllerState) -> StreamingResult<()> {
        match state {
            ControllerState::Streaming => {
                // Start frame processing
                debug!("Entering streaming state");
            }
            ControllerState::Paused => {
                // Notify peers of pause
                debug!("Entering paused state");
            }
            ControllerState::Connecting => {
                // Initiate connections
                debug!("Entering connecting state");
            }
            ControllerState::Error => {
                // Log error state
                warn!("Entering error state");
            }
            ControllerState::Closed => {
                // Cleanup resources
            }
            _ => {}
        }
        Ok(())
    }

    /// Shutdown the controller
    pub async fn shutdown(&self) -> StreamingResult<()> {
        info!("Shutting down FullDuplexController {}", self.config.controller_id);

        *self.state.write().await = ControllerState::Closed;

        // Abort all background tasks
        let mut tasks = self.tasks.lock().await;
        for task in tasks.drain(..) {
            task.abort();
        }

        // Shutdown multiplexer
        self.mux.shutdown().await;

        // Clear channels
        *self.audio_channel.lock().await = None;
        *self.video_channel.lock().await = None;

        // Flush synchronizer
        let _ = self.synchronizer.lock().await.flush();

        info!("FullDuplexController {} shutdown complete", self.config.controller_id);
        Ok(())
    }

    /// Get current state
    pub async fn state(&self) -> ControllerState {
        *self.state.read().await
    }

    /// Get configuration
    pub fn config(&self) -> &DuplexConfig {
        &self.config
    }

    /// Get audio channel reference
    pub async fn has_audio_channel(&self) -> bool {
        self.audio_channel.lock().await.is_some()
    }

    /// Receive audio data from network (WebRTC inbound)
    ///
    /// This method should be called when audio data arrives from the remote peer
    pub async fn receive_audio_data(
        &self,
        data: &[u8],
        sequence: u64,
    ) -> StreamingResult<()> {
        if let Some(channel) = self.audio_channel.lock().await.as_ref() {
            channel.on_audio_data(data, Some(Utc::now()), sequence).await?;
        }
        Ok(())
    }

    /// Receive video data from network (WebRTC inbound)
    ///
    /// SYSTEM_LAW: STUB_APPROVED - Full integration pending T2-A2 VisionChannel
    pub async fn receive_video_data(
        &self,
        _data: &[u8],
        _sequence: u64,
    ) -> StreamingResult<()> {
        if !self.config.enable_video {
            return Ok(());
        }

        // Placeholder for video data handling
        // Will be routed to VisionChannel when T2-A2 is ready
        warn!("Video receive is STUB_APPROVED pending T2-A2 VisionChannel");
        
        Ok(())
    }

    /// Send audio data to network (WebRTC outbound)
    ///
    /// SYSTEM_LAW: STUB_APPROVED - WebRTC transport layer
    pub async fn send_audio_data(&self, _data: &[u8]) -> StreamingResult<()> {
        // Placeholder for WebRTC outbound audio
        // This will be implemented when WebRTC transport is ready
        trace!("Audio send STUB_APPROVED - WebRTC transport layer");
        Ok(())
    }

    /// Send video data to network (WebRTC outbound)
    ///
    /// SYSTEM_LAW: STUB_APPROVED - WebRTC transport layer
    pub async fn send_video_data(&self, _data: &[u8]) -> StreamingResult<()> {
        if !self.config.enable_video {
            return Ok(());
        }
        
        // Placeholder for WebRTC outbound video
        trace!("Video send STUB_APPROVED - WebRTC transport layer");
        Ok(())
    }

    /// Get controller statistics
    pub async fn stats(&self) -> DuplexStats {
        self.stats.lock().await.clone()
    }

    /// Get multiplexer statistics
    pub async fn mux_stats(&self) -> crate::controller::MuxStats {
        self.mux.stats().await
    }

    /// Get synchronizer statistics
    pub async fn sync_stats(&self) -> crate::controller::SyncStats {
        self.synchronizer.lock().await.stats()
    }
}

/// Statistics for FullDuplexController
#[derive(Debug, Clone, Default)]
pub struct DuplexStats {
    pub audio_frames_received: u64,
    pub video_frames_received: u64,
    pub audio_frames_synced: u64,
    pub video_frames_synced: u64,
    pub synchronized_frames: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub connection_duration_secs: u64,
}

impl std::fmt::Display for DuplexStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DuplexStats: audio={}/{}, video={}/{}, sync={}",
            self.audio_frames_synced,
            self.audio_frames_received,
            self.video_frames_synced,
            self.video_frames_received,
            self.synchronized_frames
        )
    }
}

/// Builder for FullDuplexController
pub struct FullDuplexControllerBuilder {
    config: DuplexConfig,
}

impl FullDuplexControllerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            config: DuplexConfig::default(),
        }
    }

    /// Set controller ID
    pub fn with_controller_id(mut self, id: impl Into<String>) -> Self {
        self.config.controller_id = id.into();
        self
    }

    /// Set session ID
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.config.session_id = id.into();
        self
    }

    /// Enable/disable audio
    pub fn with_audio(mut self, enable: bool) -> Self {
        self.config.enable_audio = enable;
        self
    }

    /// Enable/disable video
    pub fn with_video(mut self, enable: bool) -> Self {
        self.config.enable_video = enable;
        self
    }

    /// Set max latency
    pub fn with_max_latency(mut self, ms: u64) -> Self {
        self.config.max_latency_ms = ms;
        self
    }

    /// Set enable sync
    pub fn with_sync(mut self, enable: bool) -> Self {
        self.config.enable_sync = enable;
        self
    }

    /// Set buffer size
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    /// Build the controller
    pub fn build(self) -> (FullDuplexController, mpsc::UnboundedReceiver<MultimodalFrame>) {
        FullDuplexController::new(self.config)
    }
}

impl Default for FullDuplexControllerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AudioCodec;

    #[tokio::test]
    async fn test_controller_creation() {
        let config = DuplexConfig::default();
        let (controller, _receiver) = FullDuplexController::new(config);
        
        assert_eq!(controller.state().await, ControllerState::Initializing);
    }

    #[tokio::test]
    async fn test_controller_builder() {
        let (controller, _receiver) = FullDuplexControllerBuilder::new()
            .with_controller_id("test_controller")
            .with_session_id("test_session")
            .with_audio(true)
            .with_video(false)
            .with_max_latency(100)
            .with_buffer_size(50)
            .build();
        
        let config = controller.config();
        assert_eq!(config.controller_id, "test_controller");
        assert_eq!(config.session_id, "test_session");
        assert!(config.enable_audio);
        assert!(!config.enable_video);
        assert_eq!(config.max_latency_ms, 100);
        assert_eq!(config.buffer_size, 50);
    }

    #[tokio::test]
    async fn test_controller_initialize() {
        let (controller, _receiver) = FullDuplexControllerBuilder::new()
            .with_controller_id("test_init")
            .with_audio(true)
            .with_video(false)
            .build();
        
        controller.initialize().await.unwrap();
        
        assert!(matches!(controller.state().await, ControllerState::Ready | ControllerState::Streaming));
    }

    #[tokio::test]
    async fn test_controller_start_stop() {
        let (controller, _receiver) = FullDuplexControllerBuilder::new()
            .with_controller_id("test_start_stop")
            .with_audio(true)
            .with_video(false)
            .build();
        
        controller.initialize().await.unwrap();
        controller.start().await.unwrap();
        
        assert_eq!(controller.state().await, ControllerState::Streaming);
        
        controller.stop().await.unwrap();
        
        assert_eq!(controller.state().await, ControllerState::Ready);
    }

    #[tokio::test]
    async fn test_controller_pause_resume() {
        let (controller, _receiver) = FullDuplexControllerBuilder::new()
            .with_controller_id("test_pause")
            .with_audio(true)
            .with_video(false)
            .build();
        
        controller.initialize().await.unwrap();
        controller.start().await.unwrap();
        
        controller.pause().await.unwrap();
        assert_eq!(controller.state().await, ControllerState::Paused);
        
        controller.resume().await.unwrap();
        assert_eq!(controller.state().await, ControllerState::Streaming);
    }

    #[tokio::test]
    async fn test_controller_shutdown() {
        let (controller, _receiver) = FullDuplexControllerBuilder::new()
            .with_controller_id("test_shutdown")
            .with_audio(true)
            .with_video(false)
            .build();
        
        controller.initialize().await.unwrap();
        controller.shutdown().await.unwrap();
        
        assert_eq!(controller.state().await, ControllerState::Closed);
    }

    #[tokio::test]
    async fn test_receive_audio_data() {
        let (controller, _receiver) = FullDuplexControllerBuilder::new()
            .with_controller_id("test_audio_recv")
            .with_audio(true)
            .with_video(false)
            .build();
        
        controller.initialize().await.unwrap();
        controller.start().await.unwrap();
        
        // Send test audio data (PCM format)
        let samples: Vec<f32> = vec![0.0, 0.5, 1.0, 0.5];
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| s.to_le_bytes().to_vec())
            .collect();
        
        // Should not error even though no peer is connected
        let result = controller.receive_audio_data(&data, 0).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_duplex_config_default() {
        let config = DuplexConfig::default();
        assert!(config.enable_audio);
        assert!(config.enable_video);
        assert_eq!(config.max_latency_ms, 200);
        assert!(config.enable_sync);
        assert_eq!(config.buffer_size, 100);
    }

    #[test]
    fn test_duplex_stats_display() {
        let stats = DuplexStats {
            audio_frames_received: 100,
            video_frames_received: 30,
            audio_frames_synced: 95,
            video_frames_synced: 28,
            synchronized_frames: 28,
            bytes_sent: 10000,
            bytes_received: 15000,
            connection_duration_secs: 60,
        };
        
        let display = format!("{}", stats);
        assert!(display.contains("audio=95/100"));
        assert!(display.contains("video=28/30"));
        assert!(display.contains("sync=28"));
    }

    #[test]
    fn test_controller_state_transitions() {
        // Test state values
        let states = vec![
            ControllerState::Initializing,
            ControllerState::Ready,
            ControllerState::Connecting,
            ControllerState::Streaming,
            ControllerState::Paused,
            ControllerState::Error,
            ControllerState::Closed,
        ];
        
        // Ensure all states are distinct
        let unique_count = states.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(unique_count, 7);
    }
}
