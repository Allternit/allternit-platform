//! Allternit Node Daemon library surface. The binary in `main.rs` is a thin
//! CLI over these modules; the integration tests drive them over real
//! sockets and PTYs.

pub mod config;
pub mod engine;
pub mod handlers;
pub mod identity;
pub mod launch;
pub mod relay;
pub mod service;
pub mod terminal;
