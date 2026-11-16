//! Engine records. They carry no per-principal data: the runtime adds `Permissions`
//! per request when it turns them into wire types.

use std::path::PathBuf;

use allternit_files_core::{
    ChangeEntry, ChangeOp, ConflictInfo, Item, ItemKind, Permissions, PrincipalKind, PrincipalRef,
    RootKind, RootPolicy, TrashEntry,
};

/// blake3 of the empty string: the content hash of an empty file and of a symlink
/// that is listed without content.
pub const EMPTY_HASH: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

#[derive(Debug, Clone, PartialEq)]
pub struct RootRecord {
    pub root_id: String,
    pub kind: RootKind,
    pub name: String,
    /// Canonical absolute path. Owner-only (`path_hint`); never sent to guests.
    pub local_path: PathBuf,
    pub policy: RootPolicy,
    /// The folder item for the root itself (no parent).
    pub root_item_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemRecord {
    pub id: String,
    pub root_id: String,
    /// None only for a root's own folder item.
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: ItemKind,
    pub size: u64,
    pub meta_version: u64,
    pub content_hash: Option<String>,
    pub created_at: String,
    pub modified_at: String,
    pub modified_by: Option<PrincipalRef>,
    /// This folder entry opens another root (Inbox inside ~/Allternit, etc.).
    pub root_link: Option<String>,
    /// A symlink that resolves outside its root: listed, never read (`scope_violation`).
    pub link_escapes: bool,
}

impl ItemRecord {
    /// `"<meta_version>-<content hash>"`, or `"<meta_version>-dir"` for folders.
    pub fn rev(&self) -> String {
        match self.kind {
            ItemKind::Folder => format!("{}-dir", self.meta_version),
            ItemKind::File => format!("{}-{}", self.meta_version, self.content_hash.as_deref().unwrap_or(EMPTY_HASH)),
        }
    }

    pub fn to_item(&self, permissions: Permissions) -> Item {
        Item {
            id: self.id.clone(),
            root_id: self.root_id.clone(),
            parent_id: self.parent_id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            size: self.size,
            rev: self.rev(),
            meta_version: self.meta_version,
            content_hash: self.content_hash.clone(),
            mime: None,
            created_at: self.created_at.clone(),
            modified_at: self.modified_at.clone(),
            modified_by: self.modified_by.clone(),
            root_link: self.root_link.clone(),
            pinned_on: None,
            tags: None,
            provenance: None,
            has_thumbnail: None,
            permissions,
        }
    }
}

/// A write that landed as a keep-both conflict copy.
#[derive(Debug, Clone, PartialEq)]
pub struct ConflictRecord {
    pub original_item_id: String,
    pub base_rev: String,
    pub current_rev: String,
    pub conflict_item: ItemRecord,
    pub conflict_name: String,
}

impl ConflictRecord {
    pub fn to_info(&self, permissions: Permissions) -> ConflictInfo {
        ConflictInfo {
            kind: "keep_both".to_string(),
            original_item_id: self.original_item_id.clone(),
            base_rev: self.base_rev.clone(),
            current_rev: self.current_rev.clone(),
            conflict_item: self.conflict_item.to_item(permissions),
            conflict_name: self.conflict_name.clone(),
        }
    }
}

/// Result of every write: the item, an optional conflict, and the root's cursor after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mutation {
    pub item: ItemRecord,
    pub conflict: Option<ConflictRecord>,
    pub cursor: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrashRecord {
    pub trash_id: String,
    pub item: ItemRecord,
    pub original_parent_id: String,
    pub trashed_at: String,
    pub trashed_by: PrincipalRef,
    pub purge_at: String,
}

impl TrashRecord {
    pub fn to_entry(&self, permissions: Permissions) -> TrashEntry {
        TrashEntry {
            trash_id: self.trash_id.clone(),
            item: self.item.to_item(permissions),
            original_parent_id: self.original_parent_id.clone(),
            trashed_at: self.trashed_at.clone(),
            trashed_by: self.trashed_by.clone(),
            purge_at: self.purge_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeRecord {
    pub root_id: String,
    pub seq: u64,
    pub op: ChangeOp,
    pub item_id: String,
    /// The item as it is now; None when deleted, trashed or moved out of this root.
    pub item: Option<ItemRecord>,
    pub old_parent_id: Option<String>,
    pub at: String,
    pub by: PrincipalRef,
}

impl ChangeRecord {
    pub fn to_entry(&self, permissions: impl Fn(&ItemRecord) -> Permissions) -> ChangeEntry {
        ChangeEntry {
            seq: self.seq,
            op: self.op,
            item_id: self.item_id.clone(),
            item: self.item.as_ref().map(|i| i.to_item(permissions(i))),
            old_parent_id: self.old_parent_id.clone(),
            at: self.at.clone(),
            by: self.by.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangesPage {
    pub root_id: String,
    pub changes: Vec<ChangeRecord>,
    pub cursor: String,
    pub has_more: bool,
    /// The cursor is from an older index generation or was pruned: list again.
    pub reset_required: bool,
}

/// The principal recorded for changes the scanner or watcher finds on disk.
pub fn local_principal() -> PrincipalRef {
    PrincipalRef { kind: PrincipalKind::Owner, id: "local".to_string(), display_name: Some("This computer".to_string()) }
}

/// A root to create on first use (SPEC section 3).
#[derive(Debug, Clone)]
pub struct RootSpec {
    pub kind: RootKind,
    pub name: String,
    pub local_path: PathBuf,
    pub policy: RootPolicy,
}
