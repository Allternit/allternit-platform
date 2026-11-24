//! Meeting worker: live transcription and spoken replies for Allternit Meetings.
//!
//! `allternit-voice-service meeting-worker` registers with LiveKit as agent
//! [`AGENT_NAME`] and is dispatched into each `meeting-<sid>` room the meeting
//! launcher creates. There it subscribes to the meeting pod's mixed call-audio
//! track and runs continuous Silero VAD + sherpa STT (no turn-taking or
//! barge-in: a meeting is transcribed end to end) and sends every segment to
//! the pod on a data topic. The pod labels the speaker from its
//! active-speaker timeline, assigns `seq` and ingests (CONTRACT.md "pod ↔
//! worker names"). When someone says "Gizzi, …" the worker sends the pod a
//! turn; the pod labels the asker and runs the in-pod agent. When the pod
//! wants a reply spoken it sends `allternit.meeting.speak` and the worker
//! synthesizes it with Kokoro onto the `tts` track the pod plays.
//!
//! The worker talks only to the pod, in the room. It calls no cloud route
//! and holds no token besides its LiveKit job token.
//!
//! Spec: `services/voice/spec/MEETING_WORKER.md`. Contract:
//! `docs/meetings/CONTRACT.md` (pod auth, ingest batch, control stream).
//!
//! Layout. Everything except [`room`] is plain Rust and unit tested without LiveKit:
//! - [`config`]: env configuration and dispatch metadata.
//! - [`wake`]: wake-word detection, sound-alike spellings included.
//! - [`pipeline`]: STT stream → segments and turns.
//! - [`reply`]: Kokoro → `tts` track frames.
//! - [`fixture`]: the offline run on recorded call audio.
//! - [`room`] (`call-worker` feature): LiveKit binding.

pub mod config;
pub mod fixture;
pub mod pipeline;
pub mod reply;
pub mod wake;

#[cfg(feature = "call-worker")]
pub mod room;

/// LiveKit agent name the meeting launcher dispatches (`call_worker` is
/// `allternit-voice`; the two never take each other's rooms).
pub const AGENT_NAME: &str = "allternit-meeting";

/// Meeting rooms are `meeting-<session id>` (CONTRACT.md, adapter.ts `PodConfig.livekit.room`).
pub const ROOM_PREFIX: &str = "meeting-";

/// The pod's published track carrying the call's mixed remote audio
/// (CONTRACT.md "pod ↔ worker names"; meeting-bot livekit.ts `CALL_AUDIO_TRACK`).
pub const CALL_AUDIO_TRACK: &str = "call-audio";

/// LiveKit data topic for STT segments, worker → pod (livekit.ts `STT_TOPIC`).
pub const STT_TOPIC: &str = "allternit.meeting.stt";

/// LiveKit data topic for wake-word [`pipeline::TurnEvent`]s, worker → pod.
pub const TURN_TOPIC: &str = "allternit.meeting.turn";

/// LiveKit data topic for spoken replies, pod → worker: `{turn_id, text}`.
pub const SPEAK_TOPIC: &str = "allternit.meeting.speak";

/// Entry point for `allternit-voice-service meeting-worker`.
#[cfg(feature = "call-worker")]
pub async fn run_worker() -> anyhow::Result<()> {
    let cfg = config::MeetingWorkerConfig::from_env()?;
    room::run(cfg).await
}

/// Entry point in builds without LiveKit.
#[cfg(not(feature = "call-worker"))]
pub async fn run_worker() -> anyhow::Result<()> {
    anyhow::bail!(
        "this allternit-voice-service build has no meeting worker; rebuild with \
         `cargo build --release -p voice-service --features call-worker`"
    )
}
