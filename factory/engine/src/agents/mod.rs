//! Factory part `agents`: bots, harnesses, peers, spawn (SPEC §5).
//!
//! `backend` is the pane engine as the engine sees it, `spawn` the one spawn
//! path through it, `registry` the one reconciled session registry, and
//! `view` the merged agent view (registry + live panes + peers).

pub mod backend;
pub mod delivery;
pub mod http;
pub mod execenv;
pub mod home_migrate;
pub mod peer;
pub mod registry;
pub mod snapshot;
pub mod spawn;
pub mod team;
pub mod team_apply;
pub mod team_pack;
pub mod team_plan;
pub mod view;
pub mod whoami;
