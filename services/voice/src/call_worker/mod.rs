//! Phone call worker: answers calls on a bot's own number.
//!
//! `allternit-voice-service worker` registers with LiveKit as agent
//! `allternit-voice` (agent worker protocol, [`dispatch`]), joins each `call-*`
//! room it is dispatched into, speaks the fixed AI disclosure plus the bot's
//! greeting, then runs the turn loop: caller audio → Voice Session core →
//! `turn.ended` → [`brain::CallBrain`] (cloud-api relay → the bot's runtime) →
//! reply text → core speak path → bot audio track.
//!
//! Contract: `HANDOFF-realtime-voice-2026-10-02.md` §4.1 (frozen). Spec:
//! `services/voice/spec/CALL_WORKER.md`.
//!
//! Layout. Everything except [`room`] is plain tokio + channels so it is unit
//! tested without LiveKit:
//! - [`config`]: env configuration.
//! - [`dispatch`]: LiveKit agent worker protocol (register, availability,
//!   assignment, termination, ping, job status).
//! - [`cloud_client`]: cloud-api client, request/response types (§4.1 field
//!   names live only there).
//! - [`events`]: `call.*` events, idempotency keys, ordered retrying queue.
//! - [`controls`]: `allternit.call.control` parsing and call state.
//! - [`disclosure`]: the fixed first line.
//! - [`audio`]: frame conversion and resampling.
//! - [`brain`]: `CallBrain` trait, `RelayBrain`, honest fallback line.
//! - [`session_adapter`]: the one binding to the Voice Session core.
//! - [`call`]: the per-call state machine.
//! - [`voicemail`]: outbound answering-machine detection (text, VAD, beep).
//! - [`hold_music`]: the synthesized hold loop.
//! - [`invite_code`]: the outbound call that reads a phone-invite code aloud.
//! - [`transfer`]: warm transfer state machine and briefing wording.
//! - [`recording`]: Egress recording decision (the disclosure follows it).
//! - [`room`] (`call-worker` feature): LiveKit media + server API binding.

pub mod audio;
pub mod brain;
pub mod call;
pub mod cloud_client;
pub mod config;
pub mod controls;
pub mod disclosure;
pub mod dispatch;
pub mod events;
pub mod hold_music;
pub mod invite_code;
pub mod recording;
pub mod revise;
pub mod session_adapter;
pub mod transfer;
pub mod voicemail;

#[cfg(feature = "call-worker")]
pub mod consult;
#[cfg(feature = "call-worker")]
pub mod room;

/// LiveKit agent name the SIP dispatch rule targets (§4.1).
pub const AGENT_NAME: &str = "allternit-voice";

/// LiveKit data channel topic for call controls (§4.1).
pub const CONTROL_TOPIC: &str = "allternit.call.control";

/// Entry point for `allternit-voice-service worker`.
#[cfg(feature = "call-worker")]
pub async fn run_worker() -> anyhow::Result<()> {
    let cfg = config::WorkerConfig::from_env()?;
    room::run(cfg).await
}

/// Entry point for `allternit-voice-service worker` in builds without LiveKit.
#[cfg(not(feature = "call-worker"))]
pub async fn run_worker() -> anyhow::Result<()> {
    anyhow::bail!(
        "this allternit-voice-service build has no call worker; rebuild with \
         `cargo build --release -p voice-service --features call-worker`"
    )
}
