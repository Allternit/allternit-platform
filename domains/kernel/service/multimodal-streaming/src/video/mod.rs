//! Video Pipeline
//!
//! GAP-38: VisionChannel implementation
//! WIH: GAP-38, Owner: T2-A2
//!
//! Video decoding, frame buffering and the VisionChannel that ties them
//! together.

mod buffer;
mod channel;
mod decoder;

pub use buffer::*;
pub use channel::*;
pub use decoder::*;

// `PixelFormat` is defined by both the decoder (decoded frame format) and the
// buffer (frame extraction/conversion target). The decoder one is the
// module-level default; the buffer one is exposed under a distinct alias.
pub use buffer::PixelFormat as ExtractionPixelFormat;
pub use decoder::PixelFormat;
