//! Files drive engine for protocol `files/2` (`contracts/files-drive/SPEC.md`).
//!
//! What a computer needs to serve its files, with no transport: the FastCDC chunker,
//! the content-addressed chunk store with refcounts, the index DB (stable item IDs,
//! revisions, versions, trash, per-root change cursor), upload sessions with have/need
//! and resume, keep-both conflict copies, the filesystem watcher, and sealed chunks
//! for client caches and replicas under the Files key hierarchy.
//!
//! Wire types come from `allternit-files-core` (generated; never edited by hand).
//! Consumers: allternit-api `files_v2` (step 12), sync and replicas (step 15), the
//! Linux FUSE/WebDAV mount and Windows helper (steps 24, 25).

pub mod chunker;
pub mod crypto;
mod db;
pub mod drive;
pub mod error;
pub mod model;
pub mod naming;
pub mod store;
pub mod upload;
pub mod watcher;

pub use allternit_files_core as wire;
pub use drive::{ChangeListener, Clock, Drive, DriveOptions, EditingDevice, PruneReport};
pub use error::{FilesError, Result};
pub use model::{ChangeRecord, ChangesPage, ConflictRecord, ItemRecord, Mutation, RootRecord, RootSpec, TrashRecord};
pub use watcher::DriveWatcher;
