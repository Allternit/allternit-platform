//! Read a recorded node output without a `ReceiptStore` handle.
//!
//! The immutable blob (`.allternit/blobs/<blob_id>`) is the truth; the derived
//! `nodes/<node>.out.md` view is the fallback. Used by read-only consumers
//! (observer, vault memory candidates).

use std::path::Path;

use crate::work::types::NodeOutputRef;

/// Default blob directory, relative to the workspace root (matches the
/// `ReceiptStoreOptions` the CLI and service build).
pub const BLOBS_DIR: &str = ".allternit/blobs";

/// Full recorded output text, or `None` when neither the blob nor the view
/// is readable.
pub fn read_node_output_text(root: &Path, output: &NodeOutputRef) -> Option<String> {
    std::fs::read_to_string(root.join(BLOBS_DIR).join(&output.blob_id))
        .or_else(|_| std::fs::read_to_string(root.join(&output.output_path)))
        .ok()
}

/// `s` cut to at most `max` bytes on a char boundary, and whether it was cut.
pub fn cap_utf8(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}
