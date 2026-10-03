//! LiveKit binding (`call-worker` feature): joins the dispatched room, wires
//! the SIP participant's audio, DTMF and the control topic into [`call`], and
//! applies [`RoomCommand`]s with the room and LiveKit's server API.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::future::BoxFuture;
use futures::StreamExt;
use livekit::participant::ParticipantKind;
use livekit::prelude::*;
use livekit::options::TrackPublishOptions;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_api::services::room::RoomClient;
use livekit_api::services::sip::{SIPClient, TransferSIPParticipantOptions};
use livekit_protocol as proto;
use tokio::sync::{mpsc, Notify};

use super::audio::{interleaved_to_mono, Resampler, CORE_INPUT_RATE, TRACK_RATE};
use super::brain::RelayBrain;
use super::call::{self, CallContext, CallDeps, HumanCoreFactory, RoomCommand, RoomInput, WarmTransfer, DEFAULT_TURN_TIMEOUT};
use super::consult::{ConsultBus, EgressRecorder, LiveKitAccess, SipConsult};
use super::hold_music::HoldMusic;
use super::transfer::ConsultDriver;
use super::recording::{Recorder, Recording, RecordingEnv, START_TIMEOUT};
use super::cloud_client::{CloudClient, Direction, StartCallRequest};
use super::config::{agent_ws_url, http_base, ws_base, WorkerConfig};
use super::controls::{dtmf_code, transfer_uri};
use super::dispatch::{run_dispatch, JobHandler, JobInfo};
use super::events::{Backoff, EventQueue};
use super::session_adapter::{connect_ws, CoreCommand};
use super::voicemail::{AnswerScreen, NoScreening, ScreenTimings, VoicemailDetector};
use super::{AGENT_NAME, CONTROL_TOPIC};

/// SIP participant attribute keys set by LiveKit SIP.
const SIP_PHONE: &str = "sip.phoneNumber";
const SIP_TRUNK_PHONE: &str = "sip.trunkPhoneNumber";
const SIP_CALL_ID: &str = "sip.callID";

pub async fn run(cfg: WorkerConfig) -> Result<()> {
    tracing::info!(?cfg, "starting call worker");
    let handler = Arc::new(CallJobs {
        cloud: CloudClient::new(&cfg.cloud_api_url, &cfg.worker_token),
        running: AtomicU32::new(0),
        cancels: Mutex::new(HashMap::new()),
        cfg: cfg.clone(),
    });
    let (key, secret) = (cfg.livekit_api_key.clone(), cfg.livekit_api_secret.clone());
    run_dispatch(agent_ws_url(&cfg.livekit_url), AGENT_NAME.into(), move || worker_token(&key, &secret), handler)
        .await
}

/// JWT for `/agent`: the `video.agent` grant (livekit-server checks `claims.Video.Agent`).
fn worker_token(key: &str, secret: &str) -> Result<String> {
    AccessToken::with_api_key(key, secret)
        .with_identity("allternit-voice-worker")
        .with_ttl(Duration::from_secs(6 * 3600))
        .with_grants(VideoGrants { agent: true, ..Default::default() })
        .to_jwt()
        .context("mint agent token")
}

struct CallJobs {
    cfg: WorkerConfig,
    cloud: CloudClient,
    running: AtomicU32,
    cancels: Mutex<HashMap<String, Arc<Notify>>>,
}

impl JobHandler for CallJobs {
    fn available(&self, job: &proto::Job) -> bool {
        job.room.as_ref().is_some_and(|r| r.name.starts_with("call-"))
            && (self.running.load(Ordering::SeqCst) as usize) < self.cfg.max_calls
    }

    fn run(self: Arc<Self>, job: JobInfo) -> BoxFuture<'static, Result<(), String>> {
        self.running.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(Notify::new());
        self.cancels.lock().unwrap().insert(job.job_id.clone(), cancel.clone());
        Box::pin(async move {
            let id = job.job_id.clone();
            let res = handle_job(self.cfg.clone(), self.cloud.clone(), job, cancel)
                .await
                .map_err(|e| format!("{e:#}"));
            if let Err(e) = &res {
                tracing::error!(job = %id, "call job failed: {e}");
            }
            self.cancels.lock().unwrap().remove(&id);
            self.running.fetch_sub(1, Ordering::SeqCst);
            res
        })
    }

    fn terminate(&self, job_id: &str) {
        if let Some(n) = self.cancels.lock().unwrap().get(job_id) {
            n.notify_one();
        }
    }

    fn load(&self) -> (f32, u32) {
        let n = self.running.load(Ordering::SeqCst);
        ((n as f32 / self.cfg.max_calls as f32).min(1.0), n)
    }
}

async fn handle_job(cfg: WorkerConfig, cloud: CloudClient, job: JobInfo, cancel: Arc<Notify>) -> Result<()> {
    let url = ws_base(job.url.as_deref().unwrap_or(&cfg.livekit_url));
    let room_name = job.room_name.clone();
    let api_host = http_base(&cfg.livekit_url);
    let rooms = RoomClient::with_api_key(&api_host, &cfg.livekit_api_key, &cfg.livekit_api_secret);
    let sip = Arc::new(SIPClient::with_api_key(&api_host, &cfg.livekit_api_key, &cfg.livekit_api_secret));

    let (room, mut lk_events) =
        Room::connect(&url, &job.token, RoomOptions::default()).await.context("join call room")?;
    let room = Arc::new(room);
    tracing::info!(room = %room_name, job = %job.job_id, "joined call room");

    // Bot track first, so the opening plays the moment it's synthesized.
    let source = NativeAudioSource::new(AudioSourceOptions::default(), TRACK_RATE, 1, 100);
    let track = LocalAudioTrack::create_audio_track("allternit-voice", RtcAudioSource::Native(source.clone()));
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() })
        .await
        .context("publish bot track")?;

    let Some(caller) = wait_for_sip(&room, &mut lk_events, Duration::from_secs(15)).await else {
        let _ = rooms.delete_room(&room_name).await;
        anyhow::bail!("no SIP participant joined {room_name}");
    };
    let caller_identity = caller.identity().to_string();
    let mut attrs = job.attributes.clone();
    attrs.extend(caller.attributes());
    let get = |k: &str| attrs.get(k).cloned().filter(|v| !v.is_empty());

    let direction = if get("direction").as_deref() == Some("outbound") { Direction::Outbound } else { Direction::Inbound };
    if direction == Direction::Outbound && get("consentRef").is_none() {
        // §4.1: outbound only through the consent gate.
        tracing::error!(room = %room_name, "outbound call without consentRef; hanging up");
        let _ = rooms.delete_room(&room_name).await;
        anyhow::bail!("outbound call without consentRef");
    }
    let remote = get(SIP_PHONE).unwrap_or_default();
    let local = get("to").or_else(|| get(SIP_TRUNK_PHONE)).unwrap_or_default();
    let start_req = StartCallRequest {
        bot_id: get("botId").unwrap_or_default(),
        number_id: get("numberId").unwrap_or_default(),
        from: if direction == Direction::Outbound { local.clone() } else { remote.clone() },
        to: if direction == Direction::Outbound { remote.clone() } else { local.clone() },
        direction: direction.clone(),
        room: room_name.clone(),
        owner_id: get("ownerId"),
        sip_call_id: get(SIP_CALL_ID),
        consent_ref: get("consentRef"),
    };

    // Call start (cloud-api cache) and the voice core open in parallel; the
    // opening is spoken as soon as both are back.
    let (start, core) = tokio::join!(
        cloud.start_call(&start_req, cfg.start_timeout),
        connect_ws(&cfg.voice_session_url, cfg.voice_session_token.as_deref(), None)
    );
    let core = match core {
        Ok(c) => c,
        Err(e) => {
            // No voice engine: nothing can be said. Don't leave the caller in silence.
            let _ = rooms.delete_room(&room_name).await;
            return Err(e.context("voice session unavailable"));
        }
    };

    let consult_bus = Arc::new(ConsultBus::default());
    let (room_tx, room_cmds) = mpsc::channel::<RoomCommand>(8192);
    let (input_tx, input_rx) = mpsc::channel::<RoomInput>(1024);
    let pump = tokio::spawn(pump(
        room.clone(),
        source,
        caller_identity.clone(),
        room_cmds,
        input_tx.clone(),
        rooms,
        sip,
        consult_bus.clone(),
    ));
    let events_task = tokio::spawn(forward_room_events(lk_events, caller_identity.clone(), input_tx.clone(), cancel));
    if let Some(t) = audio_track_of(&caller) {
        tokio::spawn(read_audio(t, input_tx.clone(), RoomInput::CallerAudio));
    }

    let mut keep_room = false;
    match start {
        Ok(resp) => {
            tracing::info!(room = %room_name, call_id = %resp.call_id, "call started");
            if let Some(v) = resp.bot.voice_id.clone().filter(|v| !v.is_empty()) {
                let _ = core.tx.send(CoreCommand::SetVoice(v)).await;
            }
            let ctx = CallContext {
                call_id: resp.call_id.clone(),
                remote,
                local,
                direction: direction.as_str().into(),
                number_id: start_req.number_id.clone(),
                bot: resp.bot,
            };
            let events = EventQueue::start(&resp.call_id, Arc::new(cloud.clone()), Backoff::default());
            let brain = Arc::new(RelayBrain::new(cloud.clone(), DEFAULT_TURN_TIMEOUT));
            let lk = LiveKitAccess {
                ws_url: url.clone(),
                api_host: api_host.clone(),
                key: cfg.livekit_api_key.clone(),
                secret: cfg.livekit_api_secret.clone(),
            };
            let voicemail: Box<dyn VoicemailDetector> = if direction == Direction::Outbound {
                Box::new(AnswerScreen::new(ScreenTimings::default()))
            } else {
                Box::new(NoScreening)
            };
            let recorder: Option<Arc<dyn Recorder>> = match &cfg.recording {
                RecordingEnv::Configured(b) => Some(Arc::new(EgressRecorder::new(&lk, b.clone()))),
                RecordingEnv::Missing(_) => None,
            };
            let recording =
                Recording::begin(recorder, ctx.bot.recording, &cfg.recording, &room_name, &ctx.call_id, START_TIMEOUT).await;
            let human_core: HumanCoreFactory = {
                let (url, token) = (cfg.voice_session_url.clone(), cfg.voice_session_token.clone());
                Arc::new(move || {
                    let (url, token) = (url.clone(), token.clone());
                    Box::pin(async move { connect_ws(&url, token.as_deref(), None).await })
                })
            };
            let warm = cfg.outbound_trunk_id.clone().map(|trunk| {
                let (lk, bus, call_room, caller) = (lk.clone(), consult_bus.clone(), room_name.clone(), caller_identity.clone());
                let launch_trunk = trunk.clone();
                WarmTransfer {
                    outbound_trunk: Some(trunk),
                    ring_timeout: cfg.transfer_ring_timeout,
                    accept_timeout: cfg.transfer_accept_timeout,
                    launch: Arc::new(move |to: &str, consent: &str| -> Arc<dyn ConsultDriver> {
                        Arc::new(SipConsult::new(
                            lk.clone(),
                            launch_trunk.clone(),
                            to.to_string(),
                            consent.to_string(),
                            call_room.clone(),
                            caller.clone(),
                            bus.clone(),
                        ))
                    }),
                }
            });
            let deps = CallDeps { voicemail, recording, human_core: Some(human_core), warm };
            let (outcome, drain) = call::run_call(ctx, core, brain, events, room_tx.clone(), input_rx, deps).await;
            keep_room = outcome.keep_room;
            tracing::info!(room = %room_name, reason = %outcome.reason, secs = outcome.duration_sec, "call ended");
            // Events keep retrying in the background until delivered.
            tokio::spawn(drain);
        }
        Err(e) => {
            tracing::error!(room = %room_name, "call start failed, speaking fallback: {e}");
            call::run_unconfigured_call(core, room_tx.clone(), input_rx, Duration::from_secs(20)).await;
        }
    }

    // Tear down: the room (and with it the SIP leg) goes away with the call.
    if !keep_room {
        let _ = room_tx.send(RoomCommand::Hangup).await;
    }
    drop(room_tx);
    let _ = pump.await;
    events_task.abort();
    let _ = room.close().await;
    Ok(())
}

async fn wait_for_sip(
    room: &Room,
    events: &mut mpsc::UnboundedReceiver<RoomEvent>,
    timeout: Duration,
) -> Option<RemoteParticipant> {
    if let Some(p) = room.remote_participants().into_values().find(|p| p.kind() == ParticipantKind::Sip) {
        return Some(p);
    }
    tokio::time::timeout(timeout, async {
        while let Some(ev) = events.recv().await {
            if let RoomEvent::ParticipantConnected(p) = ev {
                if p.kind() == ParticipantKind::Sip {
                    return Some(p);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

fn audio_track_of(p: &RemoteParticipant) -> Option<RemoteAudioTrack> {
    p.track_publications().into_values().find_map(|publ| match publ.track() {
        Some(RemoteTrack::Audio(t)) => Some(t),
        _ => None,
    })
}

/// A participant's audio → 16 kHz mono → the call, wrapped by `wrap`
/// (`CallerAudio` for the SIP caller, `HumanAudio` for a takeover human).
async fn read_audio(track: RemoteAudioTrack, tx: mpsc::Sender<RoomInput>, wrap: fn(Vec<i16>) -> RoomInput) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), CORE_INPUT_RATE as i32, 1);
    let mut resampler: Option<Resampler> = None;
    while let Some(frame) = stream.next().await {
        let mono = interleaved_to_mono(&frame.data, frame.num_channels);
        let samples = if frame.sample_rate == CORE_INPUT_RATE {
            mono
        } else {
            resampler.get_or_insert_with(|| Resampler::new(frame.sample_rate, CORE_INPUT_RATE)).process(&mono)
        };
        if tx.send(wrap(samples)).await.is_err() {
            return;
        }
    }
}

async fn forward_room_events(
    mut events: mpsc::UnboundedReceiver<RoomEvent>,
    caller: String,
    tx: mpsc::Sender<RoomInput>,
    cancel: Arc<Notify>,
) {
    loop {
        let ev = tokio::select! {
            ev = events.recv() => ev,
            _ = cancel.notified() => {
                let _ = tx.send(RoomInput::Disconnected).await;
                return;
            }
        };
        let Some(ev) = ev else {
            let _ = tx.send(RoomInput::Disconnected).await;
            return;
        };
        let input = match ev {
            RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(t), participant, .. }
                if participant.identity().to_string() == caller =>
            {
                tokio::spawn(read_audio(t, tx.clone(), RoomInput::CallerAudio));
                None
            }
            // A person who joined to take over: their speech is transcribed
            // (the call decides whether a takeover is active).
            RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(t), participant, .. }
                if participant.kind() == ParticipantKind::Standard =>
            {
                tokio::spawn(read_audio(t, tx.clone(), RoomInput::HumanAudio));
                None
            }
            RoomEvent::DataReceived { payload, topic, participant, .. } if topic.as_deref() == Some(CONTROL_TOPIC) => {
                match participant {
                    // Controls come from cloud-api through the server API only;
                    // a participant (even a takeover human) can't send them.
                    None => Some(RoomInput::Control(payload.to_vec())),
                    Some(p) => {
                        tracing::warn!(from = %p.identity(), "ignored call control sent by a participant");
                        None
                    }
                }
            }
            RoomEvent::SipDTMFReceived { code, digit, participant } => {
                let from_caller = participant.as_ref().is_none_or(|p| p.identity().to_string() == caller);
                let d = digit.filter(|d| !d.is_empty()).or_else(|| code_to_digit(code));
                match (from_caller, d) {
                    (true, Some(d)) => Some(RoomInput::Dtmf(d)),
                    _ => None,
                }
            }
            RoomEvent::ParticipantDisconnected(p) if p.identity().to_string() == caller => Some(RoomInput::CallerLeft),
            RoomEvent::Disconnected { .. } => Some(RoomInput::Disconnected),
            _ => None,
        };
        if let Some(i) = input {
            if tx.send(i).await.is_err() {
                return;
            }
        }
    }
}

fn code_to_digit(code: u32) -> Option<String> {
    "0123456789*#ABCD".chars().find(|c| dtmf_code(*c) == Some(code)).map(String::from)
}

/// Applies the call's commands. Bot audio goes through a playout queue so the
/// call loop never waits on real-time pacing (barge-in stays instant), and
/// `ClearAudio` drops both the queue and the source's internal buffer. While
/// the call is on hold the playout loop fills the gaps with hold music; bot
/// speech still has priority.
#[allow(clippy::too_many_arguments)]
async fn pump(
    room: Arc<Room>,
    source: NativeAudioSource,
    caller: String,
    mut cmds: mpsc::Receiver<RoomCommand>,
    input: mpsc::Sender<RoomInput>,
    rooms: RoomClient,
    sip: Arc<SIPClient>,
    consult: Arc<ConsultBus>,
) {
    let queue: Arc<Mutex<VecDeque<Vec<i16>>>> = Arc::default();
    let wake = Arc::new(Notify::new());
    let holding = Arc::new(AtomicBool::new(false));
    let playout = {
        let (queue, wake, source, holding) = (queue.clone(), wake.clone(), source.clone(), holding.clone());
        tokio::spawn(async move {
            let mut music = HoldMusic::new();
            loop {
                let next = queue.lock().unwrap().pop_front();
                let samples = match next {
                    Some(f) => f,
                    None if holding.load(Ordering::SeqCst) => music.next_frame(),
                    None => {
                        music.rewind();
                        // Re-check on a timer too: `HoldMusic(true)` may race the wait.
                        let _ = tokio::time::timeout(Duration::from_millis(100), wake.notified()).await;
                        continue;
                    }
                };
                let frame = AudioFrame {
                    samples_per_channel: samples.len() as u32,
                    data: Cow::Owned(samples),
                    sample_rate: TRACK_RATE,
                    num_channels: 1,
                };
                // Paces to real time once the source's 100 ms buffer is full.
                if let Err(e) = source.capture_frame(&frame).await {
                    tracing::warn!("capture_frame: {e}");
                }
            }
        })
    };
    let room_name = room.name();
    let mut hung_up = false;
    while let Some(cmd) = cmds.recv().await {
        match cmd {
            RoomCommand::PublishFrame(f) => {
                queue.lock().unwrap().push_back(f);
                wake.notify_one();
            }
            RoomCommand::ClearAudio => {
                queue.lock().unwrap().clear();
                source.clear_buffer();
            }
            RoomCommand::HoldMusic(on) => {
                holding.store(on, Ordering::SeqCst);
                if !on {
                    // Stop the music now, not when the 100 ms buffer drains.
                    queue.lock().unwrap().clear();
                    source.clear_buffer();
                }
                wake.notify_one();
            }
            RoomCommand::ConsultFrame(f) => consult.push(f),
            RoomCommand::ConsultClear => consult.clear(),
            RoomCommand::SendDtmf(digits) => {
                for c in digits.chars() {
                    let Some(code) = dtmf_code(c) else { continue };
                    let dtmf = SipDTMF { code, digit: c.to_string(), destination_identities: vec![caller.clone().into()] };
                    if let Err(e) = room.local_participant().publish_dtmf(dtmf).await {
                        tracing::warn!("publish_dtmf: {e}");
                    }
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
            }
            RoomCommand::Transfer { to } => {
                let (sip, room_name, caller, input) = (sip.clone(), room_name.clone(), caller.clone(), input.clone());
                tokio::spawn(async move {
                    let res = sip
                        .transfer_sip_participant(
                            room_name,
                            caller,
                            transfer_uri(&to),
                            TransferSIPParticipantOptions { play_dialtone: Some(false), ..Default::default() },
                        )
                        .await;
                    let (ok, reason) = match res {
                        Ok(()) => (true, None),
                        Err(e) => (false, Some(e.to_string())),
                    };
                    let _ = input.send(RoomInput::TransferResult { to, ok, reason }).await;
                });
            }
            RoomCommand::Hangup => {
                if !hung_up {
                    hung_up = true;
                    hangup(&rooms, &room_name, &caller).await;
                }
            }
            RoomCommand::HangupAfterPlayout => {
                if !hung_up {
                    hung_up = true;
                    // Let the closing line play out (queue, then the source's buffer).
                    for _ in 0..750 {
                        if queue.lock().unwrap().is_empty() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    hangup(&rooms, &room_name, &caller).await;
                }
            }
            RoomCommand::Leave => {
                // Warm transfer connected the caller and the target in this
                // room: leave it standing.
                let _ = room.close().await;
                break;
            }
        }
    }
    playout.abort();
}

/// Deleting the room removes the SIP participant (BYE to the carrier).
async fn hangup(rooms: &RoomClient, room_name: &str, caller: &str) {
    if let Err(e) = rooms.delete_room(room_name).await {
        tracing::warn!(room = %room_name, "delete_room failed, removing caller: {e}");
        let _ = rooms.remove_participant(room_name, caller).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtmf_code_roundtrip() {
        assert_eq!(code_to_digit(0).as_deref(), Some("0"));
        assert_eq!(code_to_digit(11).as_deref(), Some("#"));
        assert_eq!(code_to_digit(15).as_deref(), Some("D"));
        assert_eq!(code_to_digit(99), None);
    }

    #[test]
    fn worker_token_has_agent_grant() {
        let jwt = worker_token("APIkey", "secret-secret-secret-secret-secret").unwrap();
        let claims = livekit_api::access_token::TokenVerifier::with_api_key("APIkey", "secret-secret-secret-secret-secret")
            .verify(&jwt)
            .unwrap();
        assert!(claims.video.agent);
    }
}
