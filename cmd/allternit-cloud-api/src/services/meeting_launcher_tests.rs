//! Launcher: capacity and `no_capacity`, pod config per platform (no
//! secret in what the pod gets), and the reaper. The pod host, LiveKit and
//! Zoom are faked (vendor/infra edges); the database and state machine are
//! real.

use super::*;
use crate::routes::livekit_admin::{CreateSipParticipantRequest, LiveKitError, ParticipantAccess};
use crate::routes::meeting_pod::hash_token;
use crate::routes::test_support::{test_state, MockGateway};
use crate::routes::zoom::{ZoomCallError, ZoomTokens, DEFAULT_REDIRECT_URI};
use std::sync::Mutex;

const CLIENT_SECRET: &str = "zoom-client-secret-never-in-a-pod";

#[derive(Default)]
struct FakeHost {
    slots: Mutex<usize>,
    started: Mutex<Vec<(PodHandle, Vec<PodFile>)>>,
    stopped: Mutex<Vec<String>>,
    gone: Mutex<bool>,
}

#[async_trait]
impl PodProvider for FakeHost {
    async fn reserve(&self, session_id: Uuid) -> Result<PodHandle, PodError> {
        let mut slots = self.slots.lock().unwrap();
        if *slots == 0 {
            return Err(PodError::NoCapacity);
        }
        *slots -= 1;
        Ok(PodHandle { provider: "fake".into(), host: "h1".into(), name: pod_name(session_id), cpu_cores: 2, memory_mb: 3072, disk_gb: 12 })
    }
    async fn start(&self, handle: &PodHandle, files: &[PodFile]) -> Result<(), PodError> {
        self.started.lock().unwrap().push((handle.clone(), files.to_vec()));
        Ok(())
    }
    async fn liveness(&self, _handle: &PodHandle) -> Result<PodLiveness, PodError> {
        Ok(if *self.gone.lock().unwrap() { PodLiveness::Gone } else { PodLiveness::Running })
    }
    async fn stop(&self, handle: &PodHandle) -> Result<(), PodError> {
        self.stopped.lock().unwrap().push(handle.name.clone());
        *self.slots.lock().unwrap() += 1;
        Ok(())
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
    host: Arc<FakeHost>,
    livekit: Arc<FakeLiveKit>,
    launcher: Launcher,
}

async fn rig(slots: usize, global_cap: i64, accounts: &[&str]) -> Rig {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/130_meetings.sql"),
        "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, status TEXT, updated_at TIMESTAMPTZ DEFAULT now())",
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.unwrap();
    }
    let host = Arc::new(FakeHost { slots: Mutex::new(slots), ..Default::default() });
    let livekit = Arc::new(FakeLiveKit::default());
    let launcher = Launcher {
        db: state.db.clone(),
        cipher: None,
        provider: host.clone(),
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
    };
    Rig { db: state.db.clone(), host, livekit, launcher }
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

/// The pod config and env the pod would see, from the runner drop-in.
fn pod_view(files: &[PodFile]) -> (Value, String) {
    let runner = &files.iter().find(|f| f.path.contains("allternit-meeting-bot.service.d")).unwrap().content;
    let encoded = runner
        .lines()
        .find_map(|l| l.strip_prefix("Environment=\"ALLTERNIT_POD_CONFIG=")?.strip_suffix('"'))
        .unwrap();
    let config: Value = serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();
    let all: String = files.iter().map(|f| f.content.clone()).collect();
    (config, all)
}

fn env_value(all: &str, name: &str) -> String {
    all.lines()
        .find_map(|l| l.strip_prefix(&format!("Environment=\"{name}="))?.strip_suffix('"').map(str::to_string))
        .unwrap_or_else(|| panic!("{name} missing"))
}

#[tokio::test]
async fn launcher_enforces_global_and_plan_caps_with_no_capacity() {
    let r = rig(10, 2, &[]).await;
    subscribe(&r.db, "plus_user", "plus").await; // plan cap 1
    subscribe(&r.db, "ultra_user", "ultra").await;

    let a = session(&r.db, "plus_user", "teams", 1).await;
    assert!(matches!(r.launcher.launch_session(a).await.unwrap(), LaunchOutcome::Launched(_)));
    // Launching again is a no-op: one claim per session.
    assert_eq!(r.launcher.launch_session(a).await.unwrap(), LaunchOutcome::Skipped);

    let b = session(&r.db, "plus_user", "teams", 2).await;
    assert_eq!(r.launcher.launch_session(b).await.unwrap(), LaunchOutcome::Ended(EndReason::NoCapacity));
    let (state, reason, pod_ref) = state_of(&r.db, b).await;
    assert_eq!((state.as_str(), reason.as_deref(), pod_ref), ("ended", Some("no_capacity"), None));

    let c = session(&r.db, "ultra_user", "teams", 3).await;
    assert!(matches!(r.launcher.launch_session(c).await.unwrap(), LaunchOutcome::Launched(_)));
    // Global cap 2 reached (a and c hold pods).
    let d = session(&r.db, "ultra_user", "teams", 4).await;
    assert_eq!(r.launcher.launch_session(d).await.unwrap(), LaunchOutcome::Ended(EndReason::NoCapacity));
    assert_eq!(r.host.started.lock().unwrap().len(), 2, "no pod for a session over a cap");
}

#[tokio::test]
async fn launcher_ends_no_capacity_when_no_host_fits() {
    let r = rig(0, 20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let sid = session(&r.db, "u1", "teams", 1).await;
    assert_eq!(r.launcher.launch_session(sid).await.unwrap(), LaunchOutcome::Ended(EndReason::NoCapacity));
    let (state, reason, pod_ref) = state_of(&r.db, sid).await;
    assert_eq!((state.as_str(), reason.as_deref(), pod_ref), ("ended", Some("no_capacity"), None));
}

#[tokio::test]
async fn launcher_pod_config_per_platform_carries_no_secret() {
    let r = rig(10, 20, &["gizzi-meet-01@example.com"]).await;
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
    let started = r.host.started.lock().unwrap().clone();
    let view = |sid: Uuid| pod_view(&started.iter().find(|(h, _)| h.name == pod_name(sid)).unwrap().1);

    let (config, all) = view(meet);
    assert_eq!(config["identity"], json!({ "kind": "google_account", "accountEmail": "gizzi-meet-01@example.com", "profileDir": BOT_PROFILE_DIR }));
    assert_eq!(config["displayName"], "Gizzi (Allternit)");
    assert_eq!(config["livekit"], json!({ "url": "wss://lk.example", "room": format!("meeting-{meet}"), "token": "" }));
    assert_eq!(config["podToken"], "");
    assert_eq!(config["meetingTitle"], "Weekly sync");
    let pod_token = env_value(&all, "ALLTERNIT_POD_TOKEN");
    assert_eq!(env_value(&all, "ALLTERNIT_MEETING_POD_TOKEN"), pod_token);
    assert_eq!(env_value(&all, "ALLTERNIT_MEETING_CLOUD_API_URL"), "https://api.example");
    assert_eq!(env_value(&all, "ALLTERNIT_LIVEKIT_TOKEN"), format!("lk-meeting-{meet}-meeting-pod-{meet}"));
    assert!(env_value(&all, "ALLTERNIT_INTERNAL_SERVICE_TOKEN").starts_with("mit_"));
    let stored: Option<String> = sqlx::query_scalar("SELECT pod_token_hash FROM meeting_sessions WHERE id = $1")
        .bind(meet)
        .fetch_one(&r.db)
        .await
        .unwrap();
    assert_eq!(stored, Some(hash_token(&pod_token)), "the pod's token is the one cloud-api accepts");

    // The only bot account is busy: the second Meet pod joins signed out.
    assert_eq!(view(meet2).0["identity"], json!({ "kind": "anonymous" }));

    let (config, all) = view(zoom);
    assert_eq!(config["identity"]["kind"], "zoom_obf");
    assert_eq!(config["identity"]["obfToken"], "obf-token-1");
    assert_eq!(config["identity"]["sdkKey"], "zoom-cid");
    assert!(config["identity"]["sdkSecretRef"].as_str().unwrap().starts_with("jwt:"));
    for secret in [CLIENT_SECRET, "zoom-refresh-secret", "zoom-access-secret"] {
        assert!(!all.contains(secret) && !config.to_string().contains(secret), "{secret} reached the pod");
    }

    let (config, _) = view(teams);
    assert_eq!(config["identity"], json!({ "kind": "anonymous" }));
    assert_eq!(config["displayName"], TEAMS_DISPLAY_NAME);
    assert!(config["noticeText"].as_str().unwrap().starts_with(TEAMS_DISPLAY_NAME));

    let rooms = r.livekit.rooms.lock().unwrap().clone();
    assert!(rooms.contains(&(format!("meeting-{teams}"), MEETING_AGENT_NAME.to_string())));
}

#[tokio::test]
async fn launcher_ends_join_failed_when_zoom_is_not_connected() {
    let r = rig(10, 20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let sid = session(&r.db, "u1", "zoom", 1).await;
    assert_eq!(r.launcher.launch_session(sid).await.unwrap(), LaunchOutcome::Ended(EndReason::JoinFailed));
    assert!(r.host.started.lock().unwrap().is_empty());
    assert_eq!(*r.host.slots.lock().unwrap(), 10, "nothing booked");
}

#[tokio::test]
async fn launcher_reaper_tears_down_ended_and_vanished_pods() {
    let r = rig(10, 20, &[]).await;
    subscribe(&r.db, "u1", "ultra").await;
    let ended = session(&r.db, "u1", "teams", 1).await;
    let vanished = session(&r.db, "u1", "teams", 2).await;
    let healthy = session(&r.db, "u1", "teams", 3).await;
    for sid in [ended, vanished, healthy] {
        r.launcher.launch_session(sid).await.unwrap();
    }
    // The user leaves before the bot went live: torn down at once.
    set_state(&r.db, ended, SessionState::Launching, SessionState::Ended, Some(EndReason::LeftByUser)).await.unwrap();
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 1);
    assert_eq!(r.host.stopped.lock().unwrap().clone(), vec![pod_name(ended)]);
    assert_eq!(state_of(&r.db, ended).await.2, None);

    // A live session's pod stays for the 5-minute end grace.
    sqlx::query("UPDATE meeting_sessions SET state = 'live', live_at = now() WHERE id = $1").bind(healthy).execute(&r.db).await.unwrap();
    set_state(&r.db, healthy, SessionState::Live, SessionState::Ended, Some(EndReason::EmptyRoom)).await.unwrap();
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 0);
    assert_eq!(r.launcher.reap(Utc::now() + chrono::Duration::minutes(6)).await.unwrap(), 1);

    // The vanished pod's container is gone: session ends `error`, pod freed.
    *r.host.gone.lock().unwrap() = true;
    assert_eq!(r.launcher.reap(Utc::now()).await.unwrap(), 1);
    let (state, reason, pod_ref) = state_of(&r.db, vanished).await;
    assert_eq!((state.as_str(), reason.as_deref(), pod_ref), ("ended", Some("error"), None));
    assert_eq!(*r.host.slots.lock().unwrap(), 10, "every booking released");
}

#[test]
fn launcher_env_lines_escape_systemd_specifiers() {
    assert_eq!(env_line("A", "x%y\"z\\"), "Environment=\"A=x%%y\\\"z\\\\\"\n");
}
