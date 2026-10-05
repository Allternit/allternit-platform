//! Hash-based, collision-resistant identifiers for Rails tickets and echoes.
//!
//! Rails IDs are opaque, content-addressed hashes prefixed with a type tag.
//! They carry no timestamp or sequence information, so they do not leak
//! creation order or workload cadence.

use std::fmt;
use std::str::FromStr;

use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Prefix for canonical ticket identifiers.
pub const TICKET_PREFIX: &str = "T";

/// Byte length of the raw hash before encoding.
pub const HASH_BYTES: usize = 16;

/// Character length of the encoded hash payload.
pub const HASH_CHARS: usize = HASH_BYTES * 2;

/// A canonical Rails ticket identifier, e.g. `T-a1b2c3d4...`.
///
/// IDs are generated from a Blake3 hash of the minting payload plus a
/// random nonce. The hash is truncated to 16 bytes (32 hex chars) for
/// readability while retaining 128 bits of entropy, which is sufficient
/// for collision resistance across a project workspace.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TicketId(String);

impl TicketId {
    /// Create an ID from a pre-computed canonical string.
    ///
    /// Prefer [`TicketId::mint`] for new IDs.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Mint a new ticket ID from a seed payload.
    ///
    /// The payload should include enough entropy and context to make the
    /// resulting ID unique and deterministic for the same logical input.
    /// A random nonce is always mixed in so that minting twice with the
    /// same payload still yields distinct IDs.
    pub fn mint(payload: impl AsRef<[u8]>) -> Self {
        let mut nonce = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut nonce);

        let hash = blake3::Hasher::new()
            .update(payload.as_ref())
            .update(&nonce)
            .finalize();

        let bytes = &hash.as_bytes()[..HASH_BYTES];
        Self(format!("{}-{}", TICKET_PREFIX, hex::encode(bytes)))
    }

    /// Return the canonical string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the prefix (`T`).
    pub fn prefix(&self) -> &str {
        TICKET_PREFIX
    }

    /// Validate that `s` is a well-formed ticket ID.
    pub fn is_valid(s: &str) -> bool {
        if let Some(stripped) = s.strip_prefix("T-") {
            stripped.len() == HASH_CHARS && stripped.chars().all(|c| c.is_ascii_hexdigit())
        } else {
            false
        }
    }
}

impl fmt::Display for TicketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TicketId {
    type Err = InvalidTicketId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if TicketId::is_valid(s) {
            Ok(Self(s.to_string()))
        } else {
            Err(InvalidTicketId(s.to_string()))
        }
    }
}

impl AsRef<str> for TicketId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid ticket id: {0}")]
pub struct InvalidTicketId(pub String);

/// A hierarchical ticket identifier that may include dotted children,
/// e.g. `T-a1b2c3d4.1.2`.
///
/// The root segment is always a canonical [`TicketId`]. Each subsequent
/// segment is a decimal child index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HierarchicalId {
    pub root: TicketId,
    pub path: Vec<u32>,
}

impl HierarchicalId {
    /// Create a root hierarchical ID from a ticket ID.
    pub fn root(id: TicketId) -> Self {
        Self {
            root: id,
            path: Vec::new(),
        }
    }

    /// Return the child ID at `index`.
    pub fn child(&self, index: u32) -> Self {
        let mut path = self.path.clone();
        path.push(index);
        Self {
            root: self.root.clone(),
            path,
        }
    }

    /// Return the parent ID, if any.
    pub fn parent(&self) -> Option<Self> {
        if self.path.is_empty() {
            return None;
        }
        let mut path = self.path.clone();
        path.pop();
        Some(Self {
            root: self.root.clone(),
            path,
        })
    }

    /// Return the depth. Root is depth 0.
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// Return the last path segment, if any.
    pub fn index(&self) -> Option<u32> {
        self.path.last().copied()
    }
}

impl fmt::Display for HierarchicalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.root)?;
        for segment in &self.path {
            write!(f, ".{}", segment)?;
        }
        Ok(())
    }
}

impl FromStr for HierarchicalId {
    type Err = InvalidHierarchicalId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.');
        let root_str = parts.next().ok_or_else(|| InvalidHierarchicalId(s.to_string()))?;
        let root = TicketId::from_str(root_str)
            .map_err(|_| InvalidHierarchicalId(s.to_string()))?;

        let mut path = Vec::new();
        for part in parts {
            let index = part
                .parse::<u32>()
                .map_err(|_| InvalidHierarchicalId(s.to_string()))?;
            path.push(index);
        }

        Ok(Self { root, path })
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid hierarchical id: {0}")]
pub struct InvalidHierarchicalId(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_valid() {
        let id = TicketId::mint("hello world");
        assert!(TicketId::is_valid(id.as_str()));
        assert!(id.as_str().starts_with("T-"));
        assert_eq!(id.as_str().len(), 2 + HASH_CHARS);
    }

    #[test]
    fn mint_twice_yields_distinct_ids() {
        let a = TicketId::mint("same payload");
        let b = TicketId::mint("same payload");
        assert_ne!(a, b);
    }

    #[test]
    fn parse_round_trip() {
        let id = TicketId::mint("test");
        let parsed = TicketId::from_str(id.as_str()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn invalid_ids_rejected() {
        assert!(!TicketId::is_valid(""));
        assert!(!TicketId::is_valid("T-"));
        assert!(!TicketId::is_valid("X-a1b2c3d4"));
        assert!(!TicketId::is_valid("T-zzzzzzzz"));
    }

    #[test]
    fn hierarchical_round_trip() {
        let root = TicketId::mint("root");
        let id = HierarchicalId::root(root).child(1).child(2);
        let s = id.to_string();
        let parsed = HierarchicalId::from_str(&s).unwrap();
        assert_eq!(id, parsed);
        assert_eq!(id.depth(), 2);
        assert_eq!(id.index(), Some(2));
        assert_eq!(id.parent().unwrap().index(), Some(1));
    }
}
