//! FullDuplexController
//!
//! GAP-39: FullDuplexController implementation
//! WIH: GAP-39, Owner: T2-A3
//!
//! Coordinates bidirectional audio/video streams: multiplexing,
//! demultiplexing and A/V synchronization.

mod duplex;
mod mux;
mod sync;

pub use duplex::*;
pub use mux::*;
pub use sync::*;
