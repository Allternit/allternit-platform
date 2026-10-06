//! ao (Allternit orchestrator) additive module.
//!
//! This module is the single intentional engine diff beyond the P0 gut list
//! (plan §7 additive `src/ao/` policy). It implements the PTY transcript tee
//! that gives `ao` byte-0-complete session logs (`script -q` parity). See
//! `docs/learnings/ALLTERNIT_RUNTIME_P1_NOTES.md` in the workspace and
//! `cli/ao.rs` for the client side of the contract.

pub(crate) mod transcript;

pub(crate) mod fabric;

pub(crate) mod harness;

pub(crate) mod mailbox;

pub(crate) mod native;

pub(crate) mod peers;

pub(crate) mod serve;

pub(crate) mod visibility;
