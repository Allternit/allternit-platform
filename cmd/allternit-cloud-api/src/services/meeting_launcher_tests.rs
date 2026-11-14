//! Launcher: capacity and `no_capacity`, the runtime picker (online cloud
//! computer, then online Desktop, then asleep cloud computer; `no_runtime`;
//! busy = `no_capacity`), what the bot gets at start (no secret in the pod
//! config), bot accounts (pool rules, Linux runtimes only), and the reaper
//! (old Incus handles, answered vs unreachable liveness). The relay, LiveKit
//! and Zoom are faked (network edges); the database, state machine, picker
//! and profile store (a temp dir) are real.

use super::*;
use crate::routes::livekit_admin::{CreateSipParticipantRequest, LiveKitError, ParticipantAccess};
use crate::routes::meeting_pod::hash_token;
use crate::routes::test_support::{test_state, MockGateway};
use crate::routes::zoom::{ZoomCallError, ZoomTokens, DEFAULT_REDIRECT_URI};
use crate::services::bot_profiles::{ProfileKey, ProfileStatus, ProfileStore};
use std::sync::Mutex;

const CLIENT_SECRET: &str = "zoom-client-secret-never-in-a-pod";
const DESK: &str = "rt-desk";

/// The relay edge. A runtime not in `connected` is unreachable unless the
/// call wakes it (an asleep cloud computer); `status` holds each runtime's
/// answer to the status preflight (none = no answer).
struct FakeRelay {
    connected: Mutex<HashSet<String>>,
    status: Mutex<HashMap<String, Value>>,
    /// Every relayed call: (runtime, path, body, wake).
    calls: Mutex<Vec<(String, String, Value, bool)>>,
}

impl FakeRelay {
    fn new() -> Self {
        Self { connected: Mutex::new(HashSet::new()), status: Mutex::new(HashMap::new()), calls: Mutex::new(Vec::new()) }
    }
    fn online(&self, runtime: &str, status: Value) {
        self.connected.lock().unwrap().insert(runtime.to_string());
        self.status.lock().unwrap().insert(runtime.to_string(), status);
    }
    fn starts(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().iter().filter(|c| c.1 == BOT_START_PATH).map(|c| (c.0.clone(), c.2.clone())).collect()
    }
    fn start_for(&self, sid: Uuid) -> (String, Value) {
        self.starts().into_iter().find(|(_, body)| body["session_id"] == sid.to_string()).expect("bot started")
    }
}

fn ready(os: &str) -> Value {
    json!({ "ready": true, "reason": null, "os": os, "max": 1, "sessions": [] })
}

#[async_trait]
impl MeetingRelay for FakeRelay {
    async fn connected(&self) -> HashSet<String> {
        self.connected.lock().unwrap().clone()
    }
    async fn post(&self, _user: &str, runtime: &str, path: &str, body: &Value, wake: bool) -> RelayReply {
        self.calls.lock().unwrap().push((runtime.to_string(), path.to_string(), body.clone(), wake));
        if !wake && !self.connected.lock().unwrap().contains(runtime) {
            return RelayReply::Offline;
        }
        match path {
            BOT_STATUS_PATH => match self.status.lock().unwrap().get(runtime) {
                Some(status) => RelayReply::Answered { status: 200, body: status.clone() },
                None => RelayReply::Offline,
            },
            BOT_START_PATH => RelayReply::Answered { status: 200, body: json!({ "status": "started", "os": "linux" }) },
            _ => RelayReply::Answered { status: 200, body: json!({ "stopped": true }) },
        }
    }
}

#[derive(Default)]
struct FakeLiveKit {
    rooms: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl LiveKitAdminClient for FakeLiveKit {
    async fn ensure_inbound_trunk(&self, _n: &str, _e: &str) -> Result<String, LiveKitError> {
        Err(LiveKitError::NotConfigured)
    }
    async fn delete_inbound_trunk(&self, _t: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn ensure_dispatch_rule(&self, _t: &str, _n: &str, _b: &str, _o: &str, _e: &str) -> Result<String, LiveKitError> {
        Err(LiveKitError::NotConfigured)
    }
    async fn delete_dispatch_rule(&self, _r: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn create_sip_participant(&self, _r: CreateSipParticipantRequest) -> Result<Value, LiveKitError> {
        Err(LiveKitError::NotConfigured)
    }
    async fn create_room_with_agent(&self, room: &str, agent: &str, _metadata: &str) -> Result<(), LiveKitError> {
        self.rooms.lock().unwrap().push((room.to_string(), agent.to_string()));
        Ok(())
    }
    async fn send_data(&self, _r: &str, _t: &str, _p: &[u8]) -> Result<(), LiveKitError> {
        Ok(())
    }
    fn participant_access(&self, room: &str, identity: &str, _p: bool) -> Result<ParticipantAccess, LiveKitError> {
        Ok(ParticipantAccess { token: format!("lk-{room}-{identity}"), url: "wss://lk.example".into() })
    }
}

struct FakeZoom;

#[async_trait]
impl ZoomApi for FakeZoom {
    async fn exchange_code(&self, _c: &str) -> Result<ZoomTokens, ZoomCallError> {
        Err(ZoomCallError::Status(400))
    }
    async fn refresh(&self, _r: &str) -> Result<ZoomTokens, ZoomCallError> {
        Ok(ZoomTokens { access_token: "zoom-access-secret".into(), refresh_token: "zoom-refresh-secret-2".into(), expires_in: 3600, scopes: vec![] })
    }
    async fn obf_token(&self, _a: &str) -> Result<String, ZoomCallError> {
        Ok("obf-token-1".into())
    }
    async fn local_recording_token(&self, _a: &str, _n: &str) -> Result<String, ZoomCallError> {
        Err(ZoomCallError::Status(400))
    }
    async fn revoke(&self, _a: &str) -> Result<(), ZoomCallError> {
        Ok(())
    }
}

struct Rig {
    db: PgPool,
    relay: Arc<FakeRelay>,
    livekit: Arc<FakeLiveKit>,
    launcher: Launcher,
    store: ProfileStore,
    _profiles_dir: tempfile::TempDir,
}

fn profile_key() -> ProfileKey {
    ProfileKey::from_base64(&STANDARD.encode([9u8; 32])).unwrap()
}

/// A runtime row owned by `user`: `cloud` = a cloud computer (with a
/// provisioned instance), else a Desktop. `seen` orders rows in a group.
async fn add_runtime(db: &PgPool, id: &str, user: &str, cloud: bool, seen: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, capabilities, credential_expires_at, last_seen_at) \
         VALUES ($1, $2, $3, $4, '[\"runtime:connect\",\"runtime:execute\"]', '2999-01-01', $5)",
    )
    .bind(id)
    .bind(user)
    .bind(format!("{id} name"))
    .bind(if cloud { "provisioned" } else { "desktop" })
    .bind(seen)
    .execute(db)
    .await
    .unwrap();
    if cloud {
        sqlx::query("INSERT INTO provisioned_instances (id, user_id, device_id, status) VALUES ($1, $2, $3, 'stopped')")
            .bind(format!("pi-{id}"))
            .bind(user)
            .bind(id)
            .execute(db)
            .await
            .unwrap();
    }
}

/// Every account in `accounts` starts signed in (active, generation 1).
/// User `u1` has one online Linux Desktop ([`DESK`]) unless a test changes it.
async fn rig(global_cap: i64, accounts: &[&str]) -> Rig {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/130_meetings.sql"),
        include_str!("../../migrations_pg/137_meeting_no_runtime.sql"),
        "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, status TEXT, updated_at TIMESTAMPTZ DEFAULT now())",
        "ALTER TABLE runtime_devices ADD COLUMN runtime_type TEXT",
        "CREATE TABLE provisioned_instances (id TEXT PRIMARY KEY, user_id TEXT, device_id TEXT, status TEXT NOT NULL)",
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.unwrap();
    }
    add_runtime(&state.db, DESK, "u1", false, Utc::now()).await;
    let relay = Arc::new(FakeRelay::new());
    relay.online(DESK, ready("linux"));
    let livekit = Arc::new(FakeLiveKit::default());
    let profiles_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(profiles_dir.path(), Some(profile_key()));
    for account in accounts {
        store.seal(account, format!("tar.zst of {account}").as_bytes(), Utc::now() - chrono::Duration::days(2), true).unwrap();
    }
    let launcher = Launcher {
        db: state.db.clone(),
        cipher: None,
        provider: Arc::new(RuntimePodProvider::new(state.db.clone(), relay.clone())),
        livekit: Some(livekit.clone()),
        zoom: Some((
            Arc::new(FakeZoom),
            ZoomConfig { client_id: "zoom-cid".into(), client_secret: CLIENT_SECRET.into(), redirect_uri: DEFAULT_REDIRECT_URI.into() },
        )),
        config: LauncherConfig {
            global_cap,
            cloud_api_url: "https://api.example".into(),
            tile_url: "https://ai.example/meeting-tile".into(),
            bot_accounts: accounts.iter().map(|a| a.to_string()).collect(),
            silent_after: chrono::Duration::minutes(20),
        },
        profiles: store.clone(),
    };
    Rig { db: state.db.clone(), relay, livekit, launcher, store, _profiles_dir: profiles_dir }
}

async fn subscribe(db: &PgPool, user: &str, plan: &str) {
    sqlx::query("INSERT INTO billing_subscriptions (user_id, plan_id, status) VALUES ($1, $2, 'active')")
        .bind(user)
        .bind(plan)
        .execute(db)
        .await
        .unwrap();
}

async fn session(db: &PgPool, user: &str, platform: &str, n: u32) -> Uuid {
    let (url, key) = match platform {
        "meet" => (format!("https://meet.google.com/abc-defg-h{n:02}"), format!("meet:abc-defg-h{n:02}")),
        "zoom" => (format!("https://zoom.us/j/8123456{n:04}?pwd=x"), format!("zoom:8123456{n:04}")),
        _ => (format!("https://teams.microsoft.com/meet/12345678{n:04}"), format!("teams:12345678{n:04}")),
    };
    let meeting: Uuid = sqlx::query_scalar(
        "INSERT INTO meetings (user_id, platform, title, join_url, url_key, source) VALUES ($1, $2, 'Weekly sync', $3, $4, 'link') RETURNING id",
    )
    .bind(user)
    .bind(platform)
    .bind(&url)
    .bind(&key)
    .fetch_one(db)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO meeting_sessions (meeting_id, user_id, url_key, state, platform) VALUES ($1, $2, $3, 'launching', $4) RETURNING id",
    )
    .bind(meeting)
    .bind(user)
    .bind(&key)
    .bind(platform)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn state_of(db: &PgPool, sid: Uuid) -> (String, Option<String>, Option<String>) {
    sqlx::query_as("SELECT state, end_reason, pod_ref FROM meeting_sessions WHERE id = $1").bind(sid).fetch_one(db).await.unwrap()
}

fn launched_on(outcome: LaunchOutcome) -> String {
    match outcome {
        LaunchOutcome::Launched(handle) => {
            assert_eq!(handle.provider, RUNTIME_PROVIDER);
            handle.host
        }
        other => panic!("not launched: {other:?}"),
    }
}

#[tokio::test]
async fn launcher_enforces_global_and_plan_caps_with_no_capacity() {
    let r = rig(2, &[]).await;
    subscribe(&r.db, "plus_user", "plus").await; // plan cap 1
    subscribe(&r.db, "u1", "ultra").await;
    add_runtime(&r.db, "rt-plus", "plus_user", false, Utc::now()).await;
    r.relay.online("rt-plus", ready("linux"));

    let a = session(&r.db, "plus_user", "teams", 1).await;
    assert_eq!(launched_on(r.launcher.launch_session(a).await.unwrap()), "rt-plus");
    // Launching again is a no-op: one claim per session.
    assert_eq!(r.launcher.launch_session(a).await.unwrap(), LaunchOutcome::Skipped);

    let b = session(&r.db, "plus_user", "teams", 2).await;
    assert_eq!(r.launcher.launch_session(b).await.unwrap(), LaunchOutcome::Ended(EndReason::NoCapacity));
    let (state, reason, pod_ref) = state_of(&r.db, b).await;
    assert_eq!((state.as_str(), reason.as_deref(), pod_ref), ("ended", Some("no_capacity"), None));

    let c = session(&r.db, "u1", "teams", 3).await;
    assert_eq!(launched_on(r.launcher.launch_session(c).await.unwrap()), DESK);
    // Global cap 2 reached (a and c hold bots).
    let d = session(&r.db, "u1", "teams", 4).await;
    assert_eq!(r.launcher.launch_session(d).await.unwrap(), LaunchOutcome::Ended(EndReason::NoCapacity));
    assert_eq!(r.relay.starts().len(), 2, "no bot for a session over a cap");
}

#[tokio::test]
async fn launcher_picks_online_cloud_then_desktop_then_asleep_cloud() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let older = Utc::now() - chrono::Duration::hours(1);
    add_runtime(&r.db, "rt-cloud", "u1", true, older).await;
    add_runtime(&r.db, "rt-cloud-asleep", "u1", true, Utc::now()).await;
    r.relay.online("rt-cloud", ready("linux"));
    // Asleep: no live connection; it answers once a relay wakes it.
    r.relay.status.lock().unwrap().insert("rt-cloud-asleep".into(), ready("linux"));

    // An online cloud computer beats the online Desktop (seen more recently).
    let first = session(&r.db, "u1", "teams", 1).await;
    assert_eq!(launched_on(r.launcher.launch_session(first).await.unwrap()), "rt-cloud");
    let runner = runner_status(&r.db, "u1", &r.relay.connected().await).await.unwrap();
    assert_eq!(runner, json!({ "state": "ready", "runtime": { "id": "rt-cloud", "name": "rt-cloud name", "kind": "cloud_computer" } }));

    // The cloud computer is busy: the online Desktop takes the next call.
    r.relay.online("rt-cloud", json!({ "ready": false, "reason": "busy", "os": "linux", "max": 1, "sessions": [] }));
    let second = session(&r.db, "u1", "teams", 2).await;
    assert_eq!(launched_on(r.launcher.launch_session(second).await.unwrap()), DESK);

    // Desktop closed, online cloud computer gone: the asleep one is woken.
    r.relay.connected.lock().unwrap().clear();
    let before = r.relay.calls.lock().unwrap().len();
    let third = session(&r.db, "u1", "teams", 3).await;
    assert_eq!(launched_on(r.launcher.launch_session(third).await.unwrap()), "rt-cloud-asleep");
    assert!(
        r.relay.calls.lock().unwrap()[before..].iter().all(|(rt, ..)| rt != DESK),
        "an offline Desktop is not a candidate"
    );
    let runner = runner_status(&r.db, "u1", &HashSet::new()).await.unwrap();
    assert_eq!(runner["state"], "asleep");
    assert_eq!(runner["runtime"]["id"], "rt-cloud-asleep", "the most recently seen asleep cloud computer");
}

#[tokio::test]
async fn launcher_ends_no_runtime_when_nothing_is_reachable() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u2", "ultra").await;
    // No runtime at all.
    let none = session(&r.db, "u2", "teams", 1).await;
    assert_eq!(r.launcher.launch_session(none).await.unwrap(), LaunchOutcome::Ended(EndReason::NoRuntime));
    assert_eq!(state_of(&r.db, none).await, ("ended".into(), Some("no_runtime".into()), None));
    assert_eq!(runner_status(&r.db, "u2", &HashSet::new()).await.unwrap(), json!({ "state": "none", "runtime": null }));

    // An offline Desktop, and an online runtime from before the meeting-bot
    // host (no status answer): still no runtime.
    add_runtime(&r.db, "rt-u2-desk", "u2", false, Utc::now()).await;
    add_runtime(&r.db, "rt-u2-old", "u2", true, Utc::now()).await;
    r.relay.connected.lock().unwrap().insert("rt-u2-old".into());
    let offline = session(&r.db, "u2", "teams", 2).await;
    assert_eq!(r.launcher.launch_session(offline).await.unwrap(), LaunchOutcome::Ended(EndReason::NoRuntime));
    assert!(r.relay.starts().is_empty());
}

#[tokio::test]
async fn launcher_ends_no_capacity_when_every_runtime_is_busy() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    r.relay.online(DESK, json!({ "ready": false, "reason": "busy", "os": "macos", "max": 1, "sessions": [] }));
    let sid = session(&r.db, "u1", "teams", 1).await;
    assert_eq!(r.launcher.launch_session(sid).await.unwrap(), LaunchOutcome::Ended(EndReason::NoCapacity));
    assert_eq!(state_of(&r.db, sid).await, ("ended".into(), Some("no_capacity".into()), None));
    assert!(r.relay.starts().is_empty());
}

#[tokio::test]
async fn launcher_start_carries_tokens_outside_the_pod_config() {
    let r = rig(20, &["gizzi-meet-01@example.com"]).await;
    subscribe(&r.db, "u1", "ultra").await;
    sqlx::query("INSERT INTO zoom_connections (user_id, encrypted_refresh_token) VALUES ('u1', 'zoom-refresh-secret-1')")
        .execute(&r.db)
        .await
        .unwrap();
    let meet = session(&r.db, "u1", "meet", 1).await;
    let meet2 = session(&r.db, "u1", "meet", 2).await;
    let zoom = session(&r.db, "u1", "zoom", 3).await;
    let teams = session(&r.db, "u1", "teams", 4).await;
    for sid in [meet, meet2, zoom, teams] {
        assert!(matches!(r.launcher.launch_session(sid).await.unwrap(), LaunchOutcome::Launched(_)), "{sid}");
    }

    let (runtime, body) = r.relay.start_for(meet);
    assert_eq!(runtime, DESK);
    let config = &body["pod_config"];
    assert_eq!(config["identity"], json!({ "kind": "google_account", "accountEmail": "gizzi-meet-01@example.com", "profileDir": BOT_PROFILE_DIR }));
    assert_eq!(body["bot_profile"], json!({ "account": "gizzi-meet-01@example.com" }));
    assert_eq!(config["displayName"], "Gizzi (Allternit)");
    assert_eq!(config["livekit"], json!({ "url": "wss://lk.example", "room": format!("meeting-{meet}"), "token": "" }));
    assert_eq!(config["podToken"], "");
    assert_eq!(config["cloudApiUrl"], "https://api.example");
    assert_eq!(config["meetingTitle"], "Weekly sync");
    assert_eq!(body["livekit_token"], format!("lk-meeting-{meet}-meeting-pod-{meet}"));
    let pod_token = body["pod_token"].as_str().unwrap();
    let stored: Option<String> = sqlx::query_scalar("SELECT pod_token_hash FROM meeting_sessions WHERE id = $1")
        .bind(meet)
        .fetch_one(&r.db)
        .await
        .unwrap();
    assert_eq!(stored, Some(hash_token(pod_token)), "the bot's token is the one cloud-api accepts");

    // The only bot account is busy: the second Meet bot joins signed out.
    let (_, body2) = r.relay.start_for(meet2);
    assert_eq!((body2["pod_config"]["identity"].clone(), body2["bot_profile"].clone()), (json!({ "kind": "anonymous" }), Value::Null));

    let (_, body) = r.relay.start_for(zoom);
    let config = &body["pod_config"];
    assert_eq!(config["identity"]["kind"], "zoom_obf");
    assert_eq!(config["identity"]["obfToken"], "obf-token-1");
    assert_eq!(config["identity"]["sdkKey"], "zoom-cid");
    assert!(config["identity"]["sdkSecretRef"].as_str().unwrap().starts_with("jwt:"));
    for secret in [CLIENT_SECRET, "zoom-refresh-secret", "zoom-access-secret"] {
        assert!(!body.to_string().contains(secret), "{secret} reached the runtime");
    }

    let (_, body) = r.relay.start_for(teams);
    assert_eq!(body["pod_config"]["identity"], json!({ "kind": "anonymous" }));
    assert_eq!(body["pod_config"]["displayName"], TEAMS_DISPLAY_NAME);
    assert!(body["pod_config"]["noticeText"].as_str().unwrap().starts_with(TEAMS_DISPLAY_NAME));

    let rooms = r.livekit.rooms.lock().unwrap().clone();
    assert!(rooms.contains(&(format!("meeting-{teams}"), MEETING_AGENT_NAME.to_string())));
}

#[tokio::test]
async fn launcher_ends_join_failed_when_zoom_is_not_connected() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let sid = session(&r.db, "u1", "zoom", 1).await;
    assert_eq!(r.launcher.launch_session(sid).await.unwrap(), LaunchOutcome::Ended(EndReason::JoinFailed));
    assert!(r.relay.starts().is_empty());
}

#[tokio::test]
async fn launcher_meet_account_only_on_linux_and_never_needs_reauth() {
    // a: needs a new sign-in; b: listed but never signed in; c: active.
    let mut r = rig(20, &["a@example.com", "c@example.com"]).await;
    r.launcher.config.bot_accounts = vec!["a@example.com".into(), "b@example.com".into(), "c@example.com".into()];
    r.store.set_status("a@example.com", ProfileStatus::NeedsReauth, Utc::now()).unwrap();
    subscribe(&r.db, "u1", "ultra").await;

    let first = session(&r.db, "u1", "meet", 1).await;
    r.launcher.launch_session(first).await.unwrap();
    assert_eq!(r.relay.start_for(first).1["bot_profile"], json!({ "account": "c@example.com" }));

    // A Mac runtime joins Meet signed out, and the account goes back to the pool.
    set_state(&r.db, first, SessionState::Launching, SessionState::Ended, Some(EndReason::LeftByUser)).await.unwrap();
    r.launcher.reap(Utc::now()).await.unwrap();
    r.relay.online(DESK, ready("macos"));
    let mac = session(&r.db, "u1", "meet", 2).await;
    r.launcher.launch_session(mac).await.unwrap();
    let (_, body) = r.relay.start_for(mac);
    assert_eq!((body["pod_config"]["identity"].clone(), body["bot_profile"].clone()), (json!({ "kind": "anonymous" }), Value::Null));
    assert_eq!(PodRef::parse(&state_of(&r.db, mac).await.2.unwrap()).unwrap().bot_account, None);
}

#[tokio::test]
async fn launcher_signed_out_report_takes_the_account_out_of_the_pool() {
    let r = rig(20, &["bot@example.com"]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let first = session(&r.db, "u1", "meet", 1).await;
    r.launcher.launch_session(first).await.unwrap();
    note_bot_profile_signed_out(&r.db, &r.store, first).await;
    let state = r.store.state("bot@example.com").unwrap().unwrap();
    assert_eq!(state.status, ProfileStatus::NeedsReauth);
    assert!(state.last_signed_out_at.is_some());

    // Even once the first session is over, the account stays out until re-signed in.
    set_state(&r.db, first, SessionState::Launching, SessionState::Ended, Some(EndReason::GuestBlocked)).await.unwrap();
    r.launcher.reap(Utc::now()).await.unwrap();
    let next = session(&r.db, "u1", "meet", 2).await;
    r.launcher.launch_session(next).await.unwrap();
    assert_eq!(r.relay.start_for(next).1["pod_config"]["identity"], json!({ "kind": "anonymous" }));
}

#[tokio::test]
async fn launcher_reaper_stops_ended_bots_after_the_grace() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let ended = session(&r.db, "u1", "teams", 1).await;
    let healthy = session(&r.db, "u1", "teams", 2).await;
    for sid in [ended, healthy] {
        r.launcher.launch_session(sid).await.unwrap();
    }
    let stops = || r.relay.calls.lock().unwrap().iter().filter(|c| c.1 == BOT_STOP_PATH).count();
    // The user leaves before the bot went live: stopped at once.
    set_state(&r.db, ended, SessionState::Launching, SessionState::Ended, Some(EndReason::LeftByUser)).await.unwrap();
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 1);
    assert_eq!(state_of(&r.db, ended).await.2, None);
    assert_eq!(stops(), 1);

    // A live session's bot stays for the 5-minute end grace.
    sqlx::query("UPDATE meeting_sessions SET state = 'live', live_at = now() WHERE id = $1").bind(healthy).execute(&r.db).await.unwrap();
    set_state(&r.db, healthy, SessionState::Live, SessionState::Ended, Some(EndReason::EmptyRoom)).await.unwrap();
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 0);
    // Even with the runtime offline by then, the stop is done (nothing to reach).
    r.relay.connected.lock().unwrap().clear();
    assert_eq!(r.launcher.reap(Utc::now() + chrono::Duration::minutes(6)).await.unwrap(), 1);
    assert_eq!(state_of(&r.db, healthy).await.2, None);
}

#[tokio::test]
async fn launcher_reaper_clears_an_old_incus_handle_without_calling_it() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let live = session(&r.db, "u1", "teams", 1).await;
    let ended = session(&r.db, "u1", "teams", 2).await;
    // pod_refs written by the removed Incus launcher.
    for (sid, state) in [(live, "live"), (ended, "launching")] {
        let old = format!(
            r#"{{"claim":"c-{sid}","handle":{{"provider":"incus","host":"host_standby","name":"{}","cpu_cores":2,"memory_mb":3072,"disk_gb":12}}}}"#,
            pod_name(sid)
        );
        sqlx::query("UPDATE meeting_sessions SET pod_ref = $2, state = $3, live_at = now() WHERE id = $1")
            .bind(sid)
            .bind(old)
            .bind(state)
            .execute(&r.db)
            .await
            .unwrap();
    }
    set_state(&r.db, ended, SessionState::Launching, SessionState::Ended, Some(EndReason::LeftByUser)).await.unwrap();

    assert_eq!(r.launcher.reap(Utc::now() + chrono::Duration::minutes(6)).await.unwrap(), 2);
    assert_eq!(state_of(&r.db, live).await, ("ended".into(), Some("error".into()), None));
    assert_eq!(state_of(&r.db, ended).await, ("ended".into(), Some("left_by_user".into()), None));
    assert!(
        r.relay.calls.lock().unwrap().iter().all(|(runtime, ..)| runtime != "host_standby"),
        "an Incus host is never called"
    );
}

#[tokio::test]
async fn launcher_reaper_gone_only_when_the_runtime_says_so() {
    let r = rig(20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let sid = session(&r.db, "u1", "teams", 1).await;
    r.launcher.launch_session(sid).await.unwrap();
    sqlx::query("UPDATE meeting_sessions SET state = 'live', live_at = now() WHERE id = $1").bind(sid).execute(&r.db).await.unwrap();
    let status = |running: bool| json!({ "ready": false, "reason": "busy", "os": "linux", "max": 1, "sessions": [{ "session_id": sid, "running": running, "exit_code": null }] });

    // Running: left alone.
    r.relay.online(DESK, status(true));
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 0);
    assert_eq!(state_of(&r.db, sid).await.0, "live");

    // The runtime is unreachable: not gone by itself...
    r.relay.connected.lock().unwrap().clear();
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 0);
    assert_eq!(state_of(&r.db, sid).await.0, "live");
    // ...but 20 silent minutes later the session ends `no_runtime`.
    r.launcher.reap(Utc::now() + chrono::Duration::minutes(21)).await.unwrap();
    assert_eq!(state_of(&r.db, sid).await.1.as_deref(), Some("no_runtime"));

    // An answered `running: false` ends `error` at once and frees the runtime.
    let next = session(&r.db, "u1", "teams", 2).await;
    r.relay.online(DESK, ready("linux"));
    r.launcher.launch_session(next).await.unwrap();
    sqlx::query("UPDATE meeting_sessions SET state = 'joining' WHERE id = $1").bind(next).execute(&r.db).await.unwrap();
    r.relay.online(DESK, json!({ "ready": true, "os": "linux", "max": 1, "sessions": [{ "session_id": next, "running": false, "exit_code": 1 }] }));
    assert!(r.launcher.reap(Utc::now()).await.unwrap() >= 1);
    assert_eq!(state_of(&r.db, next).await, ("ended".into(), Some("error".into()), None));
}

#[test]
fn launcher_notice_names_the_bot_once() {
    for platform in ["meet", "zoom"] {
        let n = notice_text(platform);
        assert!(n.starts_with("Gizzi (Allternit notetaker) is transcribing this meeting as text."), "{platform}: {n}");
        assert!(!n.contains("(Allternit) ("), "{platform}: name doubled: {n}");
    }
    let n = notice_text("teams");
    assert!(n.starts_with("Gizzi - Allternit (notetaker) is transcribing this meeting as text."), "{n}");
}
