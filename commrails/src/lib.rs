pub mod bridge;
pub mod bus;
pub mod batch;
pub mod cli;
pub mod compact;
pub mod context;
pub mod core;
pub mod dependencies;
pub mod doctor;
#[cfg(feature = "dolt")]
pub mod dolt;
pub mod echoes;
pub mod gate;
pub mod graph;
pub mod index;
pub mod killswitch;
pub mod leases;
pub mod ledger;
pub mod mail;
pub mod mcp;
pub mod memory;
pub mod merge_locks;
pub mod orchestrator;
pub mod peer;
pub mod policy;
pub mod projections;
pub mod templates;
pub mod prompt;
pub mod query;
pub mod rails_id;
pub mod receipts;
pub mod service;
pub mod setup;
pub mod steer;
pub mod sync;
pub mod tickets;
pub mod vault;
pub mod verification;
pub mod wait_gates;
pub mod wih;
pub mod work;
pub mod workspace;

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
pub use crate::orchestrator::{ExecutorProbe, Orchestrator, SpawnOptions, WatchOutcome};
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
