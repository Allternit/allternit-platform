//! Channel provider modules living on the cloud control plane.
//!
//! Each provider's shared-app surface (the cloud endpoints a platform posts
//! to, plus the cloud-side send/connect routes the runtime calls) lives in
//! its own module, registered in `lib.rs`. See
//! `docs/CHANNELS_CONTRACTS.md` for the ownership rules.

pub mod teams_app;
