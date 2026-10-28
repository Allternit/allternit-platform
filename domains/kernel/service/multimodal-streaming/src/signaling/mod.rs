//! WebRTC Signaling
//!
//! GAP-43, GAP-44: Signaling protocol, room management
//! WIH: GAP-43, GAP-44, Owner: T2-A5
//!
//! Signaling protocol messages, WebSocket transport, SDP handling,
//! room management and peer coordination.

mod coordinator;
mod protocol;
mod room;
mod sdp;
mod websocket;

pub use coordinator::*;
pub use protocol::*;
pub use room::*;
pub use sdp::*;
pub use websocket::*;

// `ConnectionState` is defined by both the signaling protocol and the
// WebSocket transport. The protocol one is the module-level default; the
// transport one is exposed under a distinct alias.
pub use protocol::ConnectionState;
pub use websocket::ConnectionState as WebSocketConnectionState;
