//! What the helper does after a write, and which badges an item gets.
//!
//! The runtime names conflict copies (SPEC section 4); the helper never invents a
//! name. It only mirrors what the server did: the local bytes now belong to the
//! conflict copy, and the original name goes back to the server's current file.

use allternit_files_core::{Item, RootKind, UploadCommitResponse};

#[derive(Debug, Clone, PartialEq)]
pub enum AfterWrite {
    /// The write landed on the item; the placeholder takes the new rev.
    Landed { item: Item },
    /// The write became a conflict copy: rename the local file to `conflict_name`,
    /// bind it to `conflict_item`, and put the server's `original_item_id` back
    /// under the old name.
    Conflict { conflict_name: String, conflict_item: Item, original_item_id: String },
}

pub fn after_write(resp: &UploadCommitResponse) -> AfterWrite {
    match &resp.conflict {
        Some(c) => AfterWrite::Conflict {
            conflict_name: c.conflict_name.clone(),
            conflict_item: c.conflict_item.clone(),
            original_item_id: c.original_item_id.clone(),
        },
        None => AfterWrite::Landed { item: resp.item.clone() },
    }
}

/// True for names the runtime gives conflict copies:
/// `<stem> (conflict <device> <YYYY-MM-DD HHmm>[ n]).<ext>`.
pub fn is_conflict_name(name: &str) -> bool {
    let Some(start) = name.find(" (conflict ") else { return false };
    let rest = &name[start + " (conflict ".len()..];
    let Some(close) = rest.find(')') else { return false };
    let inner = &rest[..close];
    // Strip an optional " 2", " 3"... added when the name was taken.
    let inner = match inner.rsplit_once(' ') {
        Some((head, n)) if !n.is_empty() && n.len() < 4 && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => inner,
    };
    let b = inner.as_bytes();
    if b.len() < 16 {
        return false;
    }
    let tail = &inner[inner.len() - 15..]; // "YYYY-MM-DD HHmm"
    let t = tail.as_bytes();
    let digits = |r: std::ops::Range<usize>| t[r].iter().all(|c| c.is_ascii_digit());
    inner.as_bytes()[inner.len() - 16] == b' '
        && digits(0..4)
        && t[4] == b'-'
        && digits(5..7)
        && t[7] == b'-'
        && digits(8..10)
        && t[10] == b' '
        && digits(11..15)
}

/// Badges from design pick 6A (colour badges, same set as Finder). "On this
/// computer" and "In cloud" are Explorer's own cfapi status column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    Agent,
    Shared,
    Conflict,
}

impl Badge {
    /// Property ID registered on the sync root.
    pub fn id(self) -> i32 {
        match self {
            Badge::Agent => 2,
            Badge::Shared => 3,
            Badge::Conflict => 4,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Badge::Agent => "Agent output",
            Badge::Shared => "Shared",
            Badge::Conflict => "Conflict, open to resolve",
        }
    }
    pub fn icon_file(self) -> &'static str {
        match self {
            Badge::Agent => "badge-agent.ico",
            Badge::Shared => "badge-shared.ico",
            Badge::Conflict => "badge-conflict.ico",
        }
    }
    pub const ALL: [Badge; 3] = [Badge::Agent, Badge::Shared, Badge::Conflict];
}

pub fn badges_for(item: &Item, root_kind: Option<&RootKind>) -> Vec<Badge> {
    let mut out = Vec::new();
    if item.provenance.as_ref().is_some_and(|p| p.ai_generated) || matches!(root_kind, Some(RootKind::AgentOutput)) {
        out.push(Badge::Agent);
    }
    if matches!(root_kind, Some(RootKind::Shared)) {
        out.push(Badge::Shared);
    }
    if is_conflict_name(&item.name) {
        out.push(Badge::Conflict);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The conflict badge keys off the runtime's naming rule, including the " 2"
    // suffix and device names with spaces; ordinary parentheses must not match.
    #[test]
    fn recognises_runtime_conflict_names_only() {
        assert!(is_conflict_name("Lease (conflict iPhone 2026-11-15 1042).pdf"));
        assert!(is_conflict_name("Lease (conflict Joe's PC 2026-11-15 1042 2).pdf"));
        assert!(is_conflict_name("notes (conflict Mac 2026-01-02 0000)"));
        assert!(!is_conflict_name("Lease (conflict).pdf"));
        assert!(!is_conflict_name("Lease (2).pdf"));
        assert!(!is_conflict_name("Lease (conflict iPhone 2026-11-15).pdf"));
    }
}
