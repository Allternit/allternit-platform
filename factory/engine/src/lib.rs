//! Allternit Factory engine (`allternit-factory-engine`).
//!
//! Modules are grouped by job (SPEC §5): `core`, `gate`, and the four parts
//! `agents`, `orchestration`, `workflows`, `workspace`, plus the `api` and
//! `remote` surfaces. `tickets` is the internal foreign-repo ticket area.
//!
//! The flat re-exports below keep the pre-grouping `crate::x::y` paths
//! compiling (internal compatibility only, not a public naming scheme).

pub mod agents;
pub mod api;
pub mod core;
pub mod gate;
pub mod orchestration;
pub mod remote;
pub mod tickets;
pub mod workflows;
pub mod workspace;

pub use crate::agents::{backend, execenv, peer, registry, spawn, view};
pub use crate::api::{cli, mcp, service};
#[cfg(feature = "dolt")]
pub use crate::core::dolt;
pub use crate::core::{compact, index, ledger, projections, prompt, query, replay};
pub use crate::gate::{constraints, egress, fence, hook, killswitch, policy};
pub use crate::orchestration::{attention, bus, mail, observer, send, steer};
pub use crate::remote::bridge;
pub use crate::tickets::{batch, doctor, graph, rails_id, setup, sync};
pub use crate::workflows::{
    dependencies, drive, kernel, leases, merge_locks, templates, wait_gates, wake,
};
pub use crate::workspace::{
    campaign, context, echoes, judge, lessons, memory, receipts, vault, verification, wih, work,
};

pub use crate::context::{
    generate_pack_id, ContextPackInputs, ContextPackQuery, ContextPackSeal, ContextPackStore,
    ContextPackStoreOptions, ContractFile, DeltaFile, InputManifestEntry, SealContextPackRequest,
    SealContextPackResponse, WIH, PolicyBundleRef,
};
pub use crate::core::types::{
    AllternitEvent, Actor, ActorType, EventProvenance, EventScope, LeaseRecord, LeaseRequest,
    LedgerQuery, ReceiptRecord,
};
pub use crate::gate::gate::{DagMutation, MutationProvenance, PromptOrigin};
pub use crate::gate::{Gate, GateError, GateOptions, GateResult, WihPickup, WihPickupOptions};
pub use crate::index::{Index, IndexOptions};
pub use crate::leases::{Leases, LeasesOptions};
pub use crate::ledger::{Ledger, LedgerOptions};
pub use crate::mail::{
    resolve_thread_id, AckState, AgentRecord, AgentRegistry, Mail, MailImportance, MailIndex,
    MailIndexOptions, MailMessage, MailOptions, MailSearchHit, OverdueMessage, TypedMessage,
    DEFAULT_MAIL_THREAD,
};
pub use crate::spawn::{CaptureFiles, SpawnOptions, Spawner, WatchOutcome};
pub use crate::peer::{
    DeliveryReceipt, Peer, PeerEnvelope, PeerRegistry, PeerStatus, send_envelope,
};
#[cfg(unix)]
pub use crate::peer::PeerSocket;
pub use crate::prompt::{project_prompt, PromptTimeline};
pub use crate::receipts::{ReceiptStore, ReceiptStoreOptions};
pub use crate::steer::{CheckpointResult, ConsultResult, Steer};
pub use crate::vault::{Vault, VaultOptions};
pub use crate::work::{project_dag, WorkOps};
