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
//! - [`strength`]: two strengths per memory behind the `strengths` stage (3.2).
//! - [`routes`]: `/api/v1/memory/sleep/...` and `/api/v1/memory/disputes...`.
//!
//! The module stays self-contained (D1) so it can become its own crate when
//! the allternit-api crate split reaches memory. Docs:
//! `surfaces/docs/api/memory-sleep.mdx`.

pub mod config;
pub mod routes;
pub mod strength;
pub mod types;
