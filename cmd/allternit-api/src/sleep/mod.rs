//! Allternit Sleep: event-driven memory consolidation on the user's own runtime.
//!
//! This step (2.1) is the contract only: migration V291, the wire types, the
//! settings and thresholds with their defaults, and the routes with real empty
//! states. Every stage flag is off by default (D15) and nothing here runs a
//! cycle, spends quota or emits events yet.
//!
//! - [`types`]: route payloads, `SourceType`, `Stage`, `DisputeChoice`,
//!   `SignalEvent`, and the `sleep.*` event names.
//! - [`config`]: code defaults overlaid with `sleep_config` rows.
//! - [`fit`]: fits decay parameters from labelled recalls (3.4).
//! - [`strength`]: two strengths per memory behind the `strengths` stage (3.2).
//! - [`trigger`]: runtime signals to queued `sleep_units` (5.2).
//! - [`queue`]: unit leases, attempts, deferral and held continuations (5.1).
//! - [`cycle`]: the orchestrator: fixed stage order, spend cap, clean stop,
//!   sleep debt, and the background worker (5.1).
//! - [`report`]: the per-cycle report, real spend and undo (5.5).
//! - [`disputes`]: contradictions kept as dispute records, the recall
//!   monitor's notes and the user's resolve (6.1).
//! - [`arbiter`]: the deterministic pick by scope, credence, recency, goal
//!   and stakes, or `needs_user` (6.1).
//! - [`checks`]: spaced checks, the bounded check set and the tether ladder (6.3).
//! - [`intentions`]: prospective memory: reminders captured from user turns,
//!   fired by cue or due time, delivered once through context (6.4).
//! - [`hypotheses`]: the queue Dream lessons and REM links wait in, kept only
//!   when their metric improves (5.4).
//! - [`rem`]: REM insight links, their metric, self-off and the use hook (5.4).
//! - [`routes`]: `/api/v1/memory/sleep/...` and `/api/v1/memory/disputes...`.
//!
//! The module stays self-contained (D1) so it can become its own crate when
//! the allternit-api crate split reaches memory. Docs:
//! `surfaces/docs/api/memory-sleep.mdx`.

pub mod arbiter;
pub mod checks;
pub mod config;
pub mod cycle;
pub mod disputes;
pub mod fit;
pub mod hypotheses;
pub mod intentions;
pub mod queue;
pub mod rem;
pub mod report;
pub mod routes;
pub mod strength;
pub mod trigger;
pub mod score;
pub mod types;
