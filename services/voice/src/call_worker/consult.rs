//! LiveKit side of warm transfer and recording (`call-worker` feature): the
//! consult room, the SIP dial-out into it, the move into the caller's room, and
//! the Egress recorder. The state machines are in [`transfer`](super::transfer)
//! and [`recording`](super::recording); both are tested there with fakes.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_api::services::egress::{EgressClient, EgressOutput, RoomCompositeOptions};
use livekit_api::services::room::RoomClient;
use livekit_api::services::sip::{CreateSIPParticipantOptions, SIPClient, TransferSIPParticipantOptions};
use livekit_protocol as proto;
use tokio::sync::{mpsc, Notify};

use super::audio::TRACK_RATE;
use super::controls::{dtmf_code, transfer_uri};
use super::recording::{RecordingConfig, Recorder};
use super::transfer::{Accept, BridgeMethod, ConsultDriver, DialError};

/// Identity of the person the owner chose as the transfer target.
const TARGET_IDENTITY: &str = "transfer-target";
const BOT_IDENTITY: &str = "allternit-voice-consult";

/// Briefing audio on its way to the consult room (the call loop pushes, the
/// consult leg plays).
#[derive(Default)]
pub struct ConsultBus {
    queue: Mutex<VecDeque<Vec<i16>>>,
    wake: Notify,
}

impl ConsultBus {
    pub fn push(&self, frame: Vec<i16>) {
        self.queue.lock().unwrap().push_back(frame);
        self.wake.notify_one();
    }

    pub fn clear(&self) {
        self.queue.lock().unwrap().clear();
    }

    async fn next(&self) -> Vec<i16> {
        loop {
            if let Some(f) = self.queue.lock().unwrap().pop_front() {
                return f;
            }
            self.wake.notified().await;
        }
    }
}

/// Server-API access shared by the consult legs of one worker.
#[derive(Clone)]
pub struct LiveKitAccess {
    pub ws_url: String,
    pub api_host: String,
    pub key: String,
    pub secret: String,
}

impl LiveKitAccess {
    fn rooms(&self) -> RoomClient {
        RoomClient::with_api_key(&self.api_host, &self.key, &self.secret)
    }
    fn sip(&self) -> SIPClient {
        SIPClient::with_api_key(&self.api_host, &self.key, &self.secret)
    }
}

/// Whether a SIP dial-out error means "nobody picked up" rather than a fault.
fn is_no_answer(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    ["486", "480", "408", "603", "busy", "no answer", "timeout", "timed out", "decline", "unavailable"]
        .iter()
        .any(|m| e.contains(m))
}

struct Leg {
    room: Option<Arc<Room>>,
    events: Option<mpsc::UnboundedReceiver<RoomEvent>>,
    playout: Option<tokio::task::JoinHandle<()>>,
}

/// One consult leg: a fresh room, the target dialed into it by the outbound trunk.
pub struct SipConsult {
    lk: LiveKitAccess,
    trunk: String,
    target: String,
    consent_ref: String,
    call_room: String,
    caller: String,
    consult_room: String,
    bus: Arc<ConsultBus>,
    leg: tokio::sync::Mutex<Leg>,
}

impl SipConsult {
    pub fn new(
        lk: LiveKitAccess,
        trunk: String,
        target: String,
        consent_ref: String,
        call_room: String,
        caller: String,
        bus: Arc<ConsultBus>,
    ) -> Self {
        let consult_room = format!("consult-{}-{:08x}", call_room.trim_start_matches("call-"), rand_suffix());
        Self {
            lk,
            trunk,
            target,
            consent_ref,
            call_room,
            caller,
            consult_room,
            bus,
            leg: tokio::sync::Mutex::new(Leg { room: None, events: None, playout: None }),
        }
    }

    async fn join(&self) -> Result<(), String> {
        let token = AccessToken::with_api_key(&self.lk.key, &self.lk.secret)
            .with_identity(BOT_IDENTITY)
            .with_ttl(Duration::from_secs(900))
            .with_grants(VideoGrants {
                room_join: true,
                room: self.consult_room.clone(),
                can_publish: true,
                can_subscribe: true,
                ..Default::default()
            })
            .to_jwt()
            .map_err(|e| format!("consult token: {e}"))?;
        let (room, events) =
            Room::connect(&self.lk.ws_url, &token, RoomOptions::default()).await.map_err(|e| format!("join consult room: {e}"))?;
        let source = NativeAudioSource::new(AudioSourceOptions::default(), TRACK_RATE, 1, 100);
        let track = LocalAudioTrack::create_audio_track("allternit-voice", RtcAudioSource::Native(source.clone()));
        room.local_participant()
            .publish_track(LocalTrack::Audio(track), TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() })
            .await
            .map_err(|e| format!("publish consult track: {e}"))?;
        let bus = self.bus.clone();
        let playout = tokio::spawn(async move {
            loop {
                let f = bus.next().await;
                let frame = AudioFrame { samples_per_channel: f.len() as u32, data: Cow::Owned(f), sample_rate: TRACK_RATE, num_channels: 1 };
                if source.capture_frame(&frame).await.is_err() {
                    return;
                }
            }
        });
        let mut leg = self.leg.lock().await;
        leg.room = Some(Arc::new(room));
        leg.events = Some(events);
        leg.playout = Some(playout);
        Ok(())
    }
}

fn rand_suffix() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    n ^ (std::process::id().rotate_left(16))
}

impl ConsultDriver for SipConsult {
    fn dial(&self, ring_timeout: Duration) -> BoxFuture<'_, Result<(), DialError>> {
        Box::pin(async move {
            self.join().await.map_err(DialError::Failed)?;
            let opts = CreateSIPParticipantOptions {
                participant_identity: TARGET_IDENTITY.into(),
                participant_name: Some("Transfer target".into()),
                participant_attributes: Some([("consentRef".to_string(), self.consent_ref.clone())].into()),
                sip_number: None,
                wait_until_answered: Some(true),
                play_dialtone: Some(false),
                ringing_timeout: Some(ring_timeout),
                ..Default::default()
            };
            let call = self.lk.sip().create_sip_participant(
                self.trunk.clone(),
                self.target.clone(),
                self.consult_room.clone(),
                opts,
                None,
            );
            match tokio::time::timeout(ring_timeout + Duration::from_secs(5), call).await {
                Err(_) => Err(DialError::NoAnswer),
                Ok(Ok(_)) => Ok(()),
                Ok(Err(e)) => {
                    let msg = e.to_string();
                    if is_no_answer(&msg) {
                        Err(DialError::NoAnswer)
                    } else {
                        Err(DialError::Failed(msg))
                    }
                }
            }
        })
    }

    fn await_accept(&self, timeout: Duration) -> BoxFuture<'_, Accept> {
        Box::pin(async move {
            let Some(mut events) = self.leg.lock().await.events.take() else { return Accept::Declined };
            let wait = async {
                while let Some(ev) = events.recv().await {
                    match ev {
                        RoomEvent::SipDTMFReceived { code, digit, .. } => {
                            let d = digit.filter(|d| !d.is_empty()).or_else(|| {
                                "0123456789*#ABCD".chars().find(|c| dtmf_code(*c) == Some(code)).map(String::from)
                            });
                            return if d.as_deref() == Some("1") { Accept::Accepted } else { Accept::Declined };
                        }
                        RoomEvent::ParticipantDisconnected(p) if p.identity().to_string() == TARGET_IDENTITY => {
                            return Accept::Declined
                        }
                        RoomEvent::Disconnected { .. } => return Accept::Declined,
                        _ => {}
                    }
                }
                Accept::Declined
            };
            tokio::time::timeout(timeout, wait).await.unwrap_or(Accept::TimedOut)
        })
    }

    fn bridge(&self) -> BoxFuture<'_, Result<BridgeMethod, String>> {
        Box::pin(async move {
            match self.lk.rooms().move_participant(&self.consult_room, TARGET_IDENTITY, &self.call_room).await {
                Ok(()) => Ok(BridgeMethod::Moved),
                Err(move_err) => {
                    tracing::warn!("MoveParticipant failed ({move_err}); falling back to SIP REFER");
                    self.lk
                        .sip()
                        .transfer_sip_participant(
                            self.call_room.clone(),
                            self.caller.clone(),
                            transfer_uri(&self.target),
                            TransferSIPParticipantOptions { play_dialtone: Some(false), ..Default::default() },
                        )
                        .await
                        .map(|()| BridgeMethod::Refer)
                        .map_err(|e| format!("move failed ({move_err}); refer failed ({e})"))
                }
            }
        })
    }

    fn cleanup(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let (room, playout) = {
                let mut leg = self.leg.lock().await;
                (leg.room.take(), leg.playout.take())
            };
            if let Some(p) = playout {
                p.abort();
            }
            self.bus.clear();
            if let Some(r) = room {
                let _ = r.close().await;
            }
            let _ = self.lk.rooms().delete_room(&self.consult_room).await;
        })
    }
}

/// LiveKit Egress: an audio-only room composite into the configured bucket.
pub struct EgressRecorder {
    client: EgressClient,
    bucket: RecordingConfig,
}

impl EgressRecorder {
    pub fn new(lk: &LiveKitAccess, bucket: RecordingConfig) -> Self {
        Self { client: EgressClient::with_api_key(&lk.api_host, &lk.key, &lk.secret), bucket }
    }
}

impl Recorder for EgressRecorder {
    fn start(&self, room: &str, key: &str) -> BoxFuture<'_, Result<String, String>> {
        let (room, key) = (room.to_string(), key.to_string());
        Box::pin(async move {
            let b = &self.bucket;
            let file = proto::EncodedFileOutput {
                file_type: proto::EncodedFileType::Ogg as i32,
                filepath: key,
                output: Some(proto::encoded_file_output::Output::S3(proto::S3Upload {
                    access_key: b.access_key.clone(),
                    secret: b.secret.clone(),
                    region: b.region.clone(),
                    endpoint: b.endpoint.clone(),
                    bucket: b.bucket.clone(),
                    force_path_style: true,
                    ..Default::default()
                })),
                ..Default::default()
            };
            let opts = RoomCompositeOptions { audio_only: true, ..Default::default() };
            self.client
                .start_room_composite_egress(&room, vec![EgressOutput::File(file)], opts)
                .await
                .map(|i| i.egress_id)
                .map_err(|e| e.to_string())
        })
    }

    fn stop(&self, egress_id: &str) -> BoxFuture<'_, Result<(), String>> {
        let id = egress_id.to_string();
        Box::pin(async move { self.client.stop_egress(&id).await.map(|_| ()).map_err(|e| e.to_string()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_answer_errors_are_told_from_faults() {
        assert!(is_no_answer("SIP status: 486 Busy Here"));
        assert!(is_no_answer("request timed out"));
        assert!(!is_no_answer("trunk not found"));
    }

    #[tokio::test]
    async fn bus_delivers_in_order_and_clears() {
        let bus = ConsultBus::default();
        bus.push(vec![1]);
        bus.push(vec![2]);
        assert_eq!(bus.next().await, vec![1]);
        bus.clear();
        assert!(tokio::time::timeout(Duration::from_millis(20), bus.next()).await.is_err());
    }
}
