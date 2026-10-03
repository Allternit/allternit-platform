//! One phone call: the state machine between the room (LiveKit), the Voice
//! Session core, the brain (cloud-api relay) and the event queue.
//!
//! The room side is two channels ([`RoomCommand`] out, [`RoomInput`] in) so this
//! file has no LiveKit types and is tested end to end with fakes; `room.rs`
//! fills them from the real room.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::audio::{to_pcm16le, Framer, Pcm16Decoder, Resampler, TRACK_RATE};
use super::brain::{CallBrain, RELAY_UNAVAILABLE_LINE};
use super::cloud_client::{BotConfig, TurnRequest};
use super::controls::{parse_control, CallState, Control, Target};
use super::disclosure;
use super::events::{CallEvent, EventQueue, Speaker, TransferMode};
use super::session_adapter::{CoreCommand, CoreEvent, CoreHandle};
use super::voicemail::VoicemailDetector;

/// What the call asks the room to do.
#[derive(Debug)]
pub enum RoomCommand {
    /// One 10 ms frame of bot speech, mono PCM16 at [`TRACK_RATE`].
    PublishFrame(Vec<i16>),
    /// Drop bot audio already queued in the track source (barge-in, mute, hold).
    ClearAudio,
    /// Send DTMF to the caller.
    SendDtmf(String),
    /// Cold transfer via SIP REFER; the room answers with [`RoomInput::TransferResult`].
    Transfer { to: String },
    /// End the call (remove the SIP participant, delete the room).
    Hangup,
}

/// What the room tells the call.
#[derive(Debug)]
pub enum RoomInput {
    /// Caller audio, mono PCM16 at 16 kHz.
    CallerAudio(Vec<i16>),
    /// DTMF the caller pressed.
    Dtmf(String),
    /// Raw `allternit.call.control` payload (server-sent only; room.rs drops
    /// packets from participants).
    Control(Vec<u8>),
    TransferResult {
        to: String,
        ok: bool,
        reason: Option<String>,
    },
    /// The SIP participant left (caller hung up, or a transfer completed).
    CallerLeft,
    /// The worker lost the room.
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct CallContext {
    pub call_id: String,
    /// The caller's number (inbound) / the called party (outbound).
    pub remote: String,
    /// The bot's number.
    pub local: String,
    pub direction: String,
    pub number_id: String,
    pub bot: BotConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutcome {
    pub reason: String,
    pub duration_sec: u64,
}

/// How long a turn waits for the first reply byte before the fallback line.
pub const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(20);

enum BrainMsg {
    Delta(u64, String),
    Done(u64),
    Failed(u64, String),
}

struct Speech {
    out_rate: u32,
    decoder: Pcm16Decoder,
    resampler: Resampler,
    framer: Framer,
}

impl Speech {
    fn new(out_rate: u32) -> Self {
        Self {
            out_rate,
            decoder: Pcm16Decoder::default(),
            resampler: Resampler::new(out_rate, TRACK_RATE),
            framer: Framer::new(TRACK_RATE, 10),
        }
    }

    fn reset(&mut self) {
        self.decoder.reset();
        self.resampler = Resampler::new(self.out_rate, TRACK_RATE);
        self.framer.clear();
    }

    fn frames(&mut self, bytes: &[u8]) -> Vec<Vec<i16>> {
        let s = self.decoder.decode(bytes);
        let s = self.resampler.process(&s);
        self.framer.push(&s)
    }
}

struct Call {
    ctx: CallContext,
    started: Instant,
    state: CallState,
    events: EventQueue,
    core_tx: mpsc::Sender<CoreCommand>,
    room_tx: mpsc::Sender<RoomCommand>,
    brain: Arc<dyn CallBrain>,
    brain_tx: mpsc::UnboundedSender<BrainMsg>,
    brain_task: Option<JoinHandle<()>>,
    /// Turn whose reply is current; replies of older turns are ignored.
    turn: u64,
    /// Utterance id → text sent to the core, for the bot transcript.
    said: HashMap<String, String>,
    /// Utterance id of the current turn's reply, once text started.
    reply_id: Option<String>,
    reply_had_text: bool,
    speech: Speech,
    /// After a cancel, audio the core already had in flight is dropped until
    /// the next `speak.started`.
    drop_audio: bool,
    voicemail: Box<dyn VoicemailDetector>,
    transferred: bool,
    end_reason: Option<String>,
}

/// Run a configured call to its end. Returns the outcome and a handle that
/// resolves once every event (through `call.ended`) is delivered.
pub async fn run_call(
    ctx: CallContext,
    core: CoreHandle,
    brain: Arc<dyn CallBrain>,
    events: EventQueue,
    room_tx: mpsc::Sender<RoomCommand>,
    mut room_rx: mpsc::Receiver<RoomInput>,
    voicemail: Box<dyn VoicemailDetector>,
) -> (CallOutcome, JoinHandle<()>) {
    let CoreHandle {
        tx: core_tx,
        rx: mut core_rx,
        output_sample_rate,
    } = core;
    let (brain_tx, mut brain_rx) = mpsc::unbounded_channel();
    let mut call = Call {
        started: Instant::now(),
        state: CallState::default(),
        events,
        core_tx,
        room_tx,
        brain,
        brain_tx,
        brain_task: None,
        turn: 0,
        said: HashMap::new(),
        reply_id: None,
        reply_had_text: false,
        speech: Speech::new(output_sample_rate),
        drop_audio: false,
        voicemail,
        transferred: false,
        end_reason: None,
        ctx,
    };

    // Speak first. The opening comes from the cached bot config only; nothing
    // here waits on the user's runtime.
    let opening = disclosure::opening(
        call.ctx.bot.display_name().as_deref(),
        call.ctx.bot.recording,
        call.ctx.bot.greeting.as_deref(),
    );
    call.say("u-0", &opening).await;
    call.events.emit(CallEvent::Started {
        direction: call.ctx.direction.clone(),
        from: if call.ctx.direction == "outbound" {
            call.ctx.local.clone()
        } else {
            call.ctx.remote.clone()
        },
        to: if call.ctx.direction == "outbound" {
            call.ctx.remote.clone()
        } else {
            call.ctx.local.clone()
        },
        number_id: call.ctx.number_id.clone(),
    });

    while call.end_reason.is_none() {
        tokio::select! {
            input = room_rx.recv() => match input {
                Some(i) => call.on_room(i).await,
                None => call.end("room_closed"),
            },
            ev = core_rx.recv() => match ev {
                Some(e) => call.on_core(e).await,
                None => call.on_core(CoreEvent::Closed).await,
            },
            Some(m) = brain_rx.recv() => call.on_brain(m).await,
        }
    }

    call.cancel_reply();
    let _ = call.core_tx.send(CoreCommand::End).await;
    let reason = call.end_reason.take().unwrap_or_default();
    let duration_sec = call.started.elapsed().as_secs();
    call.events.emit(CallEvent::Ended {
        duration_sec,
        reason: reason.clone(),
        recording_ref: None,
    });
    (
        CallOutcome {
            reason,
            duration_sec,
        },
        call.events.close(),
    )
}

impl Call {
    fn end(&mut self, reason: &str) {
        if self.end_reason.is_none() {
            self.end_reason = Some(reason.to_string());
        }
    }

    async fn say(&mut self, id: &str, text: &str) {
        self.said.entry(id.to_string()).or_default().push_str(text);
        let _ = self
            .core_tx
            .send(CoreCommand::SpeakDelta {
                id: id.into(),
                text: text.into(),
            })
            .await;
        let _ = self
            .core_tx
            .send(CoreCommand::SpeakDone { id: id.into() })
            .await;
    }

    fn bot_transcript(&mut self, id: &str) {
        if let Some(text) = self.said.remove(id).filter(|t| !t.trim().is_empty()) {
            self.events.emit(CallEvent::TranscriptDelta {
                speaker: Speaker::Bot,
                text,
                is_final: true,
                segment_id: id.to_string(),
            });
        }
    }

    /// Abort the in-flight reply (barge-in, hold, takeover, hang up).
    fn cancel_reply(&mut self) {
        if let Some(t) = self.brain_task.take() {
            t.abort();
        }
        self.turn += 1;
        self.reply_id = None;
        self.reply_had_text = false;
    }

    /// Stop the bot's voice now: core stops generating, room drops queued audio.
    async fn silence(&mut self) {
        let _ = self
            .core_tx
            .send(CoreCommand::SpeakCancel { id: None })
            .await;
        let _ = self.room_tx.send(RoomCommand::ClearAudio).await;
        self.speech.reset();
        self.drop_audio = true;
    }

    async fn on_room(&mut self, input: RoomInput) {
        match input {
            RoomInput::CallerAudio(samples) => {
                if !self.state.muted_caller && !self.state.held {
                    // Never block the call loop on the core; drop audio if it lags.
                    if self
                        .core_tx
                        .try_send(CoreCommand::Audio(to_pcm16le(&samples)))
                        .is_err()
                    {
                        tracing::debug!(call_id = %self.ctx.call_id, "core input full, dropped caller frame");
                    }
                }
            }
            RoomInput::Dtmf(digits) => {
                let from = self.ctx.remote.clone();
                self.events.emit(CallEvent::Dtmf { digits, from });
            }
            RoomInput::Control(bytes) => match parse_control(&bytes) {
                Ok(c) => self.apply(c).await,
                Err(e) => {
                    tracing::warn!(call_id = %self.ctx.call_id, "ignored call control: {e}");
                    // Ack with the unchanged state so the UI's pending action settles.
                    self.events.emit(self.state.state_event());
                }
            },
            RoomInput::TransferResult { to, ok, reason } => {
                self.events.emit(CallEvent::Transferred {
                    to,
                    mode: TransferMode::Cold,
                    ok,
                    reason,
                });
                if ok {
                    self.transferred = true;
                }
            }
            RoomInput::CallerLeft => self.end(if self.transferred {
                "transferred"
            } else {
                "caller_hangup"
            }),
            RoomInput::Disconnected => self.end("room_closed"),
        }
    }

    async fn apply(&mut self, c: Control) {
        match c {
            Control::Hangup => {
                let _ = self.room_tx.send(RoomCommand::Hangup).await;
                self.end("hangup_control");
                return;
            }
            Control::Mute(Target::Bot) => {
                self.state.muted_bot = true;
                self.silence().await;
            }
            Control::Unmute(Target::Bot) => self.state.muted_bot = false,
            Control::Mute(Target::Caller) => {
                self.state.muted_caller = true;
                let _ = self.core_tx.send(CoreCommand::MicMute).await;
            }
            Control::Unmute(Target::Caller) => {
                self.state.muted_caller = false;
                if !self.state.held {
                    let _ = self.core_tx.send(CoreCommand::MicUnmute).await;
                }
            }
            Control::Hold => {
                self.state.held = true;
                self.cancel_reply();
                self.silence().await;
                let _ = self.core_tx.send(CoreCommand::MicMute).await;
            }
            Control::Resume => {
                self.state.held = false;
                if !self.state.muted_caller {
                    let _ = self.core_tx.send(CoreCommand::MicUnmute).await;
                }
            }
            Control::Dtmf(digits) => {
                let _ = self
                    .room_tx
                    .send(RoomCommand::SendDtmf(digits.clone()))
                    .await;
                let from = self.ctx.local.clone();
                self.events.emit(CallEvent::Dtmf { digits, from });
                return;
            }
            Control::Transfer {
                to,
                mode: TransferMode::Cold,
            } => {
                self.cancel_reply();
                self.silence().await;
                let _ = self.room_tx.send(RoomCommand::Transfer { to }).await;
                return; // acked by call.transferred when the room answers
            }
            Control::Transfer {
                to,
                mode: TransferMode::Warm,
            } => {
                self.events.emit(CallEvent::Transferred {
                    to,
                    mode: TransferMode::Warm,
                    ok: false,
                    reason: Some("warm transfer is not supported yet".into()),
                });
                return;
            }
            Control::Takeover { by } => {
                self.state.takeover_by = Some(by.clone());
                self.cancel_reply();
                self.silence().await;
                self.events.emit(CallEvent::Takeover { by, active: true });
                return;
            }
            Control::Release { by } => {
                self.state.takeover_by = None;
                self.events.emit(CallEvent::Takeover { by, active: false });
                return;
            }
            Control::Listen => {}
        }
        self.events.emit(self.state.state_event());
    }

    async fn on_core(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::SpeechStarted => self.state.speaker = Some(Speaker::Caller),
            CoreEvent::SpeechStopped => {}
            CoreEvent::TranscriptDelta { segment_id, text } => {
                self.events.emit(CallEvent::TranscriptDelta {
                    speaker: Speaker::Caller,
                    text,
                    is_final: false,
                    segment_id,
                });
            }
            CoreEvent::TranscriptFinal { segment_id, text } => {
                if self.ctx.direction == "outbound" {
                    let at = self.started.elapsed().as_millis() as u64;
                    if let Some(action) = self.voicemail.observe_final(&text, at) {
                        self.events.emit(CallEvent::VoicemailDetected {
                            action: action.as_str().into(),
                        });
                    }
                }
                self.events.emit(CallEvent::TranscriptDelta {
                    speaker: Speaker::Caller,
                    text,
                    is_final: true,
                    segment_id,
                });
            }
            CoreEvent::TurnEnded { text, confidence } => {
                self.state.speaker = None;
                if text.trim().is_empty() || !self.state.bot_active() {
                    return; // during takeover/hold the bot only transcribes
                }
                self.start_turn(text, confidence);
            }
            CoreEvent::SpeakStarted { id: _ } => {
                self.speech.reset();
                self.drop_audio = false;
                if self.state.bot_active() && !self.state.muted_bot {
                    self.state.speaker = Some(Speaker::Bot);
                }
            }
            CoreEvent::SpeakAudio(bytes) => {
                if self.drop_audio || self.state.muted_bot || !self.state.bot_active() {
                    return;
                }
                for f in self.speech.frames(&bytes) {
                    let _ = self.room_tx.send(RoomCommand::PublishFrame(f)).await;
                }
            }
            CoreEvent::SpeakEnded { id } => {
                if let Some(f) = self.speech.framer.flush() {
                    if !self.drop_audio && !self.state.muted_bot && self.state.bot_active() {
                        let _ = self.room_tx.send(RoomCommand::PublishFrame(f)).await;
                    }
                }
                if self.state.speaker == Some(Speaker::Bot) {
                    self.state.speaker = None;
                }
                self.bot_transcript(&id);
            }
            CoreEvent::SpeakInterrupted { id } => {
                // Barge-in: the caller talked over the bot. Stop publishing at
                // once and abort the in-flight reply.
                let _ = self.room_tx.send(RoomCommand::ClearAudio).await;
                self.speech.reset();
                self.drop_audio = true;
                if self.reply_id.as_deref() == Some(id.as_str()) {
                    self.cancel_reply();
                }
                self.state.speaker = Some(Speaker::Caller);
                self.bot_transcript(&id);
            }
            CoreEvent::Error {
                code,
                message,
                fatal,
            } => {
                tracing::error!(call_id = %self.ctx.call_id, %code, fatal, "voice core error: {message}");
                if fatal {
                    self.end("voice_engine_error");
                }
            }
            CoreEvent::Closed => {
                tracing::error!(call_id = %self.ctx.call_id, "voice core closed mid-call");
                let _ = self.room_tx.send(RoomCommand::Hangup).await;
                self.end("voice_engine_error");
            }
        }
    }

    fn start_turn(&mut self, text: String, confidence: f32) {
        self.cancel_reply();
        let turn = self.turn;
        let req = TurnRequest {
            call_id: self.ctx.call_id.clone(),
            turn_id: format!("turn:{}:{}", self.ctx.call_id, turn),
            text,
            confidence,
        };
        let brain = self.brain.clone();
        let tx = self.brain_tx.clone();
        self.brain_task = Some(tokio::spawn(async move {
            match brain.reply(req).await {
                Ok(mut s) => {
                    while let Some(item) = s.next().await {
                        match item {
                            Ok(t) => {
                                let _ = tx.send(BrainMsg::Delta(turn, t));
                            }
                            Err(e) => {
                                let _ = tx.send(BrainMsg::Failed(turn, e.0));
                                return;
                            }
                        }
                    }
                    let _ = tx.send(BrainMsg::Done(turn));
                }
                Err(e) => {
                    let _ = tx.send(BrainMsg::Failed(turn, e.0));
                }
            }
        }));
    }

    async fn on_brain(&mut self, m: BrainMsg) {
        let (turn, msg) = match m {
            BrainMsg::Delta(t, s) => (t, Ok(Some(s))),
            BrainMsg::Done(t) => (t, Ok(None)),
            BrainMsg::Failed(t, e) => (t, Err(e)),
        };
        if turn != self.turn || !self.state.bot_active() {
            return;
        }
        let id = self
            .reply_id
            .get_or_insert_with(|| format!("u-{turn}"))
            .clone();
        match msg {
            Ok(Some(text)) => {
                if text.is_empty() {
                    return;
                }
                self.reply_had_text = true;
                self.said.entry(id.clone()).or_default().push_str(&text);
                let _ = self
                    .core_tx
                    .send(CoreCommand::SpeakDelta { id, text })
                    .await;
            }
            Ok(None) => {
                if !self.reply_had_text {
                    tracing::warn!(call_id = %self.ctx.call_id, "relay returned an empty reply");
                    self.say(&id, RELAY_UNAVAILABLE_LINE).await;
                } else {
                    let _ = self.core_tx.send(CoreCommand::SpeakDone { id }).await;
                }
                self.brain_task = None;
            }
            Err(e) => {
                tracing::error!(call_id = %self.ctx.call_id, "turn relay failed: {e}");
                if self.reply_had_text {
                    let _ = self.core_tx.send(CoreCommand::SpeakDone { id }).await;
                } else {
                    self.say(&id, RELAY_UNAVAILABLE_LINE).await;
                }
                self.brain_task = None;
            }
        }
    }
}

/// When call start fails (cloud-api down, unknown number): still disclose, say
/// the honest line, then hang up. No events, since there is no `callId`.
pub async fn run_unconfigured_call(
    core: CoreHandle,
    room_tx: mpsc::Sender<RoomCommand>,
    mut room_rx: mpsc::Receiver<RoomInput>,
    max_wait: Duration,
) {
    let CoreHandle {
        tx,
        rx: mut core_rx,
        output_sample_rate,
    } = core;
    let text = format!(
        "{} {}",
        disclosure::disclosure(None, false),
        RELAY_UNAVAILABLE_LINE
    );
    let _ = tx
        .send(CoreCommand::SpeakDelta {
            id: "u-0".into(),
            text,
        })
        .await;
    let _ = tx.send(CoreCommand::SpeakDone { id: "u-0".into() }).await;
    let mut speech = Speech::new(output_sample_rate);
    let deadline = tokio::time::sleep(max_wait);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            ev = core_rx.recv() => match ev {
                Some(CoreEvent::SpeakAudio(b)) => {
                    for f in speech.frames(&b) {
                        let _ = room_tx.send(RoomCommand::PublishFrame(f)).await;
                    }
                }
                Some(CoreEvent::SpeakEnded { .. }) => {
                    if let Some(f) = speech.framer.flush() {
                        let _ = room_tx.send(RoomCommand::PublishFrame(f)).await;
                    }
                    // Let the last frames play out before hanging up.
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    break;
                }
                Some(CoreEvent::Closed) | None => break,
                Some(_) => {}
            },
            input = room_rx.recv() => match input {
                Some(RoomInput::CallerLeft) | Some(RoomInput::Disconnected) | None => return,
                Some(_) => {}
            },
        }
    }
    let _ = tx.send(CoreCommand::End).await;
    let _ = room_tx.send(RoomCommand::Hangup).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_worker::brain::ScriptedBrain;
    use crate::call_worker::cloud_client::{CloudError, EventEnvelope};
    use crate::call_worker::events::{Backoff, EventTransport};
    use crate::call_worker::voicemail::NoVoicemailDetection;
    use futures::future::BoxFuture;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<EventEnvelope>>);

    impl EventTransport for Recorder {
        fn post<'a>(
            &'a self,
            _id: &'a str,
            ev: &'a EventEnvelope,
        ) -> BoxFuture<'a, Result<(), CloudError>> {
            self.0.lock().unwrap().push(ev.clone());
            Box::pin(async { Ok(()) })
        }
    }

    struct Harness {
        core_cmds: mpsc::Receiver<CoreCommand>,
        core_events: mpsc::Sender<CoreEvent>,
        room_cmds: mpsc::Receiver<RoomCommand>,
        room_input: mpsc::Sender<RoomInput>,
        rec: Arc<Recorder>,
        task: JoinHandle<(CallOutcome, JoinHandle<()>)>,
        brain: Arc<ScriptedBrain>,
    }

    fn ctx(recording: bool) -> CallContext {
        CallContext {
            call_id: "c1".into(),
            remote: "+15551230000".into(),
            local: "+16512686010".into(),
            direction: "inbound".into(),
            number_id: "num_1".into(),
            bot: BotConfig {
                name: Some("Acme Plumbing".into()),
                greeting: Some("How can I help?".into()),
                recording,
                ..Default::default()
            },
        }
    }

    fn start(recording: bool, replies: Vec<Result<Vec<&str>, &str>>) -> Harness {
        let (core_tx, core_cmds) = mpsc::channel(256);
        let (core_events, core_rx) = mpsc::channel(256);
        let (room_tx, room_cmds) = mpsc::channel(4096);
        let (room_input, room_rx) = mpsc::channel(256);
        let rec = Arc::new(Recorder::default());
        let brain = Arc::new(ScriptedBrain::new(replies));
        let events = EventQueue::start("c1", rec.clone(), Backoff::default());
        let core = CoreHandle {
            tx: core_tx,
            rx: core_rx,
            output_sample_rate: 24_000,
        };
        let task = tokio::spawn(run_call(
            ctx(recording),
            core,
            brain.clone(),
            events,
            room_tx,
            room_rx,
            Box::new(NoVoicemailDetection),
        ));
        Harness {
            core_cmds,
            core_events,
            room_cmds,
            room_input,
            rec,
            task,
            brain,
        }
    }

    async fn next_cmd(h: &mut Harness) -> CoreCommand {
        tokio::time::timeout(Duration::from_secs(2), h.core_cmds.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn finish(h: Harness) -> (CallOutcome, Vec<EventEnvelope>) {
        let (outcome, drain) = h.task.await.unwrap();
        drain.await.unwrap();
        let evs = h.rec.0.lock().unwrap().clone();
        (outcome, evs)
    }

    #[tokio::test]
    async fn speaks_disclosure_first_then_runs_turns() {
        let mut h = start(false, vec![Ok(vec!["We open ", "at nine."])]);
        // First thing the core hears: the fixed disclosure, then the greeting.
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDelta {
                id: "u-0".into(),
                text: "Hi, you've reached Acme Plumbing. I'm an AI assistant, and this call isn't recorded. How can I help?".into()
            }
        );
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDone { id: "u-0".into() }
        );

        // Bot audio is resampled 24k → 48k and published in 10 ms frames.
        h.core_events
            .send(CoreEvent::SpeakStarted { id: "u-0".into() })
            .await
            .unwrap();
        h.core_events
            .send(CoreEvent::SpeakAudio(to_pcm16le(&[100i16; 480])))
            .await
            .unwrap();
        h.core_events
            .send(CoreEvent::SpeakEnded { id: "u-0".into() })
            .await
            .unwrap();
        let RoomCommand::PublishFrame(f) = h.room_cmds.recv().await.unwrap() else {
            panic!()
        };
        assert_eq!(f.len(), 480);

        // Caller audio reaches the core.
        h.room_input
            .send(RoomInput::CallerAudio(vec![1, 2]))
            .await
            .unwrap();
        assert_eq!(next_cmd(&mut h).await, CoreCommand::Audio(vec![1, 0, 2, 0]));

        // Caller turn → relay → reply streamed into the speak path.
        h.core_events
            .send(CoreEvent::TranscriptFinal {
                segment_id: "s1".into(),
                text: "When do you open?".into(),
            })
            .await
            .unwrap();
        h.core_events
            .send(CoreEvent::TurnEnded {
                text: "When do you open?".into(),
                confidence: 0.9,
            })
            .await
            .unwrap();
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDelta {
                id: "u-1".into(),
                text: "We open ".into()
            }
        );
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDelta {
                id: "u-1".into(),
                text: "at nine.".into()
            }
        );
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDone { id: "u-1".into() }
        );
        assert_eq!(h.brain.seen.lock().unwrap()[0].turn_id, "turn:c1:1");
        h.core_events
            .send(CoreEvent::SpeakEnded { id: "u-1".into() })
            .await
            .unwrap();
        wait_events(&h, 4).await;

        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "caller_hangup");
        let kinds: Vec<_> = evs.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "call.started",
                "call.transcript.delta",
                "call.transcript.delta",
                "call.transcript.delta",
                "call.ended"
            ]
        );
        assert_eq!(evs[1].payload["speaker"], "bot");
        assert!(evs[1].payload["text"]
            .as_str()
            .unwrap()
            .starts_with("Hi, you've reached"));
        assert_eq!(evs[2].payload["speaker"], "caller");
        assert_eq!(evs[3].payload["text"], "We open at nine.");
        assert_eq!(evs[4].payload["reason"], "caller_hangup");
        let seqs: Vec<_> = evs.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn relay_failure_speaks_the_honest_line() {
        let mut h = start(true, vec![Err("relay 503")]);
        let CoreCommand::SpeakDelta { text, .. } = next_cmd(&mut h).await else {
            panic!()
        };
        assert!(text.contains("this call may be recorded"));
        next_cmd(&mut h).await;
        h.core_events
            .send(CoreEvent::TurnEnded {
                text: "hello?".into(),
                confidence: 1.0,
            })
            .await
            .unwrap();
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDelta {
                id: "u-1".into(),
                text: RELAY_UNAVAILABLE_LINE.into()
            }
        );
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        finish(h).await;
    }

    #[tokio::test]
    async fn barge_in_clears_audio_and_controls_ack() {
        let mut h = start(false, vec![]);
        next_cmd(&mut h).await;
        next_cmd(&mut h).await;
        h.core_events
            .send(CoreEvent::SpeakStarted { id: "u-0".into() })
            .await
            .unwrap();
        h.core_events
            .send(CoreEvent::SpeakInterrupted { id: "u-0".into() })
            .await
            .unwrap();
        assert!(matches!(
            h.room_cmds.recv().await.unwrap(),
            RoomCommand::ClearAudio
        ));

        // Takeover: bot goes quiet, turns don't reach the brain, transcripts continue.
        h.room_input
            .send(RoomInput::Control(
                br#"{"action":"takeover","by":"user_1"}"#.to_vec(),
            ))
            .await
            .unwrap();
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakCancel { id: None }
        );
        h.core_events
            .send(CoreEvent::TurnEnded {
                text: "are you there".into(),
                confidence: 1.0,
            })
            .await
            .unwrap();
        h.core_events
            .send(CoreEvent::TranscriptDelta {
                segment_id: "s2".into(),
                text: "hel".into(),
            })
            .await
            .unwrap();
        // Core events and room input arrive on separate channels; wait until the
        // transcript is recorded so the expected order below is deterministic.
        wait_events(&h, 4).await;
        h.room_input
            .send(RoomInput::Control(
                br#"{"action":"release","by":"user_1"}"#.to_vec(),
            ))
            .await
            .unwrap();
        h.room_input
            .send(RoomInput::Control(
                br#"{"action":"mute","target":"caller"}"#.to_vec(),
            ))
            .await
            .unwrap();
        h.room_input
            .send(RoomInput::Control(
                br#"{"action":"dtmf","digits":"42"}"#.to_vec(),
            ))
            .await
            .unwrap();
        h.room_input
            .send(RoomInput::Dtmf("9".into()))
            .await
            .unwrap();
        h.room_input
            .send(RoomInput::Control(
                br#"{"action":"transfer","to":"+15105550100"}"#.to_vec(),
            ))
            .await
            .unwrap();
        h.room_input
            .send(RoomInput::TransferResult {
                to: "+15105550100".into(),
                ok: true,
                reason: None,
            })
            .await
            .unwrap();
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "transferred");
        let kinds: Vec<_> = evs.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "call.started",
                "call.transcript.delta",
                "call.takeover",
                "call.transcript.delta",
                "call.takeover",
                "call.state.changed",
                "call.dtmf",
                "call.dtmf",
                "call.transferred",
                "call.ended"
            ]
        );
        // The interrupted opening is still logged as what the bot said.
        assert_eq!(evs[1].payload["speaker"], "bot");
        assert_eq!(evs[2].payload["active"], true);
        assert_eq!(evs[3].payload["final"], false);
        assert_eq!(evs[4].payload["active"], false);
        assert_eq!(evs[5].payload["mutedCaller"], true);
        assert_eq!(evs[6].payload["from"], "+16512686010");
        assert_eq!(evs[7].payload["from"], "+15551230000");
        assert_eq!(evs[8].payload["ok"], true);
        assert_eq!(evs[7].idempotency_key, "call:c1:call.dtmf:2");
        assert!(h_brain_unused(&evs));
    }

    async fn wait_events(h: &Harness, n: usize) {
        for _ in 0..200 {
            if h.rec.0.lock().unwrap().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("expected {n} events");
    }

    fn h_brain_unused(evs: &[EventEnvelope]) -> bool {
        !evs.iter()
            .any(|e| e.payload["speaker"] == "bot" && e.payload["segmentId"] != "u-0")
    }

    #[tokio::test]
    async fn hangup_control_ends_call() {
        let mut h = start(false, vec![]);
        next_cmd(&mut h).await;
        h.room_input
            .send(RoomInput::Control(br#"{"action":"hangup"}"#.to_vec()))
            .await
            .unwrap();
        let mut saw_hangup = false;
        while let Some(c) = h.room_cmds.recv().await {
            if matches!(c, RoomCommand::Hangup) {
                saw_hangup = true;
                break;
            }
        }
        assert!(saw_hangup);
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "hangup_control");
        assert_eq!(evs.last().unwrap().event_type, "call.ended");
    }

    #[tokio::test]
    async fn unconfigured_call_still_discloses_then_hangs_up() {
        let (core_tx, mut core_cmds) = mpsc::channel(16);
        let (core_events, core_rx) = mpsc::channel(16);
        let (room_tx, mut room_cmds) = mpsc::channel(64);
        let (_room_input, room_rx) = mpsc::channel(16);
        let core = CoreHandle {
            tx: core_tx,
            rx: core_rx,
            output_sample_rate: 24_000,
        };
        let t = tokio::spawn(run_unconfigured_call(
            core,
            room_tx,
            room_rx,
            Duration::from_secs(5),
        ));
        let CoreCommand::SpeakDelta { text, .. } = core_cmds.recv().await.unwrap() else {
            panic!()
        };
        assert_eq!(
            text,
            format!(
                "Hi, I'm an AI assistant, and this call isn't recorded. {RELAY_UNAVAILABLE_LINE}"
            )
        );
        core_events
            .send(CoreEvent::SpeakEnded { id: "u-0".into() })
            .await
            .unwrap();
        t.await.unwrap();
        let mut last = None;
        while let Ok(c) = room_cmds.try_recv() {
            last = Some(c);
        }
        assert!(matches!(last, Some(RoomCommand::Hangup)));
    }
}
