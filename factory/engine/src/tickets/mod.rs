//! Ticket model and event-sourced store for the Rails CLI.
//!
//! A ticket is the primary unit of tracked work. Tickets are stored as an
//! append-only event log in the workspace and projected into current state
//! on read.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};
use crate::dependencies::{DependencyGraph, DependencyKind};
use crate::rails_id::{HierarchicalId, TicketId};
use crate::wait_gates::WaitGateStore;

/// Default directory for ticket events, relative to workspace root.
pub const TICKET_EVENTS_DIR: &str = ".allternit/rails/ticket_events";

/// Default directory for ticket snapshots, relative to workspace root.
pub const TICKET_SNAPSHOTS_DIR: &str = ".allternit/rails/ticket_snapshots";

/// Ledger head tracking the hash of the most recent event.
pub const TICKET_LEDGER_HEAD: &str = ".allternit/rails/ticket_events/HEAD.json";

/// Lifecycle status of a ticket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    #[default]
    Open,
    InProgress,
    Blocked,
    Deferred,
    Closed,
}

impl fmt::Display for TicketStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TicketStatus::Open => write!(f, "open"),
            TicketStatus::InProgress => write!(f, "in_progress"),
            TicketStatus::Blocked => write!(f, "blocked"),
            TicketStatus::Deferred => write!(f, "deferred"),
            TicketStatus::Closed => write!(f, "closed"),
        }
    }
}

impl FromStr for TicketStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(TicketStatus::Open),
            "in_progress" => Ok(TicketStatus::InProgress),
            "blocked" => Ok(TicketStatus::Blocked),
            "deferred" => Ok(TicketStatus::Deferred),
            "closed" => Ok(TicketStatus::Closed),
            _ => bail!("unknown ticket status: {s}"),
        }
    }
}

use std::fmt;
use std::str::FromStr;

/// Kind of ticket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    #[default]
    Task,
    Bug,
    Feature,
    Epic,
    Chore,
    Decision,
}

impl fmt::Display for TicketKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TicketKind::Task => write!(f, "task"),
            TicketKind::Bug => write!(f, "bug"),
            TicketKind::Feature => write!(f, "feature"),
            TicketKind::Epic => write!(f, "epic"),
            TicketKind::Chore => write!(f, "chore"),
            TicketKind::Decision => write!(f, "decision"),
        }
    }
}

impl FromStr for TicketKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "task" => Ok(TicketKind::Task),
            "bug" => Ok(TicketKind::Bug),
            "feature" => Ok(TicketKind::Feature),
            "epic" => Ok(TicketKind::Epic),
            "chore" => Ok(TicketKind::Chore),
            "decision" => Ok(TicketKind::Decision),
            _ => bail!("unknown ticket kind: {s}"),
        }
    }
}

/// Priority level, where P0 is highest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketPriority {
    #[default]
    P2,
    P0,
    P1,
    P3,
    P4,
}

impl TicketPriority {
    /// Return the numeric priority level, where lower is more urgent.
    pub fn level(&self) -> u8 {
        match self {
            TicketPriority::P0 => 0,
            TicketPriority::P1 => 1,
            TicketPriority::P2 => 2,
            TicketPriority::P3 => 3,
            TicketPriority::P4 => 4,
        }
    }
}

impl fmt::Display for TicketPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TicketPriority::P0 => write!(f, "P0"),
            TicketPriority::P1 => write!(f, "P1"),
            TicketPriority::P2 => write!(f, "P2"),
            TicketPriority::P3 => write!(f, "P3"),
            TicketPriority::P4 => write!(f, "P4"),
        }
    }
}

impl FromStr for TicketPriority {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "P0" | "0" => Ok(TicketPriority::P0),
            "P1" | "1" => Ok(TicketPriority::P1),
            "P2" | "2" => Ok(TicketPriority::P2),
            "P3" | "3" => Ok(TicketPriority::P3),
            "P4" | "4" => Ok(TicketPriority::P4),
            _ => bail!("unknown ticket priority: {s}"),
        }
    }
}

/// A comment or note attached to a ticket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TicketNote {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// A ticket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ticket {
    pub id: TicketId,
    pub hierarchical_id: HierarchicalId,
    pub title: String,
    pub description: String,
    pub design: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Vec<TicketNote>,
    pub status: TicketStatus,
    pub kind: TicketKind,
    pub priority: TicketPriority,
    pub assignee: Option<String>,
    pub estimate_minutes: Option<u32>,
    pub due_at: Option<DateTime<Utc>>,
    pub defer_until: Option<DateTime<Utc>>,
    pub labels: Vec<String>,
    pub external_ref: Option<String>,
    pub metadata: HashMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub close_reason: Option<String>,
}

impl Ticket {
    /// Return true if the ticket counts as open for ready-list purposes.
    pub fn is_open(&self) -> bool {
        !matches!(self.status, TicketStatus::Closed)
    }

    /// Return true if the ticket is deferred past now.
    pub fn is_deferred(&self, now: DateTime<Utc>) -> bool {
        matches!(self.status, TicketStatus::Deferred)
            || self
                .defer_until
                .map(|d| d > now)
                .unwrap_or(false)
    }
}

/// Events that mutate ticket state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum TicketEvent {
    Created {
        ticket: Ticket,
    },
    Updated {
        id: TicketId,
        title: Option<String>,
        description: Option<String>,
        design: Option<Option<String>>,
        acceptance: Option<Option<String>>,
        priority: Option<TicketPriority>,
        assignee: Option<Option<String>>,
        estimate_minutes: Option<Option<u32>>,
        due_at: Option<Option<DateTime<Utc>>>,
        defer_until: Option<Option<DateTime<Utc>>>,
        labels: Option<Vec<String>>,
        external_ref: Option<Option<String>>,
        metadata: Option<HashMap<String, Value>>,
        updated_at: DateTime<Utc>,
    },
    StatusChanged {
        id: TicketId,
        status: TicketStatus,
        actor: String,
        reason: Option<String>,
        ts: DateTime<Utc>,
    },
    NoteAdded {
        id: TicketId,
        note: TicketNote,
    },
    LabelAdded {
        id: TicketId,
        label: String,
        ts: DateTime<Utc>,
    },
    LabelRemoved {
        id: TicketId,
        label: String,
        ts: DateTime<Utc>,
    },
    /// A dependency edge was added. Spans two tickets; filed under `from`.
    /// Graph rebuilds must scan the full event log, not `events_for(id)`.
    DependencyAdded {
        from: TicketId,
        to: TicketId,
        kind: DependencyKind,
        ts: DateTime<Utc>,
    },
    /// A dependency edge was removed. Spans two tickets; filed under `from`.
    DependencyRemoved {
        from: TicketId,
        to: TicketId,
        kind: DependencyKind,
        ts: DateTime<Utc>,
    },
}

impl TicketEvent {
    pub fn ticket_id(&self) -> &TicketId {
        match self {
            TicketEvent::Created { ticket } => &ticket.id,
            TicketEvent::Updated { id, .. }
            | TicketEvent::StatusChanged { id, .. }
            | TicketEvent::NoteAdded { id, .. }
            | TicketEvent::LabelAdded { id, .. }
            | TicketEvent::LabelRemoved { id, .. } => id,
            TicketEvent::DependencyAdded { from, .. }
            | TicketEvent::DependencyRemoved { from, .. } => from,
        }
    }
}

/// Tamper-evident envelope around a ticket event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TicketEventEnvelope {
    /// Hash of the previous envelope in the chain, if any.
    pub previous_hash: Option<String>,
    /// Hash of this envelope's event and previous_hash.
    pub event_hash: String,
    /// Sequence number in the global ticket ledger.
    pub sequence: u64,
    /// The wrapped ticket event.
    pub event: TicketEvent,
}

/// Ledger head tracking the latest envelope hash and sequence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LedgerHead {
    pub latest_hash: String,
    pub sequence: u64,
}

/// Event-sourced ticket store.
pub struct TicketStore {
    events_dir: PathBuf,
    snapshots_dir: PathBuf,
    head_path: PathBuf,
}

impl TicketStore {
    /// Open the store rooted at `root`.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let events_dir = root.join(TICKET_EVENTS_DIR);
        let snapshots_dir = root.join(TICKET_SNAPSHOTS_DIR);
        let head_path = root.join(TICKET_LEDGER_HEAD);
        ensure_dir(&events_dir)?;
        ensure_dir(&snapshots_dir)?;
        Ok(Self {
            events_dir,
            snapshots_dir,
            head_path,
        })
    }

    /// Append an event to the store.
    pub fn append(&self, event: &TicketEvent) -> Result<TicketEventEnvelope> {
        let head = self.read_head()?;
        let sequence = head.as_ref().map(|h| h.sequence + 1).unwrap_or(0);
        let previous_hash = head.map(|h| h.latest_hash);

        let envelope = TicketEventEnvelope {
            previous_hash: previous_hash.clone(),
            event_hash: compute_event_hash(event, previous_hash.as_deref(), sequence),
            sequence,
            event: event.clone(),
        };

        let id = event.ticket_id();
        let ts = Utc::now().timestamp_millis();
        let filename = format!("{:08}-{}-{ts}.json", sequence, id);
        let path = self.events_dir.join(&filename);
        write_json_atomic(&path, &envelope)
            .with_context(|| format!("failed to write ticket event to {path:?}"))?;

        self.write_head(&LedgerHead {
            latest_hash: envelope.event_hash.clone(),
            sequence,
        })?;

        Ok(envelope)
    }

    fn read_head(&self) -> Result<Option<LedgerHead>> {
        read_json(&self.head_path)
            .with_context(|| format!("failed to read ledger head {:?}", self.head_path))
    }

    fn write_head(&self, head: &LedgerHead) -> Result<()> {
        write_json_atomic(&self.head_path, head)
            .with_context(|| format!("failed to write ledger head {:?}", self.head_path))
    }

    /// Verify the tamper-evident chain of all ticket events.
    pub fn verify_chain(&self) -> Result<ChainVerification> {
        let mut envelopes = self.envelopes()?;
        envelopes.sort_by(|a, b| a.sequence.cmp(&b.sequence));

        let mut issues = Vec::new();
        let mut previous_hash: Option<String> = None;

        for envelope in &envelopes {
            let expected = compute_event_hash(
                &envelope.event,
                envelope.previous_hash.as_deref(),
                envelope.sequence,
            );
            if expected != envelope.event_hash {
                issues.push(format!(
                    "sequence {} has invalid event hash",
                    envelope.sequence
                ));
            }
            if envelope.previous_hash != previous_hash {
                issues.push(format!(
                    "sequence {} has broken previous_hash link",
                    envelope.sequence
                ));
            }
            previous_hash = Some(envelope.event_hash.clone());
        }

        let head = self.read_head()?;
        if let Some(head) = head {
            if previous_hash.as_ref() != Some(&head.latest_hash) {
                issues.push("ledger head does not match latest event hash".to_string());
            }
        }

        Ok(ChainVerification {
            valid: issues.is_empty(),
            event_count: envelopes.len(),
            issues,
        })
    }

    /// Return all stored event envelopes.
    pub fn envelopes(&self) -> Result<Vec<TicketEventEnvelope>> {
        let mut envelopes = Vec::new();
        for entry in std::fs::read_dir(&self.events_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "HEAD.json" || !name.ends_with(".json") {
                    continue;
                }
                envelopes.push(Self::read_event(&entry.path())?);
            }
        }
        Ok(envelopes)
    }

    /// Create a new ticket and persist it.
    pub fn create(&self, ticket: Ticket) -> Result<Ticket> {
        let event = TicketEvent::Created { ticket: ticket.clone() };
        self.append(&event)?;
        self.write_snapshot(&ticket)?;
        Ok(ticket)
    }

    /// Update an existing ticket.
    pub fn update(&self, id: &TicketId, patch: TicketUpdate) -> Result<Ticket> {
        let mut ticket = self.get(id)?.context("ticket not found")?;

        if let Some(ref title) = patch.title {
            ticket.title = title.clone();
        }
        if let Some(ref description) = patch.description {
            ticket.description = description.clone();
        }
        if let Some(ref design) = patch.design {
            ticket.design = design.clone();
        }
        if let Some(ref acceptance) = patch.acceptance {
            ticket.acceptance = acceptance.clone();
        }
        if let Some(priority) = patch.priority {
            ticket.priority = priority;
        }
        if let Some(ref assignee) = patch.assignee {
            ticket.assignee = assignee.clone();
        }
        if let Some(estimate) = patch.estimate_minutes {
            ticket.estimate_minutes = estimate;
        }
        if let Some(due_at) = patch.due_at {
            ticket.due_at = due_at;
        }
        if let Some(defer_until) = patch.defer_until {
            ticket.defer_until = defer_until;
        }
        if let Some(ref labels) = patch.labels {
            ticket.labels = labels.clone();
        }
        if let Some(ref external_ref) = patch.external_ref {
            ticket.external_ref = external_ref.clone();
        }
        if let Some(ref metadata) = patch.metadata {
            ticket.metadata = metadata.clone();
        }

        ticket.updated_at = Utc::now();

        let event = TicketEvent::Updated {
            id: id.clone(),
            title: patch.title.clone(),
            description: patch.description.clone(),
            design: patch.design.clone(),
            acceptance: patch.acceptance.clone(),
            priority: patch.priority,
            assignee: patch.assignee.clone(),
            estimate_minutes: patch.estimate_minutes,
            due_at: patch.due_at,
            defer_until: patch.defer_until,
            labels: patch.labels.clone(),
            external_ref: patch.external_ref.clone(),
            metadata: patch.metadata.clone(),
            updated_at: ticket.updated_at,
        };
        self.append(&event)?;
        self.write_snapshot(&ticket)?;
        Ok(ticket)
    }

    /// Change a ticket's status.
    pub fn set_status(
        &self,
        id: &TicketId,
        status: TicketStatus,
        actor: impl Into<String>,
        reason: Option<String>,
    ) -> Result<Ticket> {
        let mut ticket = self.get(id)?.context("ticket not found")?;
        let actor = actor.into();
        let ts = Utc::now();

        ticket.status = status;
        ticket.updated_at = ts;
        if status == TicketStatus::Closed {
            ticket.closed_at = Some(ts);
            ticket.close_reason = reason.clone();
        }

        let event = TicketEvent::StatusChanged {
            id: id.clone(),
            status,
            actor,
            reason,
            ts,
        };
        self.append(&event)?;
        self.write_snapshot(&ticket)?;
        Ok(ticket)
    }

    /// Add a note to a ticket.
    pub fn add_note(
        &self,
        id: &TicketId,
        author: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<Ticket> {
        let mut ticket = self.get(id)?.context("ticket not found")?;
        let note = TicketNote {
            id: format!("note-{}", Utc::now().timestamp_millis()),
            author: author.into(),
            body: body.into(),
            created_at: Utc::now(),
        };
        ticket.notes.push(note.clone());
        ticket.updated_at = Utc::now();

        let event = TicketEvent::NoteAdded {
            id: id.clone(),
            note,
        };
        self.append(&event)?;
        self.write_snapshot(&ticket)?;
        Ok(ticket)
    }

    /// Add a label to a ticket.
    pub fn add_label(&self, id: &TicketId, label: impl Into<String>) -> Result<Ticket> {
        let mut ticket = self.get(id)?.context("ticket not found")?;
        let label = label.into();
        let ts = Utc::now();
        if !ticket.labels.contains(&label) {
            ticket.labels.push(label.clone());
        }
        ticket.updated_at = ts;

        let event = TicketEvent::LabelAdded {
            id: id.clone(),
            label,
            ts,
        };
        self.append(&event)?;
        self.write_snapshot(&ticket)?;
        Ok(ticket)
    }

    /// Remove a label from a ticket.
    pub fn remove_label(&self, id: &TicketId, label: impl Into<String>) -> Result<Ticket> {
        let mut ticket = self.get(id)?.context("ticket not found")?;
        let label = label.into();
        let ts = Utc::now();
        ticket.labels.retain(|l| l != &label);
        ticket.updated_at = ts;

        let event = TicketEvent::LabelRemoved {
            id: id.clone(),
            label,
            ts,
        };
        self.append(&event)?;
        self.write_snapshot(&ticket)?;
        Ok(ticket)
    }

    /// Load the current ticket state, preferring the snapshot if present.
    pub fn get(&self, id: &TicketId) -> Result<Option<Ticket>> {
        let snapshot_path = self.snapshot_path(id);
        if snapshot_path.exists() {
            return read_json(&snapshot_path)
                .with_context(|| format!("failed to read ticket snapshot {snapshot_path:?}"));
        }

        // No snapshot: rebuild full state by replaying the event log.
        let mut ticket: Option<Ticket> = None;
        for event in self.events_for(id)? {
            match event {
                TicketEvent::Created { ticket: t } => ticket = Some(t),
                ref e => {
                    if let Some(ref mut t) = ticket {
                        apply_event(t, e);
                    }
                }
            }
        }
        Ok(ticket)
    }

    /// Return all tickets, projected from snapshots.
    pub fn list(&self) -> Result<Vec<Ticket>> {
        let mut tickets = Vec::new();
        for entry in std::fs::read_dir(&self.snapshots_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let ticket: Ticket = read_json(&entry.path())?
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty snapshot"))?;
                tickets.push(ticket);
            }
        }
        tickets.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(tickets)
    }

    /// Rebuild all snapshots from the event log.
    pub fn rebuild_snapshots(&self) -> Result<usize> {
        let mut tickets: HashMap<TicketId, Ticket> = HashMap::new();
        for event in self.all_events()? {
            match event {
                TicketEvent::Created { ticket } => {
                    tickets.insert(ticket.id.clone(), ticket);
                }
                ref e => {
                    let id = e.ticket_id().clone();
                    if let Some(mut ticket) = tickets.get(&id).cloned() {
                        apply_event(&mut ticket, e);
                        tickets.insert(id, ticket);
                    }
                }
            }
        }

        let count = tickets.len();
        for ticket in tickets.values() {
            self.write_snapshot(ticket)?;
        }
        Ok(count)
    }

    fn snapshot_path(&self, id: &TicketId) -> PathBuf {
        self.snapshots_dir.join(format!("{}.json", id))
    }

    fn write_snapshot(&self, ticket: &Ticket) -> Result<()> {
        let path = self.snapshot_path(&ticket.id);
        write_json_atomic(&path, ticket)
            .with_context(|| format!("failed to write ticket snapshot {path:?}"))
    }

    fn events_for(&self, id: &TicketId) -> Result<Vec<TicketEvent>> {
        let mut envelopes = Vec::new();
        // Event files are named `{sequence:08}-{id}-{ts}.json`; the id is the
        // middle segment, so match on `-{id}-` rather than a name prefix.
        let needle = format!("-{id}-");
        for entry in std::fs::read_dir(&self.events_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains(&needle) && name.ends_with(".json") {
                let event = Self::read_event(&entry.path())?;
                envelopes.push(event);
            }
        }
        envelopes.sort_by(|a, b| a.sequence.cmp(&b.sequence));
        Ok(envelopes.into_iter().map(|e| e.event).collect())
    }

    /// Return all events in the log, ordered by sequence.
    ///
    /// Dependency events span two tickets but are filed under a single id, so
    /// anything that rebuilds cross-ticket state (the dependency graph) must
    /// use this full-log scan rather than `events_for(id)`.
    pub fn all_events(&self) -> Result<Vec<TicketEvent>> {
        let mut envelopes = Vec::new();
        for entry in std::fs::read_dir(&self.events_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "HEAD.json" {
                    continue;
                }
                let event = Self::read_event(&entry.path())?;
                envelopes.push(event);
            }
        }
        envelopes.sort_by(|a, b| a.sequence.cmp(&b.sequence));
        Ok(envelopes.into_iter().map(|e| e.event).collect())
    }

    fn read_event(path: &Path) -> Result<TicketEventEnvelope> {
        // Prefer tamper-evident envelope; fall back to legacy plain event.
        if let Some(envelope) = read_json::<TicketEventEnvelope>(path)? {
            return Ok(envelope);
        }
        if let Some(event) = read_json::<TicketEvent>(path)? {
            return Ok(TicketEventEnvelope {
                previous_hash: None,
                event_hash: compute_event_hash(&event, None, 0),
                sequence: 0,
                event,
            });
        }
        Err(io::Error::new(io::ErrorKind::InvalidData, "empty or invalid event").into())
    }
}

/// Result of verifying the tamper-evident event chain.
#[derive(Clone, Debug, Default)]
pub struct ChainVerification {
    pub valid: bool,
    pub event_count: usize,
    pub issues: Vec<String>,
}

fn compute_event_hash(
    event: &TicketEvent,
    previous_hash: Option<&str>,
    sequence: u64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sequence.to_string().as_bytes());
    if let Some(prev) = previous_hash {
        hasher.update(prev.as_bytes());
    }
    hasher.update(b"\0");
    let event_bytes = serde_json::to_vec(event).expect("event serializes");
    hasher.update(&event_bytes);
    hex::encode(hasher.finalize())
}

/// Apply a single event to an in-flight ticket state.
///
/// Used by both the no-snapshot fallback in `get` and `rebuild_snapshots` so
/// event-only replay stays consistent everywhere. Dependency events do not
/// mutate ticket state and are ignored.
fn apply_event(ticket: &mut Ticket, event: &TicketEvent) {
    match event {
        TicketEvent::Created { .. } => {}
        TicketEvent::Updated {
            title,
            description,
            design,
            acceptance,
            priority,
            assignee,
            estimate_minutes,
            due_at,
            defer_until,
            labels,
            external_ref,
            metadata,
            updated_at,
            ..
        } => {
            if let Some(title) = title {
                ticket.title = title.clone();
            }
            if let Some(description) = description {
                ticket.description = description.clone();
            }
            if let Some(design) = design {
                ticket.design = design.clone();
            }
            if let Some(acceptance) = acceptance {
                ticket.acceptance = acceptance.clone();
            }
            if let Some(priority) = priority {
                ticket.priority = *priority;
            }
            if let Some(assignee) = assignee {
                ticket.assignee = assignee.clone();
            }
            if let Some(estimate) = estimate_minutes {
                ticket.estimate_minutes = *estimate;
            }
            if let Some(due_at) = due_at {
                ticket.due_at = *due_at;
            }
            if let Some(defer_until) = defer_until {
                ticket.defer_until = *defer_until;
            }
            if let Some(labels) = labels {
                ticket.labels = labels.clone();
            }
            if let Some(external_ref) = external_ref {
                ticket.external_ref = external_ref.clone();
            }
            if let Some(metadata) = metadata {
                ticket.metadata = metadata.clone();
            }
            ticket.updated_at = *updated_at;
        }
        TicketEvent::StatusChanged {
            status,
            reason,
            ts,
            ..
        } => {
            ticket.status = *status;
            ticket.updated_at = *ts;
            if *status == TicketStatus::Closed {
                ticket.closed_at = Some(*ts);
                ticket.close_reason = reason.clone();
            }
        }
        TicketEvent::NoteAdded { note, .. } => {
            ticket.notes.push(note.clone());
        }
        TicketEvent::LabelAdded { label, ts, .. } => {
            if !ticket.labels.contains(label) {
                ticket.labels.push(label.clone());
            }
            ticket.updated_at = *ts;
        }
        TicketEvent::LabelRemoved { label, ts, .. } => {
            ticket.labels.retain(|l| l != label);
            ticket.updated_at = *ts;
        }
        TicketEvent::DependencyAdded { .. } | TicketEvent::DependencyRemoved { .. } => {}
    }
}

/// A ticket that is not ready, together with the ids of the open tickets
/// whose `blocks` edges hold it back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockedTicket {
    pub ticket: Ticket,
    pub blocked_by: Vec<TicketId>,
}

/// Compute the ready list: tickets that are open, not deferred, have every
/// incoming `blocks` edge closed, and have no unsatisfied wait-gate.
///
/// This is the single shared implementation — MCP, the CLI, and the HTTP
/// surface all call it.
pub fn ready(
    tickets: &[Ticket],
    graph: &DependencyGraph,
    gates: &WaitGateStore,
    now: DateTime<Utc>,
) -> Result<Vec<Ticket>> {
    let mut ready = Vec::new();
    for ticket in tickets {
        if !ticket.is_open() || ticket.is_deferred(now) {
            continue;
        }
        let blockers_closed = graph.blocks(&ticket.id).into_iter().all(|id| {
            tickets
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.status == TicketStatus::Closed)
                .unwrap_or(true)
        });
        if !blockers_closed {
            continue;
        }
        if !gates.blocking_for(&ticket.id)?.is_empty() {
            continue;
        }
        ready.push(ticket.clone());
    }
    Ok(ready)
}

/// Compute the blocked list: open, non-deferred tickets that are held back by
/// at least one open incoming `blocks` edge or one unsatisfied wait-gate.
pub fn blocked(
    tickets: &[Ticket],
    graph: &DependencyGraph,
    gates: &WaitGateStore,
    now: DateTime<Utc>,
) -> Result<Vec<BlockedTicket>> {
    let mut blocked = Vec::new();
    for ticket in tickets {
        if !ticket.is_open() || ticket.is_deferred(now) {
            continue;
        }
        let open_blockers: Vec<TicketId> = graph
            .blocks(&ticket.id)
            .into_iter()
            .filter(|id| {
                tickets
                    .iter()
                    .find(|t| &t.id == *id)
                    .map(|t| t.status != TicketStatus::Closed)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        let gated = !gates.blocking_for(&ticket.id)?.is_empty();
        if !open_blockers.is_empty() || gated {
            blocked.push(BlockedTicket {
                ticket: ticket.clone(),
                blocked_by: open_blockers,
            });
        }
    }
    Ok(blocked)
}

/// Patch fields for [`TicketStore::update`].
#[derive(Clone, Debug, Default)]
pub struct TicketUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub design: Option<Option<String>>,
    pub acceptance: Option<Option<String>>,
    pub priority: Option<TicketPriority>,
    pub assignee: Option<Option<String>>,
    pub estimate_minutes: Option<Option<u32>>,
    pub due_at: Option<Option<DateTime<Utc>>>,
    pub defer_until: Option<Option<DateTime<Utc>>>,
    pub labels: Option<Vec<String>>,
    pub external_ref: Option<Option<String>>,
    pub metadata: Option<HashMap<String, Value>>,
}

impl From<Ticket> for TicketUpdate {
    fn from(ticket: Ticket) -> Self {
        Self {
            title: Some(ticket.title),
            description: Some(ticket.description),
            design: Some(ticket.design),
            acceptance: Some(ticket.acceptance),
            priority: Some(ticket.priority),
            assignee: Some(ticket.assignee),
            estimate_minutes: Some(ticket.estimate_minutes),
            due_at: Some(ticket.due_at),
            defer_until: Some(ticket.defer_until),
            labels: Some(ticket.labels),
            external_ref: Some(ticket.external_ref),
            metadata: Some(ticket.metadata),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_ticket(title: &str) -> Ticket {
        let id = TicketId::mint(title);
        Ticket {
            id: id.clone(),
            hierarchical_id: HierarchicalId::root(id),
            title: title.to_string(),
            description: "description".to_string(),
            design: None,
            acceptance: None,
            notes: Vec::new(),
            status: TicketStatus::Open,
            kind: TicketKind::Task,
            priority: TicketPriority::P2,
            assignee: None,
            estimate_minutes: None,
            due_at: None,
            defer_until: None,
            labels: Vec::new(),
            external_ref: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
            close_reason: None,
        }
    }

    #[test]
    fn create_and_get_ticket() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let ticket = sample_ticket("Fix login");
        store.create(ticket.clone()).unwrap();

        let loaded = store.get(&ticket.id).unwrap().unwrap();
        assert_eq!(loaded.title, "Fix login");
        assert_eq!(loaded.status, TicketStatus::Open);
    }

    #[test]
    fn update_ticket() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let ticket = sample_ticket("Fix login");
        store.create(ticket.clone()).unwrap();

        let updated = store
            .update(
                &ticket.id,
                TicketUpdate {
                    priority: Some(TicketPriority::P0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.priority, TicketPriority::P0);

        let loaded = store.get(&ticket.id).unwrap().unwrap();
        assert_eq!(loaded.priority, TicketPriority::P0);
    }

    #[test]
    fn close_ticket() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let ticket = sample_ticket("Fix login");
        store.create(ticket.clone()).unwrap();

        let closed = store
            .set_status(&ticket.id, TicketStatus::Closed, "agent", Some("done".to_string()))
            .unwrap();
        assert_eq!(closed.status, TicketStatus::Closed);
        assert!(closed.closed_at.is_some());
        assert_eq!(closed.close_reason.as_deref(), Some("done"));
    }

    #[test]
    fn tamper_evident_chain() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let t1 = sample_ticket("One");
        let t2 = sample_ticket("Two");
        store.create(t1.clone()).unwrap();
        store.create(t2.clone()).unwrap();

        let verification = store.verify_chain().unwrap();
        assert!(verification.valid);
        assert_eq!(verification.event_count, 2);
        assert!(verification.issues.is_empty());

        // Verify envelopes link to previous hash.
        let mut envelopes = store.envelopes().unwrap();
        envelopes.sort_by(|a, b| a.sequence.cmp(&b.sequence));
        assert_eq!(envelopes.len(), 2);
        assert!(envelopes[0].previous_hash.is_none());
        assert_eq!(envelopes[1].previous_hash.as_ref(), Some(&envelopes[0].event_hash));
    }

    #[test]
    fn label_ops_emit_events() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let ticket = sample_ticket("Labeled");
        store.create(ticket.clone()).unwrap();

        let t = store.add_label(&ticket.id, "backend").unwrap();
        assert_eq!(t.labels, vec!["backend".to_string()]);
        let t = store.add_label(&ticket.id, "urgent").unwrap();
        assert_eq!(t.labels.len(), 2);
        let t = store.remove_label(&ticket.id, "backend").unwrap();
        assert_eq!(t.labels, vec!["urgent".to_string()]);

        let events = store.all_events().unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, TicketEvent::LabelAdded { label, .. } if label == "backend")));
        assert!(events
            .iter()
            .any(|e| matches!(e, TicketEvent::LabelRemoved { label, .. } if label == "backend")));
    }

    #[test]
    fn get_rebuilds_state_from_events_without_snapshot() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let ticket = sample_ticket("Event sourced");
        store.create(ticket.clone()).unwrap();

        store
            .update(
                &ticket.id,
                TicketUpdate {
                    title: Some("Renamed".to_string()),
                    priority: Some(TicketPriority::P0),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .set_status(&ticket.id, TicketStatus::InProgress, "agent", None)
            .unwrap();
        store.add_label(&ticket.id, "backend").unwrap();
        store.add_label(&ticket.id, "urgent").unwrap();
        store.remove_label(&ticket.id, "urgent").unwrap();
        store.add_note(&ticket.id, "agent", "first note").unwrap();
        store.add_note(&ticket.id, "human", "second note").unwrap();

        // Remove the snapshot; get() must replay the event log.
        let snapshot = tmp
            .path()
            .join(TICKET_SNAPSHOTS_DIR)
            .join(format!("{}.json", ticket.id));
        std::fs::remove_file(&snapshot).unwrap();

        let rebuilt = store.get(&ticket.id).unwrap().unwrap();
        assert_eq!(rebuilt.title, "Renamed");
        assert_eq!(rebuilt.priority, TicketPriority::P0);
        assert_eq!(rebuilt.status, TicketStatus::InProgress);
        assert_eq!(rebuilt.labels, vec!["backend".to_string()]);
        assert_eq!(rebuilt.notes.len(), 2);
        assert_eq!(rebuilt.notes[0].body, "first note");
        assert_eq!(rebuilt.notes[1].author, "human");
    }

    #[test]
    fn ready_excludes_blocked_and_gated_tickets() {
        use crate::dependencies::{add_edge, load_graph, DependencyEdge, DependencyKind};
        use crate::wait_gates::{WaitGateKind, WaitGateStore};

        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        let blocker = sample_ticket("blocker");
        let blocked_t = sample_ticket("blocked");
        let gated = sample_ticket("gated");
        let free = sample_ticket("free");
        store.create(blocker.clone()).unwrap();
        store.create(blocked_t.clone()).unwrap();
        store.create(gated.clone()).unwrap();
        store.create(free.clone()).unwrap();

        add_edge(
            tmp.path(),
            &store,
            DependencyEdge::new(blocker.id.clone(), blocked_t.id.clone(), DependencyKind::Blocks),
        )
        .unwrap();

        // Unsatisfied timer gate (far future) blocks the gated ticket.
        let gates = WaitGateStore::new(tmp.path()).unwrap();
        let mut params = HashMap::new();
        params.insert(
            "until".to_string(),
            serde_json::json!("2999-01-01T00:00:00Z"),
        );
        gates
            .add(gated.id.clone(), WaitGateKind::Timer, "wait".to_string(), params)
            .unwrap();

        let graph = load_graph(tmp.path()).unwrap();
        let all = store.list().unwrap();
        let ready = super::ready(&all, &graph, &gates, Utc::now()).unwrap();
        let ready_ids: Vec<_> = ready.iter().map(|t| t.id.clone()).collect();
        assert!(ready_ids.contains(&blocker.id));
        assert!(ready_ids.contains(&free.id));
        assert!(!ready_ids.contains(&blocked_t.id));
        assert!(!ready_ids.contains(&gated.id));

        // Blocked list carries the open blockers.
        let blocked = super::blocked(&all, &graph, &gates, Utc::now()).unwrap();
        let blocked_entry = blocked
            .iter()
            .find(|b| b.ticket.id == blocked_t.id)
            .unwrap();
        assert_eq!(blocked_entry.blocked_by, vec![blocker.id.clone()]);
        assert!(blocked.iter().any(|b| b.ticket.id == gated.id));

        // Closing the blocker makes the blocked ticket ready.
        store
            .set_status(&blocker.id, TicketStatus::Closed, "agent", None)
            .unwrap();
        let all = store.list().unwrap();
        let ready = super::ready(&all, &graph, &gates, Utc::now()).unwrap();
        let ready_ids: Vec<_> = ready.iter().map(|t| t.id.clone()).collect();
        assert!(ready_ids.contains(&blocked_t.id));
        assert!(!ready_ids.contains(&gated.id));
    }
}
