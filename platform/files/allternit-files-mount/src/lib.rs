//! Mount frontends for the Files drive (`files/2`, `contracts/files-drive/SPEC.md`).
//!
//! One computer's drive, as a folder:
//!
//! - **FUSE** (Linux, `fuse.rs`): a real mount point, through `fusermount3`.
//! - **WebDAV** (`dav.rs`): a loopback server for machines without FUSE, or
//!   for clients that speak WebDAV (`davfs2`, `dav://` in GNOME/KDE, rclone).
//!
//! Both sit on the same tree (`vfs.rs`) over a `Backend`. `HttpBackend`
//! (`http.rs`) talks to the computer's `/api/v1/files/v2` over loopback (this
//! machine) or cloud-api's relay (any other computer), with the same wake
//! rules as every other client: browsing never wakes a computer; content on a
//! sleeping one needs the user's consent, given when mounting.
//!
//! `gizzi files mount` starts the `allternit-files-mount` binary (`main.rs`).

pub mod backend;
pub mod dav;
#[cfg(target_os = "linux")]
pub mod fuse;
pub mod http;
pub mod vfs;

pub use backend::{Backend, MountError};
pub use http::{HttpBackend, HttpConfig, Transport};
pub use vfs::{Entry, Vfs};
