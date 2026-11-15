//! LiveKit binding (`call-worker` feature): registers as agent
//! `allternit-meeting`, joins each dispatched `meeting-<sid>` room, feeds the
//! pod's `call-audio` track to the transcription thread, sends segments to
//! the pod on `allternit.meeting.stt` and wake-word turns on
//! `allternit.meeting.turn`, and speaks the pod's `allternit.meeting.speak`
//! requests on the `tts` track. No cloud calls: everything goes through the pod.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::future::BoxFuture;
use futures::StreamExt;
use livekit::options::TrackPublishOptions;
use livekit::participant::ParticipantKind;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_protocol as proto;
use tokio::sync::{mpsc, Notify};

use super::config::{MeetingJob, MeetingWorkerConfig};
use super::pipeline::{Output, SegmentProcessor, Transcriber};
use super::reply::{speak_blocking, SpeakRequest, TTS_TRACK};
use super::{AGENT_NAME, CALL_AUDIO_TRACK, ROOM_PREFIX, SPEAK_TOPIC, STT_TOPIC, TURN_TOPIC};
use crate::call_worker::audio::{interleaved_to_mono, Resampler, TRACK_RATE};
use crate::call_worker::config::agent_ws_url;
use crate::call_worker::config::ws_base;
use crate::call_worker::dispatch::{run_dispatch, JobHandler, JobInfo};
use crate::models::PackManager;
use crate::stt::{SttEngine, SAMPLE_RATE};
use crate::tts::TtsEngine;

pub async fn run(cfg: MeetingWorkerConfig) -> Result<()> {
    tracing::info!(?cfg, "starting meeting worker");
    let packs = Arc::new(PackManager::new());
    let handler = Arc::new(MeetingJobs {
        stt: Arc::new(SttEngine::new(packs.clone())),
        tts: Arc::new(TtsEngine::new(packs)),
        running: AtomicU32::new(0),
        cancels: Mutex::new(HashMap::new()),
        cfg: cfg.clone(),
    });
    // Load the recogniser now so the first meeting doesn't lose its opening.
    {
        let (stt, model) = (handler.stt.clone(), cfg.stt_model);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = stt.prepare(model) {
                tracing::warn!("meeting worker warm-up: STT not loaded: {e}");
            }
        });
    }
    let (key, secret) = (cfg.livekit_api_key.clone(), cfg.livekit_api_secret.clone());
    run_dispatch(agent_ws_url(&cfg.livekit_url), AGENT_NAME.into(), move || worker_token(&key, &secret), handler).await
}

fn worker_token(key: &str, secret: &str) -> Result<String> {
    AccessToken::with_api_key(key, secret)
        .with_identity("allternit-meeting-worker")
        .with_ttl(Duration::from_secs(6 * 3600))
        .with_grants(VideoGrants { agent: true, ..Default::default() })
        .to_jwt()
        .context("mint agent token")
}

struct MeetingJobs {
    cfg: MeetingWorkerConfig,
    stt: Arc<SttEngine>,
    tts: Arc<TtsEngine>,
    running: AtomicU32,
    cancels: Mutex<HashMap<String, Arc<Notify>>>,
}

impl JobHandler for MeetingJobs {
    fn available(&self, job: &proto::Job) -> bool {
        job.room.as_ref().is_some_and(|r| r.name.starts_with(ROOM_PREFIX))
            && (self.running.load(Ordering::SeqCst) as usize) < self.cfg.max_sessions
    }

    fn run(self: Arc<Self>, job: JobInfo) -> BoxFuture<'static, Result<(), String>> {
        self.running.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(Notify::new());
        self.cancels.lock().unwrap().insert(job.job_id.clone(), cancel.clone());
        Box::pin(async move {
            let id = job.job_id.clone();
            let res = handle_job(&self, job, cancel).await.map_err(|e| format!("{e:#}"));
            if let Err(e) = &res {
                tracing::error!(job = %id, "meeting job failed: {e}");
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
        ((n as f32 / self.cfg.max_sessions as f32).min(1.0), n)
    }
}

/// Messages to the transcription thread.
enum AudioMsg {
    /// 16 kHz mono samples; `epoch_ms` = wall time of the first sample.
    Samples { data: Vec<f32>, epoch_ms: i64 },
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn handle_job(jobs: &MeetingJobs, job: JobInfo, cancel: Arc<Notify>) -> Result<()> {
    let meta = MeetingJob::parse(&job.room_name, &job.metadata)?;
    let url = ws_base(job.url.as_deref().unwrap_or(&jobs.cfg.livekit_url));
    let (room, mut events) = Room::connect(&url, &job.token, RoomOptions::default()).await.context("join meeting room")?;
    let room = Arc::new(room);
    tracing::info!(room = %job.room_name, session = %meta.session_id, "joined meeting room");

    let source = NativeAudioSource::new(AudioSourceOptions::default(), TRACK_RATE, 1, 100);
    let track = LocalAudioTrack::create_audio_track(TTS_TRACK, RtcAudioSource::Native(source.clone()));
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() })
        .await
        .context("publish tts track")?;

    let (audio_tx, audio_rx) = mpsc::channel::<AudioMsg>(1_000);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Output>();

    // Transcription thread (the STT stream is not Send).
    let stt_thread = {
        let (engine, model, sid) = (jobs.stt.clone(), jobs.cfg.stt_model, meta.session_id.clone());
        std::thread::Builder::new()
            .name(format!("meeting-stt-{}", meta.session_id))
            .spawn(move || transcribe_loop(engine, model, sid, audio_rx, out_tx))
            .context("spawn transcription thread")?
    };

    // Segments and turns → the pod over reliable data packets, in order.
    let outputs_task = {
        let room = room.clone();
        tokio::spawn(async move {
            let mut sent = 0u64;
            while let Some(o) = out_rx.recv().await {
                let (topic, payload) = match &o {
                    Output::Segment(s) => (STT_TOPIC, serde_json::to_vec(s)),
                    Output::Turn(t) => {
                        tracing::info!(heard = %t.heard, "wake word turn");
                        (TURN_TOPIC, serde_json::to_vec(t))
                    }
                };
                let packet = DataPacket {
                    payload: payload.unwrap_or_default(),
                    topic: Some(topic.into()),
                    reliable: true,
                    ..Default::default()
                };
                match room.local_participant().publish_data(packet).await {
                    Ok(()) => sent += 1,
                    Err(e) => tracing::warn!(topic, "publish data: {e}"),
                }
            }
            sent
        })
    };

    // Spoken replies, one at a time in arrival order: Kokoro → `tts` track.
    let (frames_tx, frames_rx) = mpsc::unbounded_channel::<Vec<i16>>();
    let playout = tokio::spawn(playout(source.clone(), frames_rx));
    let (speak_tx, mut speak_rx) = mpsc::channel::<SpeakRequest>(16);
    let speaker = {
        let (tts, voice, frames_tx) = (jobs.tts.clone(), meta.voice.clone(), frames_tx.clone());
        tokio::spawn(async move {
            while let Some(req) = speak_rx.recv().await {
                let (tts, voice, tx) = (tts.clone(), voice.clone(), frames_tx.clone());
                let turn = req.turn_id.clone();
                let res = tokio::task::spawn_blocking(move || {
                    speak_blocking(&tts, &req.text, voice.as_deref(), |frames| frames.into_iter().all(|f| tx.send(f).is_ok()))
                })
                .await;
                match res {
                    Ok(Ok(())) => tracing::info!(turn = %turn, "spoke reply"),
                    Ok(Err(e)) => tracing::error!(turn = %turn, "spoken reply failed: {e}"),
                    Err(e) => tracing::error!(turn = %turn, "spoken reply task: {e}"),
                }
            }
        })
    };

    // `call-audio` may already be published when the worker joins.
    let mut reader: Option<tokio::task::JoinHandle<()>> = None;
    for p in room.remote_participants().into_values() {
        if p.kind() == ParticipantKind::Agent {
            continue;
        }
        for publ in p.track_publications().into_values() {
            if let (Some(RemoteTrack::Audio(t)), true) = (publ.track(), publ.name() == CALL_AUDIO_TRACK) {
                reader = Some(attach_reader(reader.take(), t, audio_tx.clone()));
            }
        }
    }

    loop {
        tokio::select! {
            _ = cancel.notified() => break,
            ev = events.recv() => match ev {
                None | Some(RoomEvent::Disconnected { .. }) => break,
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(t), publication, participant })
                    if participant.kind() != ParticipantKind::Agent && publication.name() == CALL_AUDIO_TRACK =>
                {
                    // A republished track (pod reconnect) replaces the old reader.
                    reader = Some(attach_reader(reader.take(), t, audio_tx.clone()));
                }
                // Only the pod asks for speech (an agent can't make the bot talk).
                Some(RoomEvent::DataReceived { payload, topic, participant: Some(p), .. })
                    if topic.as_deref() == Some(SPEAK_TOPIC) && p.kind() != ParticipantKind::Agent =>
                {
                    match SpeakRequest::parse(&payload) {
                        Some(req) => {
                            if speak_tx.try_send(req).is_err() {
                                tracing::warn!("speak queue full; reply dropped");
                            }
                        }
                        None => tracing::warn!(from = %p.identity(), "bad speak payload"),
                    }
                }
                _ => {}
            },
        }
    }

    // Teardown: stop audio, flush the speech in progress, send the last segments.
    if let Some(h) = reader.take() {
        h.abort();
    }
    drop(audio_tx);
    let _ = tokio::task::spawn_blocking(move || stt_thread.join()).await;
    let sent = outputs_task.await.unwrap_or_default();
    tracing::info!(room = %job.room_name, sent, "meeting transcription ended");
    drop(speak_tx);
    speaker.abort();
    drop(frames_tx);
    playout.abort();
    let _ = room.close().await;
    Ok(())
}

/// One audio reader at a time: a newer `call-audio` subscription replaces the old.
fn attach_reader(old: Option<tokio::task::JoinHandle<()>>, track: RemoteAudioTrack, tx: mpsc::Sender<AudioMsg>) -> tokio::task::JoinHandle<()> {
    if let Some(h) = old {
        h.abort();
    }
    tokio::spawn(read_audio(track, tx))
}

/// Track audio → 16 kHz mono f32 chunks stamped with wall time.
async fn read_audio(track: RemoteAudioTrack, tx: mpsc::Sender<AudioMsg>) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), SAMPLE_RATE, 1);
    let mut resampler: Option<Resampler> = None;
    while let Some(frame) = stream.next().await {
        let mono = interleaved_to_mono(&frame.data, frame.num_channels);
        let pcm = if frame.sample_rate == SAMPLE_RATE as u32 {
            mono
        } else {
            resampler.get_or_insert_with(|| Resampler::new(frame.sample_rate, SAMPLE_RATE as u32)).process(&mono)
        };
        let dur_ms = pcm.len() as i64 * 1000 / SAMPLE_RATE as i64;
        let data = pcm.iter().map(|s| *s as f32 / 32768.0).collect();
        match tx.try_send(AudioMsg::Samples { data, epoch_ms: now_ms() - dur_ms }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => tracing::warn!("transcription is behind; dropped 10 ms of audio"),
            Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
    }
}

/// Audio later than this past where the sample count says it belongs means the
/// track paused (muted, reconnect): start a new stream so times stay true.
const GAP_MS: i64 = 500;

fn transcribe_loop(
    engine: Arc<SttEngine>,
    model: crate::stt::SttModel,
    session_id: String,
    mut rx: mpsc::Receiver<AudioMsg>,
    out: mpsc::UnboundedSender<Output>,
) {
    let mut processor: Option<SegmentProcessor> = None;
    let mut stream: Option<Transcriber> = None;
    let emit = |v: Vec<Output>| v.into_iter().for_each(|o| { let _ = out.send(o); });
    while let Some(msg) = rx.blocking_recv() {
        match msg {
            AudioMsg::Samples { data, epoch_ms } => {
                if stream.as_ref().is_some_and(|t| epoch_ms - t.expected_epoch_ms() > GAP_MS) {
                    let (o, p) = stream.take().expect("checked").finish();
                    emit(o);
                    processor = Some(p);
                }
                if stream.is_none() {
                    // Offsets count from the first audio of the track, for the whole job.
                    let p = processor.take().unwrap_or_else(|| SegmentProcessor::new(session_id.clone(), epoch_ms));
                    match Transcriber::new(&engine, model, epoch_ms, p) {
                        Ok(t) => stream = Some(t),
                        Err(e) => {
                            tracing::error!("meeting STT unavailable: {e}");
                            return;
                        }
                    }
                }
                if let Some(t) = stream.as_mut() {
                    emit(t.feed(&data));
                }
            }
        }
    }
    if let Some(t) = stream.take() {
        emit(t.finish().0);
    }
}

/// Reply frames → the reply track, paced by the source's 100 ms buffer.
async fn playout(source: NativeAudioSource, mut rx: mpsc::UnboundedReceiver<Vec<i16>>) {
    while let Some(samples) = rx.recv().await {
        let frame = AudioFrame {
            samples_per_channel: samples.len() as u32,
            data: Cow::Owned(samples),
            sample_rate: TRACK_RATE,
            num_channels: 1,
        };
        if let Err(e) = source.capture_frame(&frame).await {
            tracing::warn!("reply capture_frame: {e}");
        }
    }
}
