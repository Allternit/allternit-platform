//! Factory part `agents`: bots, harnesses, peers, spawn (SPEC §5).
//!
//! `backend` is the pane engine as the engine sees it, `spawn` the one spawn
//! path through it, `registry` the one reconciled session registry, and
//! `view` the merged agent view (registry + live panes + peers).

pub mod backend;
pub mod execenv;
pub mod peer;
pub mod registry;
pub mod spawn;
pub mod view;
