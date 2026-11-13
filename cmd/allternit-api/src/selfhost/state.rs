//! The small state the selfhost router runs on.
//!
//! Deliberately not the hosted `AppState`: no Clerk JWKS warmup, no Stripe,
//! no `cloud_api_url`, no SQLite migrations of the main API.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::broadcast;

use super::auth::OidcVerifier;
use super::egress_guard::{EgressClient, EgressError, EgressGuard};
use super::routes_sessions::{NotImplementedTurnRunner, TurnRunner};
use super::routes_work::WorkHub;
use super::settings::Settings;
use super::store::{DynStore, EventRecord, StoreResult};

/// Env var holding the one-time setup token that may mint the first admin
/// key at `POST /v1/api_keys` while the key table is empty.
pub const SETUP_TOKEN_ENV: &str = "ALLTERNIT_SELF_HOSTED_SETUP_TOKEN";

/// Env var the agent loop child gets the internal token in (step 8).
pub const INTERNAL_TOKEN_ENV: &str = "ALLTERNIT_SELFHOST_INTERNAL_TOKEN";

const EVENT_CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct SelfHostState {
    pub store: DynStore,
    pub settings: Arc<Settings>,
    pub egress: EgressClient,
    /// Random per boot. Accepted only on /internal/* and only from loopback.
    pub internal_token: Arc<str>,
    pub setup_token: Option<Arc<str>>,
    /// Every stored event, for live SSE. Receivers filter by session.
    pub events: broadcast::Sender<Arc<EventRecord>>,
    pub turn_runner: Arc<dyn TurnRunner>,
    /// Queue wake-ups and the gizzi session map (in memory, one replica).
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
}
