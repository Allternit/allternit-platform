//! The identity blob cfapi stores on each placeholder (FileIdentity, at most 4 KiB).
//!
//! Item IDs are opaque (SPEC section 4): the blob carries them as given, never a
//! path. The rev is the `base_rev` the next write sends.

use serde::{Deserialize, Serialize};

/// cfapi's limit on FileIdentity and SyncRootIdentity.
pub const MAX_IDENTITY_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Identity {
    /// The sync root folder itself: it lists the computer's top-level roots.
    #[serde(rename = "top")]
    Top { v: u8 },
    /// A file or folder inside a root.
    #[serde(rename = "item")]
    Item {
        v: u8,
        /// Root the item belongs to.
        r: String,
        /// Item ID (`it_...`).
        i: String,
        /// Revision the placeholder holds (`<meta_version>-<hash>` or `-dir`).
        rev: String,
        /// Folder.
        d: bool,
    },
}

impl Identity {
    pub fn top() -> Self {
        Identity::Top { v: 1 }
    }

    pub fn item(root_id: &str, item_id: &str, rev: &str, dir: bool) -> Self {
        Identity::Item { v: 1, r: root_id.into(), i: item_id.into(), rev: rev.into(), d: dir }
    }

    pub fn encode(&self) -> Vec<u8> {
        let b = serde_json::to_vec(self).expect("identity serializes");
        assert!(b.len() <= MAX_IDENTITY_BYTES, "identity over 4 KiB");
        b
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return Some(Identity::top());
        }
        serde_json::from_slice(bytes).ok()
    }

    pub fn item_id(&self) -> Option<&str> {
        match self {
            Identity::Item { i, .. } => Some(i),
            Identity::Top { .. } => None,
        }
    }
}

/// The sync-root identity: which computer the root mirrors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootIdentity {
    pub v: u8,
    pub computer_id: String,
}

impl RootIdentity {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("root identity serializes")
    }
}
