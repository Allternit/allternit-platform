//! The small state the selfhost router runs on.
//!
//! Deliberately not the hosted `AppState`: no Clerk JWKS warmup, no Stripe,
//! no `cloud_api_url`, no SQLite migrations of the main API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::{broadcast, watch};

use super::auth::OidcVerifier;
use super::egress_guard::{EgressClient, EgressError, EgressGuard};
use super::routes_sessions::{NotImplementedTurnRunner, TurnRunner};
use super::routes_work::WorkHub;
use super::settings::Settings;
use super::store::{DynStore, EventRecord, StoreResult, Wake};

/// Env var holding the one-time setup token that may mint the first admin
/// key at `POST /v1/api_keys` while the key table is empty.
pub const SETUP_TOKEN_ENV: &str = "ALLTERNIT_SELF_HOSTED_SETUP_TOKEN";

/// Env var the agent loop child gets the internal token in (step 8).
pub const INTERNAL_TOKEN_ENV: &str = "ALLTERNIT_SELFHOST_INTERNAL_TOKEN";

const EVENT_CHANNEL_CAPACITY: usize = 1024;
const WAKE_CHANNEL_CAPACITY: usize = 1024;

/// This server process as one replica of the deployment.
///
/// `id` is stable across restarts of the same replica (the binary reads it
/// from `<data_dir>/replica-id`, see [`super::replica`]); a bare state gets a
/// random one. Turns this replica runs are recorded under it
/// (`sessions.turn_replica`), and its liveness is the `server_replicas` row
/// the heartbeat keeps fresh.
pub struct Replica {
    pub id: String,
    pub started_at: DateTime<Utc>,
    draining: AtomicBool,
    /// Raised once the drain is over: long-lived streams end.
    stopped: watch::Sender<bool>,
}

impl Replica {
    pub fn new(id: impl Into<String>) -> Self {
        let (stopped, _) = watch::channel(false);
        Self {
            id: id.into(),
            started_at: Utc::now(),
            draining: AtomicBool::new(false),
            stopped,
        }
    }

    /// A random id, for states that are not the server binary (tests, tools).
    pub fn random() -> Self {
        Self::new(format!("r-{}", uuid::Uuid::new_v4().simple()))
    }

    /// Stop taking new turns (graceful shutdown).
    pub fn begin_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    pub fn draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// The drain is over: SSE streams on this replica end, so the HTTP
    /// server can finish its graceful shutdown.
    pub fn mark_stopped(&self) {
        self.begin_drain();
        self.stopped.send_replace(true);
    }

    pub fn stopped(&self) -> watch::Receiver<bool> {
        self.stopped.subscribe()
    }
}

#[derive(Clone)]
pub struct SelfHostState {
    pub store: DynStore,
    pub settings: Arc<Settings>,
    pub egress: EgressClient,
    /// Random per boot. Accepted only on /internal/* and only from loopback.
    pub internal_token: Arc<str>,
    pub setup_token: Option<Arc<str>>,
    /// Every event stored by this process, for live SSE. Receivers filter
    /// by session.
    pub events: broadcast::Sender<Arc<EventRecord>>,
    /// Wake hints for SSE: events and status changes made by any replica
    /// (fed by [`super::wake`]), plus local ones for changes the store made
    /// itself (failed orphaned turns). A receiver re-reads the store on one.
    pub wakes: broadcast::Sender<Wake>,
    /// This process as a replica (id, drain state).
    pub replica: Arc<Replica>,
    pub turn_runner: Arc<dyn TurnRunner>,
    /// Queue wake-ups and the gizzi session map (this process's memory;
    /// the store is the truth across replicas).
    pub work: Arc<WorkHub>,
    pub oidc: Option<Arc<OidcVerifier>>,
    pub started_at: DateTime<Utc>,
}

impl SelfHostState {
    /// Builds the egress client from the settings, makes a fresh internal
    /// token, and reads the setup token from [`SETUP_TOKEN_ENV`]. The turn
    /// runner starts as the 501 stub.
    pub fn new(store: DynStore, settings: Settings) -> Result<Self, EgressError> {
        let guard = Arc::new(EgressGuard::from_settings(&settings));
        let egress = guard.client()?;
        let oidc = settings
            .auth
            .oidc
            .clone()
            .map(|o| Arc::new(OidcVerifier::new(o, egress.clone())));
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (wakes, _) = broadcast::channel(WAKE_CHANNEL_CAPACITY);
        let setup_token = std::env::var(SETUP_TOKEN_ENV)
            .ok()
            .filter(|t| !t.is_empty())
            .map(Arc::from);
        Ok(Self {
            store,
            settings: Arc::new(settings),
            egress,
            internal_token: Arc::from(super::store::new_secret("ait").as_str()),
            setup_token,
            events,
            wakes,
            replica: Arc::new(Replica::random()),
            turn_runner: Arc::new(NotImplementedTurnRunner),
            work: Arc::new(WorkHub::new()),
            oidc,
            started_at: Utc::now(),
        })
    }

    pub fn with_turn_runner(mut self, runner: Arc<dyn TurnRunner>) -> Self {
        self.turn_runner = runner;
        self
    }

    /// Use this replica id (the server binary passes the one from
    /// `<data_dir>/replica-id`).
    pub fn with_replica_id(mut self, id: impl Into<String>) -> Self {
        self.replica = Arc::new(Replica::new(id));
        self
    }

    pub fn with_setup_token(mut self, token: Option<String>) -> Self {
        self.setup_token = token.filter(|t| !t.is_empty()).map(Arc::from);
        self
    }

    /// Store an event, then publish it to live streams. Always use this,
    /// not `store.append_event`, so SSE clients see the event.
    pub async fn append_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: Value,
    ) -> StoreResult<EventRecord> {
        let ev = self.store.append_event(session_id, event_type, data).await?;
        // No receivers is fine.
        let _ = self.events.send(Arc::new(ev.clone()));
        Ok(ev)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<EventRecord>> {
        self.events.subscribe()
    }

    pub fn subscribe_wakes(&self) -> broadcast::Receiver<Wake> {
        self.wakes.subscribe()
    }

    /// Tell this process's SSE streams that the session changed in the
    /// store behind their back (no receivers is fine).
    pub fn hint_session(&self, session_id: &str, seq: i64) {
        let _ = self.wakes.send(Wake::Event {
            session_id: session_id.to_string(),
            seq,
        });
    }
}
