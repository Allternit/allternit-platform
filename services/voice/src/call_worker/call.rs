//! One phone call: the state machine between the room (LiveKit), the Voice
//! Session core, the brain (cloud-api relay) and the event queue.
//!
//! The room side is two channels ([`RoomCommand`] out, [`RoomInput`] in) so this
//! file has no LiveKit types and is tested end to end with fakes; `room.rs`
//! fills them from the real room.
//!
//! Beyond the turn loop the call handles:
//! - **takeover**: a second, transcription-only core listens to the human who
//!   joined the room ([`Call::start_human_pipeline`]);
//! - **hold**: hold music instead of silence ([`RoomCommand::HoldMusic`]);
//! - **warm transfer**: hold, consult the target in another room, brief them,
//!   then connect ([`transfer`]);
//! - **outbound answer screening**: don't speak until it's clear a person
//!   answered; leave a short message on a machine ([`voicemail`]);
//! - **recording**: the opening states whether the call is recorded, from what
//!   is really happening ([`recording`]).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::audio::{to_pcm16le, Framer, Pcm16Decoder, Resampler, TRACK_RATE};
use super::brain::{CallBrain, RELAY_UNAVAILABLE_LINE};
use super::cloud_client::{BotConfig, TurnRequest};
use super::controls::{parse_control, CallState, Control, Target};
use super::disclosure;
use super::invite_code::{self, InviteCall};
use super::events::{CallEvent, EventQueue, Speaker, TransferMode};
use super::recording::Recording;
use super::session_adapter::{CoreCommand, CoreEvent, CoreHandle};
use super::transfer::{self, ConsultDriver, TransferOutcome, TransferPlan};
use super::voicemail::{self, Signal, VoicemailAction, VoicemailDetector};

/// What the call asks the room to do.
#[derive(Debug)]
pub enum RoomCommand {
    /// One 10 ms frame of bot speech, mono PCM16 at [`TRACK_RATE`].
    PublishFrame(Vec<i16>),
    /// Drop bot audio already queued in the track source (barge-in, mute, hold).
    ClearAudio,
    /// Start or stop the hold music on the bot's track.
    HoldMusic(bool),
    /// One 10 ms frame of the warm-transfer briefing, for the consult room.
    ConsultFrame(Vec<i16>),
    /// Drop briefing audio already queued for the consult room.
    ConsultClear,
    /// Send DTMF to the caller.
    SendDtmf(String),
    /// Cold transfer via SIP REFER; the room answers with [`RoomInput::TransferResult`].
    Transfer { to: String },
    /// End the call now (delete the room, which drops the SIP leg).
    Hangup,
    /// End the call once the queued bot audio has played out (a closing line
    /// must not be cut off).
    HangupAfterPlayout,
    /// The bot leaves; the room and everyone else in it stay (warm transfer
    /// connected the caller and the target).
    Leave,
}

/// What the room tells the call.
#[derive(Debug)]
pub enum RoomInput {
    /// Caller audio, mono PCM16 at 16 kHz.
    CallerAudio(Vec<i16>),
    /// Audio of the human who took over, mono PCM16 at 16 kHz.
    HumanAudio(Vec<i16>),
    /// DTMF the caller pressed.
    Dtmf(String),
    /// Raw `allternit.call.control` payload (server-sent only; room.rs drops
    /// packets from participants).
    Control(Vec<u8>),
    TransferResult { to: String, ok: bool, reason: Option<String> },
    /// The SIP participant left (caller hung up, or a transfer completed).
    CallerLeft,
    /// The SIP participant's `sip.callStatus` changed (`dialing`, `ringing`, `active`, ...).
    SipStatus(String),
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
    /// The SIP leg was already answered when the worker joined (always true
    /// inbound). An outbound leg that is still ringing flips on `SipStatus("active")`.
    pub sip_answered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutcome {
    pub reason: String,
    pub duration_sec: u64,
    /// The bot should leave the room without deleting it (a warm transfer
    /// connected the caller and the target there).
    pub keep_room: bool,
}

/// How long a turn waits for the first reply byte before the fallback line.
pub const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(20);

/// Opens a transcription-only Voice Session for a human who took over.
pub type HumanCoreFactory = Arc<dyn Fn() -> BoxFuture<'static, anyhow::Result<CoreHandle>> + Send + Sync>;

/// Builds the consult leg for a warm transfer: `(target, consentRef)`.
pub type ConsultLauncher = Arc<dyn Fn(&str, &str) -> Arc<dyn ConsultDriver> + Send + Sync>;

/// Everything a warm transfer needs from the worker.
#[derive(Clone)]
pub struct WarmTransfer {
    /// LiveKit outbound SIP trunk; `None` fails the transfer with the reason.
    pub outbound_trunk: Option<String>,
    pub ring_timeout: Duration,
    pub accept_timeout: Duration,
    pub launch: ConsultLauncher,
}

/// What a call gets besides its room, core, brain and events.
pub struct CallDeps {
    /// Answer screening: `NoScreening` on inbound, `AnswerScreen` on outbound.
    pub voicemail: Box<dyn VoicemailDetector>,
    pub recording: Recording,
    /// `None`: a takeover is not transcribed (logged, never silently).
    pub human_core: Option<HumanCoreFactory>,
    /// `None`: warm transfer answers `ok:false`.
    pub warm: Option<WarmTransfer>,
    /// `Some`: an invite-code call. No conversation and no brain; it speaks the
    /// code script once and hangs up.
    pub invite: Option<InviteCall>,
}

impl CallDeps {
    /// Inbound call with no recording, takeover transcription or warm transfer.
    pub fn bare(voicemail: Box<dyn VoicemailDetector>) -> Self {
        Self { voicemail, recording: Recording::none(), human_core: None, warm: None, invite: None }
    }
}

enum BrainMsg {
    Delta(u64, String),
    Done(u64),
    Failed(u64, String),
}

/// Messages from the takeover transcription pipeline.
enum HumanMsg {
    Opened(u64, mpsc::Sender<CoreCommand>),
    Event(u64, CoreEvent),
    Failed(u64, String),
}

/// Messages from a running warm transfer.
enum XferMsg {
    /// Speak this to the target in the consult room; answer `true` once said.
    Brief { text: String, done: oneshot::Sender<bool> },
    Done(TransferOutcome),
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

/// Where the bot's current speech goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dest {
    Caller,
    /// The warm-transfer briefing, into the consult room.
    Consult,
}

/// Who answered an outbound call. Inbound calls are `Person` from the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Waiting to find out; the bot hasn't spoken.
    Screening,
    /// A person (or an inbound call): normal conversation.
    Person,
    /// An answering machine: waiting for the beep.
    Machine,
    /// Speaking the voicemail message; hang up when it ends.
    LeavingMessage,
}

struct Xfer {
    to: String,
    driver: Arc<dyn ConsultDriver>,
    task: JoinHandle<()>,
    /// Briefing in flight: resolves the pipeline's `brief` call when spoken.
    brief_done: Option<oneshot::Sender<bool>>,
    brief_id: Option<String>,
}

/// An outbound leg that hasn't been answered by now is given up on.
const DIAL_TIMEOUT_MS: u64 = 60_000;
/// Voicemail utterance id.
const VM_ID: &str = "vm-0";
/// An invite-code call that hasn't finished by now hangs up.
const INVITE_MAX_MS: u64 = 120_000;
/// Most final transcript lines kept for the transfer briefing.
const TRANSCRIPT_KEEP: usize = 200;
/// Human audio buffered while the transcription core is still opening
/// (10 ms-ish frames from the room; about 2 s).
const HUMAN_BUFFER_FRAMES: usize = 200;

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
    dest: Dest,
    answer: Answer,
    /// The callee picked up (inbound: always). Unanswered outbound ends as `no_answer`.
    sip_answered: bool,
    /// The bot's first audio frame reached the caller.
    bot_audio_sent: bool,
    voicemail: Box<dyn VoicemailDetector>,
    /// The opening, held back on outbound calls until a person is confirmed.
    pending_opening: Option<String>,
    /// The pickup's own turn ("Hello?") must not reach the brain.
    skip_turn: bool,
    transferred: bool,
    keep_room: bool,
    end_reason: Option<String>,
    // takeover transcription
    human_core: Option<HumanCoreFactory>,
    human_tx: mpsc::UnboundedSender<HumanMsg>,
    human_gen: u64,
    human_core_tx: Option<mpsc::Sender<CoreCommand>>,
    human_buffer: VecDeque<Vec<u8>>,
    // warm transfer
    warm: Option<WarmTransfer>,
    xfer: Option<Xfer>,
    xfer_tx: mpsc::UnboundedSender<XferMsg>,
    xfer_count: u64,
    /// Final lines, for the transfer briefing.
    transcript: Vec<(Speaker, String)>,
    recording: Option<Recording>,
    invite: Option<InviteCall>,
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
    deps: CallDeps,
) -> (CallOutcome, JoinHandle<()>) {
    let CoreHandle { tx: core_tx, rx: mut core_rx, output_sample_rate } = core;
    let (brain_tx, mut brain_rx) = mpsc::unbounded_channel();
    let (human_tx, mut human_rx) = mpsc::unbounded_channel();
    let (xfer_tx, mut xfer_rx) = mpsc::unbounded_channel();
    let CallDeps { voicemail, recording, human_core, warm, invite } = deps;
    let screening = voicemail.active();
    let mut call = Call {
        started: Instant::now(),
        state: CallState { recording: recording.active, ..Default::default() },
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
        dest: Dest::Caller,
        sip_answered: ctx.sip_answered,
        bot_audio_sent: false,
        answer: if screening { Answer::Screening } else { Answer::Person },
        voicemail,
        pending_opening: None,
        skip_turn: false,
        transferred: false,
        keep_room: false,
        end_reason: None,
        human_core,
        human_tx,
        human_gen: 0,
        human_core_tx: None,
        human_buffer: VecDeque::new(),
        warm,
        xfer: None,
        xfer_tx,
        xfer_count: 0,
        transcript: Vec::new(),
        recording: None,
        invite,
        ctx,
    };

    // Speak first. The opening comes from the cached bot config only; nothing
    // here waits on the user's runtime. Whether it says "recorded" follows what
    // the recording setup actually did (decided before the call got here).
    let opening = match &call.invite {
        // The code never goes through the phrase cache, so no `Prepare`.
        Some(inv) => inv.script(call.ctx.bot.display_name().as_deref()),
        None => {
            let opening = disclosure::opening(
                call.ctx.bot.display_name().as_deref(),
                recording.active,
                call.ctx.bot.greeting.as_deref(),
            );
            // Pre-render the opening (X1 phrase cache) so the disclosure plays fast.
            let _ = call.core_tx.send(CoreCommand::Prepare { texts: vec![opening.clone()] }).await;
            opening
        }
    };
    if screening {
        // Outbound: a machine would hear us talk over its greeting. Wait until
        // a person is confirmed.
        call.pending_opening = Some(opening);
    } else {
        call.say("u-0", &opening).await;
    }
    call.events.emit(CallEvent::Started {
        direction: call.ctx.direction.clone(),
        from: if call.ctx.direction == "outbound" { call.ctx.local.clone() } else { call.ctx.remote.clone() },
        to: if call.ctx.direction == "outbound" { call.ctx.remote.clone() } else { call.ctx.local.clone() },
        number_id: call.ctx.number_id.clone(),
    });
    if let Some(why) = &recording.unavailable {
        // Recording was requested and isn't happening: say so in the state the
        // UI shows. The opening already told the caller "isn't recorded".
        tracing::error!(call_id = %call.ctx.call_id, "{why}");
        call.events.emit(call.state.state_event());
    }
    call.recording = Some(recording);

    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
            Some(m) = human_rx.recv() => call.on_human(m).await,
            Some(m) = xfer_rx.recv() => call.on_xfer(m).await,
            _ = tick.tick() => call.on_tick().await,
        }
    }

    call.cancel_reply();
    call.cancel_transfer("the call ended").await;
    call.end_human_pipeline().await;
    let _ = call.core_tx.send(CoreCommand::End).await;
    let mut reason = call.end_reason.take().unwrap_or_default();
    let answered = call.sip_answered;
    if !answered {
        reason = "no_answer".into();
    }
    let missed = answered && call.ctx.direction == "inbound" && reason == "caller_hangup" && !call.bot_audio_sent;
    let duration_sec = call.started.elapsed().as_secs();
    let recording_ref = match call.recording.take() {
        Some(r) => tokio::time::timeout(Duration::from_secs(5), r.finish(&call.ctx.call_id)).await.unwrap_or_else(|_| {
            tracing::error!(call_id = %call.ctx.call_id, "stopping the recording timed out, no recordingRef");
            None
        }),
        None => None,
    };
    call.events.emit(CallEvent::Ended { duration_sec, reason: reason.clone(), recording_ref, answered, missed });
    let keep_room = call.keep_room;
    (CallOutcome { reason, duration_sec, keep_room }, call.events.close())
}

impl Call {
    fn end(&mut self, reason: &str) {
        if self.end_reason.is_none() {
            self.end_reason = Some(reason.to_string());
        }
    }

    fn at_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn remember(&mut self, speaker: Speaker, text: &str) {
        if !text.trim().is_empty() {
            self.transcript.push((speaker, text.trim().to_string()));
            if self.transcript.len() > TRANSCRIPT_KEEP {
                self.transcript.remove(0);
            }
        }
    }

    async fn say(&mut self, id: &str, text: &str) {
        // An invite-code call keeps no text of what it said: it holds the code.
        if self.invite.is_none() {
            self.said.entry(id.to_string()).or_default().push_str(text);
        }
        let _ = self.core_tx.send(CoreCommand::SpeakDelta { id: id.into(), text: text.into() }).await;
        let _ = self.core_tx.send(CoreCommand::SpeakDone { id: id.into() }).await;
    }

    fn bot_transcript(&mut self, id: &str) {
        if let Some(text) = self.said.remove(id).filter(|t| !t.trim().is_empty()) {
            self.remember(Speaker::Bot, &text);
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
        let _ = self.core_tx.send(CoreCommand::SpeakCancel { id: None }).await;
        let _ = self.room_tx.send(RoomCommand::ClearAudio).await;
        self.speech.reset();
        self.drop_audio = true;
    }

    async fn on_tick(&mut self) {
        if !self.sip_answered && self.at_ms() > DIAL_TIMEOUT_MS {
            // Ringing too long: stop dialing.
            let _ = self.room_tx.send(RoomCommand::Hangup).await;
            self.end("no_answer");
            return;
        }
        if self.invite.is_some() && self.at_ms() > INVITE_MAX_MS {
            // Nobody to read to (or the voice stalled): don't hold the line.
            let _ = self.room_tx.send(RoomCommand::Hangup).await;
            self.end(invite_code::END_FAILED);
            return;
        }
        if matches!(self.answer, Answer::Screening | Answer::Machine) {
            let at = self.at_ms();
            if let Some(sig) = self.voicemail.on_tick(at) {
                self.on_signal(sig, false).await;
            }
        }
    }

    /// Outbound answer screening result.
    async fn on_signal(&mut self, sig: Signal, from_text: bool) {
        match sig {
            Signal::Human => {
                if self.answer != Answer::Screening {
                    return;
                }
                self.answer = Answer::Person;
                // The pickup's own turn ("Hello?") already has our answer: the opening.
                self.skip_turn = from_text;
                if let Some(opening) = self.pending_opening.take() {
                    self.say("u-0", &opening).await;
                }
            }
            Signal::Machine => {
                if self.answer == Answer::Screening {
                    tracing::info!(call_id = %self.ctx.call_id, "answering machine detected, waiting for the beep");
                    self.answer = Answer::Machine;
                    self.pending_opening = None;
                }
            }
            Signal::LeaveMessage => {
                if self.answer == Answer::Machine {
                    self.answer = Answer::LeavingMessage;
                    // On an invite-code call the message is the code itself: it's
                    // what the invitee asked to hear.
                    let text = match &self.invite {
                        Some(inv) => inv.script(self.ctx.bot.display_name().as_deref()),
                        None => voicemail::message(
                            self.ctx.bot.display_name().as_deref(),
                            &self.ctx.local,
                            self.ctx.bot.voicemail_message.as_deref(),
                        ),
                    };
                    self.say(VM_ID, &text).await;
                }
            }
            Signal::HangUp { reason } => {
                tracing::info!(call_id = %self.ctx.call_id, %reason, "voicemail: hanging up without a message");
                self.pending_opening = None;
                self.events.emit(CallEvent::VoicemailDetected { action: VoicemailAction::HungUp.as_str().into() });
                let _ = self.room_tx.send(RoomCommand::Hangup).await;
                self.end("voicemail");
            }
        }
    }

    async fn on_room(&mut self, input: RoomInput) {
        match input {
            RoomInput::CallerAudio(samples) => {
                if matches!(self.answer, Answer::Screening | Answer::Machine) {
                    let at = self.at_ms();
                    if let Some(sig) = self.voicemail.on_audio(&samples, at) {
                        self.on_signal(sig, false).await;
                    }
                }
                if !self.state.muted_caller && !self.state.held {
                    // Never block the call loop on the core; drop audio if it lags.
                    if self.core_tx.try_send(CoreCommand::Audio(to_pcm16le(&samples))).is_err() {
                        tracing::debug!(call_id = %self.ctx.call_id, "core input full, dropped caller frame");
                    }
                }
            }
            RoomInput::HumanAudio(samples) => self.on_human_audio(&samples),
            RoomInput::Dtmf(digits) => {
                let from = self.ctx.remote.clone();
                self.events.emit(CallEvent::Dtmf { digits, from });
            }
            RoomInput::Control(_) if self.invite.is_some() => {
                // Nobody drives an invite-code call; settle the UI's pending action.
                self.events.emit(self.state.state_event());
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
                self.events.emit(CallEvent::Transferred { to, mode: TransferMode::Cold, ok, reason });
                if ok {
                    self.transferred = true;
                }
            }
            RoomInput::CallerLeft => {
                self.cancel_transfer("the caller hung up").await;
                self.end(if self.transferred { "transferred" } else { "caller_hangup" })
            }
            RoomInput::SipStatus(s) => {
                if s == "active" {
                    self.sip_answered = true;
                }
            }
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
                if self.xfer.is_none() {
                    self.begin_hold().await;
                }
            }
            Control::Resume => {
                // Taking the caller back during a warm transfer cancels it.
                self.cancel_transfer("the transfer was cancelled").await;
                self.end_hold().await;
            }
            Control::Dtmf(digits) => {
                let _ = self.room_tx.send(RoomCommand::SendDtmf(digits.clone())).await;
                let from = self.ctx.local.clone();
                self.events.emit(CallEvent::Dtmf { digits, from });
                return;
            }
            Control::Transfer { to, mode: TransferMode::Cold, .. } => {
                self.cancel_reply();
                self.silence().await;
                let _ = self.room_tx.send(RoomCommand::Transfer { to }).await;
                return; // acked by call.transferred when the room answers
            }
            Control::Transfer { to, mode: TransferMode::Warm, consent_ref } => {
                self.start_warm_transfer(to, consent_ref).await;
                return;
            }
            Control::Takeover { by } => {
                self.state.takeover_by = Some(by.clone());
                self.cancel_reply();
                self.silence().await;
                self.start_human_pipeline();
                self.events.emit(CallEvent::Takeover { by, active: true });
                return;
            }
            Control::Release { by } => {
                self.state.takeover_by = None;
                self.end_human_pipeline().await;
                self.events.emit(CallEvent::Takeover { by, active: false });
                return;
            }
            Control::Listen => {}
        }
        self.events.emit(self.state.state_event());
    }

    /// Hold: the bot goes quiet, the caller hears the hold music.
    async fn begin_hold(&mut self) {
        self.state.held = true;
        self.cancel_reply();
        self.silence().await;
        let _ = self.core_tx.send(CoreCommand::MicMute).await;
        let _ = self.room_tx.send(RoomCommand::HoldMusic(true)).await;
    }

    async fn end_hold(&mut self) {
        let was_held = self.state.held;
        self.state.held = false;
        if was_held {
            let _ = self.room_tx.send(RoomCommand::HoldMusic(false)).await;
        }
        if !self.state.muted_caller {
            let _ = self.core_tx.send(CoreCommand::MicUnmute).await;
        }
    }

    // ---- takeover: transcribe the human ---------------------------------

    /// Open a second, transcription-only Voice Session for the human who took
    /// over: VAD and STT, no turn-taking, no brain, no speech. The caller's own
    /// pipeline keeps running.
    fn start_human_pipeline(&mut self) {
        self.human_gen += 1;
        self.human_core_tx = None;
        self.human_buffer.clear();
        let Some(factory) = self.human_core.clone() else {
            tracing::error!(
                call_id = %self.ctx.call_id,
                "takeover: this worker has no transcription core for the human; their speech is not transcribed"
            );
            return;
        };
        let (gen, tx) = (self.human_gen, self.human_tx.clone());
        tokio::spawn(async move {
            match factory().await {
                Ok(CoreHandle { tx: cmd_tx, mut rx, .. }) => {
                    let _ = tx.send(HumanMsg::Opened(gen, cmd_tx));
                    while let Some(ev) = rx.recv().await {
                        if tx.send(HumanMsg::Event(gen, ev)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(HumanMsg::Failed(gen, format!("{e:#}")));
                }
            }
        });
    }

    async fn end_human_pipeline(&mut self) {
        self.human_gen += 1; // events of the old pipeline are now stale
        self.human_buffer.clear();
        if let Some(tx) = self.human_core_tx.take() {
            let _ = tx.send(CoreCommand::End).await;
        }
        if self.state.speaker == Some(Speaker::Human) {
            self.state.speaker = None;
        }
    }

    fn on_human_audio(&mut self, samples: &[i16]) {
        if self.state.takeover_by.is_none() || self.human_core.is_none() {
            return; // nobody has taken over: not ours to transcribe
        }
        let bytes = to_pcm16le(samples);
        match &self.human_core_tx {
            Some(tx) => {
                if tx.try_send(CoreCommand::Audio(bytes)).is_err() {
                    tracing::debug!(call_id = %self.ctx.call_id, "human core input full, dropped frame");
                }
            }
            None => {
                // The core is still opening: keep the first couple of seconds.
                if self.human_buffer.len() < HUMAN_BUFFER_FRAMES {
                    self.human_buffer.push_back(bytes);
                }
            }
        }
    }

    async fn on_human(&mut self, m: HumanMsg) {
        match m {
            HumanMsg::Opened(gen, tx) => {
                if gen != self.human_gen {
                    let _ = tx.send(CoreCommand::End).await; // released before it opened
                    return;
                }
                for b in self.human_buffer.drain(..) {
                    let _ = tx.try_send(CoreCommand::Audio(b));
                }
                self.human_core_tx = Some(tx);
            }
            HumanMsg::Failed(gen, e) => {
                if gen == self.human_gen {
                    tracing::error!(call_id = %self.ctx.call_id, "takeover: couldn't open the human transcription core: {e}");
                }
            }
            HumanMsg::Event(gen, ev) => {
                if gen != self.human_gen {
                    return;
                }
                match ev {
                    CoreEvent::SpeechStarted => self.state.speaker = Some(Speaker::Human),
                    CoreEvent::SpeechStopped => {
                        if self.state.speaker == Some(Speaker::Human) {
                            self.state.speaker = None;
                        }
                    }
                    CoreEvent::TranscriptDelta { segment_id, text } => {
                        self.events.emit(CallEvent::TranscriptDelta {
                            speaker: Speaker::Human,
                            text,
                            is_final: false,
                            segment_id: format!("h-{segment_id}"),
                        });
                    }
                    CoreEvent::TranscriptFinal { segment_id, text } => {
                        self.remember(Speaker::Human, &text);
                        self.events.emit(CallEvent::TranscriptDelta {
                            speaker: Speaker::Human,
                            text,
                            is_final: true,
                            segment_id: format!("h-{segment_id}"),
                        });
                    }
                    CoreEvent::Error { code, message, fatal } => {
                        tracing::error!(call_id = %self.ctx.call_id, %code, fatal, "human transcription core error: {message}");
                    }
                    CoreEvent::Closed => {
                        if self.human_core_tx.take().is_some() {
                            tracing::error!(call_id = %self.ctx.call_id, "human transcription core closed mid-takeover");
                        }
                    }
                    // Transcription only: never a turn, never speech.
                    CoreEvent::TurnEnded { .. }
                    | CoreEvent::SpeakStarted { .. }
                    | CoreEvent::SpeakAudio(_)
                    | CoreEvent::SpeakEnded { .. }
                    | CoreEvent::SpeakInterrupted { .. } => {}
                }
            }
        }
    }

    // ---- warm transfer ---------------------------------------------------

    async fn start_warm_transfer(&mut self, to: String, consent_ref: Option<String>) {
        let fail = |this: &mut Self, to: String, reason: String| {
            tracing::warn!(call_id = %this.ctx.call_id, %reason, "warm transfer refused");
            this.events.emit(CallEvent::Transferred { to, mode: TransferMode::Warm, ok: false, reason: Some(reason) });
        };
        if self.xfer.is_some() {
            return fail(self, to, "a transfer is already in progress".into());
        }
        if self.state.takeover_by.is_some() {
            return fail(self, to, "a person has taken over this call; they can transfer it themselves".into());
        }
        let Some(warm) = self.warm.clone() else {
            return fail(self, to, "warm transfer isn't available on this worker".into());
        };
        if let Err(reason) = transfer::check_prerequisites(warm.outbound_trunk.as_deref(), consent_ref.as_deref()) {
            return fail(self, to, reason);
        }
        let consent = consent_ref.unwrap_or_default();

        // 1. Hold the caller, with music.
        self.begin_hold().await;
        self.events.emit(self.state.state_event());

        // 2-4. Dial, brief, connect: run off the call loop, which keeps
        // serving the room while the target's phone rings.
        let driver = (warm.launch)(&to, &consent);
        let plan = TransferPlan {
            briefing: transfer::briefing(self.ctx.bot.display_name().as_deref(), &self.ctx.remote, &self.transcript),
            ring_timeout: warm.ring_timeout,
            accept_timeout: warm.accept_timeout,
        };
        let msg_tx = self.xfer_tx.clone();
        let brief: transfer::BriefFn = {
            let msg_tx = msg_tx.clone();
            Arc::new(move |text| {
                let (done, rx) = oneshot::channel();
                let sent = msg_tx.send(XferMsg::Brief { text, done }).is_ok();
                Box::pin(async move { sent && rx.await.unwrap_or(false) })
            })
        };
        let task = {
            let driver = driver.clone();
            tokio::spawn(async move {
                let outcome = transfer::run_warm_transfer(driver, plan, brief).await;
                let _ = msg_tx.send(XferMsg::Done(outcome));
            })
        };
        self.xfer = Some(Xfer { to, driver, task, brief_done: None, brief_id: None });
    }

    async fn on_xfer(&mut self, m: XferMsg) {
        match m {
            XferMsg::Brief { text, done } => {
                let Some(x) = self.xfer.as_mut() else {
                    let _ = done.send(false);
                    return;
                };
                self.xfer_count += 1;
                let id = format!("brief-{}", self.xfer_count);
                x.brief_done = Some(done);
                x.brief_id = Some(id.clone());
                // Not `say`: the briefing is for the target, not part of the
                // caller's transcript.
                let _ = self.core_tx.send(CoreCommand::SpeakDelta { id: id.clone(), text }).await;
                let _ = self.core_tx.send(CoreCommand::SpeakDone { id }).await;
            }
            XferMsg::Done(outcome) => {
                let Some(x) = self.xfer.take() else { return };
                let _ = self.room_tx.send(RoomCommand::ConsultClear).await;
                self.dest = Dest::Caller;
                self.events.emit(CallEvent::Transferred {
                    to: x.to.clone(),
                    mode: TransferMode::Warm,
                    ok: outcome.ok(),
                    reason: outcome.reason(),
                });
                match outcome.caller_line(None) {
                    None => {
                        // Connected: the bot leaves, the room stays.
                        let _ = self.room_tx.send(RoomCommand::HoldMusic(false)).await;
                        self.state.held = false;
                        self.transferred = true;
                        self.keep_room = true;
                        let _ = self.room_tx.send(RoomCommand::Leave).await;
                        self.end("transferred");
                    }
                    Some(line) => {
                        // Not connected: take the caller off hold and say so.
                        self.end_hold().await;
                        self.events.emit(self.state.state_event());
                        let id = format!("u-x{}", self.xfer_count);
                        self.say(&id, &line).await;
                    }
                }
            }
        }
    }

    /// Stop a warm transfer in flight (caller hung up, owner resumed, call
    /// ending) and tear down its consult leg.
    async fn cancel_transfer(&mut self, why: &str) {
        let Some(x) = self.xfer.take() else { return };
        x.task.abort();
        if let Some(done) = x.brief_done {
            let _ = done.send(false);
        }
        if x.brief_id.is_some() {
            let _ = self.core_tx.send(CoreCommand::SpeakCancel { id: None }).await;
        }
        let _ = self.room_tx.send(RoomCommand::ConsultClear).await;
        x.driver.cleanup().await;
        self.dest = Dest::Caller;
        self.events.emit(CallEvent::Transferred {
            to: x.to,
            mode: TransferMode::Warm,
            ok: false,
            reason: Some(why.to_string()),
        });
    }

    fn consult_brief_id(&self) -> Option<&str> {
        self.xfer.as_ref().and_then(|x| x.brief_id.as_deref())
    }

    // ---- core events -----------------------------------------------------

    async fn on_core(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::SpeechStarted => {
                self.state.speaker = Some(Speaker::Caller);
                if matches!(self.answer, Answer::Screening | Answer::Machine) {
                    let at = self.at_ms();
                    if let Some(sig) = self.voicemail.on_speech(true, at) {
                        self.on_signal(sig, false).await;
                    }
                }
            }
            CoreEvent::SpeechStopped => {
                if matches!(self.answer, Answer::Screening | Answer::Machine) {
                    let at = self.at_ms();
                    if let Some(sig) = self.voicemail.on_speech(false, at) {
                        self.on_signal(sig, false).await;
                    }
                }
            }
            CoreEvent::TranscriptDelta { segment_id, text } => {
                if matches!(self.answer, Answer::Screening | Answer::Machine) {
                    let at = self.at_ms();
                    if let Some(sig) = self.voicemail.on_text(&text, false, at) {
                        self.on_signal(sig, false).await;
                    }
                }
                self.events.emit(CallEvent::TranscriptDelta {
                    speaker: Speaker::Caller,
                    text,
                    is_final: false,
                    segment_id,
                });
            }
            CoreEvent::TranscriptFinal { segment_id, text } => {
                if matches!(self.answer, Answer::Screening | Answer::Machine) {
                    let at = self.at_ms();
                    if let Some(sig) = self.voicemail.on_text(&text, true, at) {
                        self.on_signal(sig, true).await;
                    }
                }
                self.remember(Speaker::Caller, &text);
                self.events.emit(CallEvent::TranscriptDelta {
                    speaker: Speaker::Caller,
                    text,
                    is_final: true,
                    segment_id,
                });
            }
            CoreEvent::TurnEnded { text, confidence } => {
                self.state.speaker = None;
                if self.invite.is_some() {
                    return; // no conversation, no brain
                }
                if self.skip_turn {
                    self.skip_turn = false; // the pickup greeting, already answered
                    return;
                }
                if text.trim().is_empty() || !self.state.bot_active() || self.answer != Answer::Person {
                    return; // during takeover/hold/screening the bot only transcribes
                }
                self.start_turn(text, confidence);
            }
            CoreEvent::SpeakStarted { id } => {
                self.speech.reset();
                self.drop_audio = false;
                self.dest = if self.consult_brief_id() == Some(id.as_str()) { Dest::Consult } else { Dest::Caller };
                if self.dest == Dest::Caller && self.state.bot_active() && !self.state.muted_bot {
                    self.state.speaker = Some(Speaker::Bot);
                }
            }
            CoreEvent::SpeakAudio(bytes) => {
                if self.dest == Dest::Consult {
                    for f in self.speech.frames(&bytes) {
                        let _ = self.room_tx.send(RoomCommand::ConsultFrame(f)).await;
                    }
                    return;
                }
                if self.drop_audio || self.state.muted_bot || !self.state.bot_active() {
                    return;
                }
                for f in self.speech.frames(&bytes) {
                    self.bot_audio_sent = true;
                    let _ = self.room_tx.send(RoomCommand::PublishFrame(f)).await;
                }
            }
            CoreEvent::SpeakEnded { id } => {
                if self.dest == Dest::Consult && self.consult_brief_id() == Some(id.as_str()) {
                    if let Some(f) = self.speech.framer.flush() {
                        let _ = self.room_tx.send(RoomCommand::ConsultFrame(f)).await;
                    }
                    if let Some(done) = self.xfer.as_mut().and_then(|x| x.brief_done.take()) {
                        let _ = done.send(true);
                    }
                    self.dest = Dest::Caller;
                    return;
                }
                if let Some(f) = self.speech.framer.flush() {
                    if !self.drop_audio && !self.state.muted_bot && self.state.bot_active() {
                        let _ = self.room_tx.send(RoomCommand::PublishFrame(f)).await;
                    }
                }
                if self.state.speaker == Some(Speaker::Bot) {
                    self.state.speaker = None;
                }
                self.bot_transcript(&id);
                if let Some(inv) = &self.invite {
                    if id == "u-0" || id == VM_ID {
                        let reason = inv.end_reason();
                        if inv.is_valid() {
                            self.events.emit(CallEvent::TranscriptDelta {
                                speaker: Speaker::Bot,
                                text: invite_code::REDACTED_TRANSCRIPT.into(),
                                is_final: true,
                                segment_id: id.clone(),
                            });
                        }
                        if id == VM_ID {
                            self.events.emit(CallEvent::VoicemailDetected { action: VoicemailAction::LeftMessage.as_str().into() });
                        }
                        let _ = self.room_tx.send(RoomCommand::HangupAfterPlayout).await;
                        self.end(reason);
                    }
                    return;
                }
                if id == VM_ID && self.answer == Answer::LeavingMessage {
                    self.events.emit(CallEvent::VoicemailDetected { action: VoicemailAction::LeftMessage.as_str().into() });
                    let _ = self.room_tx.send(RoomCommand::HangupAfterPlayout).await;
                    self.end("voicemail");
                }
            }
            CoreEvent::SpeakInterrupted { .. } if self.invite.is_some() => {
                // A "Hello?" over the code must not cut it off.
            }
            CoreEvent::SpeakInterrupted { id } => {
                if self.consult_brief_id() == Some(id.as_str()) {
                    return; // nobody talks over the briefing; the target's line isn't the caller's mic
                }
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
            CoreEvent::Error { code, message, fatal } => {
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
        let id = self.reply_id.get_or_insert_with(|| format!("u-{turn}")).clone();
        match msg {
            Ok(Some(text)) => {
                if text.is_empty() {
                    return;
                }
                self.reply_had_text = true;
                self.said.entry(id.clone()).or_default().push_str(&text);
                let _ = self.core_tx.send(CoreCommand::SpeakDelta { id, text }).await;
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
    let CoreHandle { tx, rx: mut core_rx, output_sample_rate } = core;
    let text = format!("{} {}", disclosure::disclosure(None, false), RELAY_UNAVAILABLE_LINE);
    let _ = tx.send(CoreCommand::SpeakDelta { id: "u-0".into(), text }).await;
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
                    // Let the queued audio play out before hanging up.
                    let _ = tx.send(CoreCommand::End).await;
                    let _ = room_tx.send(RoomCommand::HangupAfterPlayout).await;
                    return;
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
    use crate::call_worker::voicemail::NoScreening;
    use futures::future::BoxFuture;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<EventEnvelope>>);

    impl EventTransport for Recorder {
        fn post<'a>(&'a self, _id: &'a str, ev: &'a EventEnvelope) -> BoxFuture<'a, Result<(), CloudError>> {
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
            sip_answered: true,
            bot: BotConfig {
                name: Some("Acme Plumbing".into()),
                greeting: Some("How can I help?".into()),
                recording,
                ..Default::default()
            },
        }
    }

    fn start(recording: bool, replies: Vec<Result<Vec<&str>, &str>>) -> Harness {
        let deps = CallDeps {
            recording: Recording::assumed(recording),
            ..CallDeps::bare(Box::new(NoScreening))
        };
        start_with(deps, ctx(recording), replies)
    }

    fn start_with(deps: CallDeps, ctx: CallContext, replies: Vec<Result<Vec<&str>, &str>>) -> Harness {
        let (core_tx, core_cmds) = mpsc::channel(256);
        let (core_events, core_rx) = mpsc::channel(256);
        let (room_tx, room_cmds) = mpsc::channel(4096);
        let (room_input, room_rx) = mpsc::channel(256);
        let rec = Arc::new(Recorder::default());
        let brain = Arc::new(ScriptedBrain::new(replies));
        let events = EventQueue::start("c1", rec.clone(), Backoff::default());
        let core = CoreHandle { tx: core_tx, rx: core_rx, output_sample_rate: 24_000 };
        let task = tokio::spawn(run_call(ctx, core, brain.clone(), events, room_tx, room_rx, deps));
        Harness { core_cmds, core_events, room_cmds, room_input, rec, task, brain }
    }

    /// The next command the core hears. `Prepare` (pre-render of the opening)
    /// is a latency hint, not part of the spoken sequence these tests assert.
    async fn next_cmd(h: &mut Harness) -> CoreCommand {
        loop {
            let cmd = tokio::time::timeout(Duration::from_secs(2), h.core_cmds.recv()).await.unwrap().unwrap();
            if !matches!(cmd, CoreCommand::Prepare { .. }) {
                return cmd;
            }
        }
    }

    async fn finish(h: Harness) -> (CallOutcome, Vec<EventEnvelope>) {
        let (outcome, drain) = h.task.await.unwrap();
        drain.await.unwrap();
        let evs = h.rec.0.lock().unwrap().clone();
        (outcome, evs)
    }

    fn ended(evs: &[EventEnvelope]) -> serde_json::Value {
        evs.last().unwrap().payload.clone()
    }

    #[tokio::test]
    async fn outbound_unanswered_ends_as_no_answer() {
        let mut c = ctx(false);
        (c.direction, c.sip_answered) = ("outbound".into(), false);
        let h = start_with(CallDeps::bare(Box::new(NoScreening)), c, vec![]);
        h.room_input.send(RoomInput::SipStatus("ringing".into())).await.unwrap();
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "no_answer");
        let p = ended(&evs);
        assert_eq!((&p["answered"], &p["reason"]), (&json!(false), &json!("no_answer")));
        assert!(p.get("missed").is_none());
    }

    #[tokio::test]
    async fn outbound_answered_after_ringing_is_answered() {
        let mut c = ctx(false);
        (c.direction, c.sip_answered) = ("outbound".into(), false);
        let h = start_with(CallDeps::bare(Box::new(NoScreening)), c, vec![]);
        h.room_input.send(RoomInput::SipStatus("active".into())).await.unwrap();
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "caller_hangup");
        assert_eq!(ended(&evs)["answered"], true);
    }

    #[tokio::test]
    async fn inbound_hangup_before_first_audio_is_missed() {
        let h = start(false, vec![]);
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "caller_hangup");
        let p = ended(&evs);
        assert_eq!((&p["answered"], &p["missed"]), (&json!(true), &json!(true)));
    }

    #[tokio::test]
    async fn inbound_hangup_after_bot_audio_is_not_missed() {
        let mut h = start(false, vec![]);
        h.core_events.send(CoreEvent::SpeakAudio(to_pcm16le(&[100i16; 480]))).await.unwrap();
        h.core_events.send(CoreEvent::SpeakEnded { id: "u-0".into() }).await.unwrap();
        let RoomCommand::PublishFrame(_) = h.room_cmds.recv().await.unwrap() else { panic!() };
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (_, evs) = finish(h).await;
        let p = ended(&evs);
        assert_eq!(p["answered"], true);
        assert!(p.get("missed").is_none());
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
        assert_eq!(next_cmd(&mut h).await, CoreCommand::SpeakDone { id: "u-0".into() });

        // Bot audio is resampled 24k → 48k and published in 10 ms frames.
        h.core_events.send(CoreEvent::SpeakStarted { id: "u-0".into() }).await.unwrap();
        h.core_events.send(CoreEvent::SpeakAudio(to_pcm16le(&[100i16; 480]))).await.unwrap();
        h.core_events.send(CoreEvent::SpeakEnded { id: "u-0".into() }).await.unwrap();
        let RoomCommand::PublishFrame(f) = h.room_cmds.recv().await.unwrap() else { panic!() };
        assert_eq!(f.len(), 480);

        // Caller audio reaches the core.
        h.room_input.send(RoomInput::CallerAudio(vec![1, 2])).await.unwrap();
        assert_eq!(next_cmd(&mut h).await, CoreCommand::Audio(vec![1, 0, 2, 0]));

        // Caller turn → relay → reply streamed into the speak path.
        h.core_events.send(CoreEvent::TranscriptFinal { segment_id: "s1".into(), text: "When do you open?".into() }).await.unwrap();
        h.core_events.send(CoreEvent::TurnEnded { text: "When do you open?".into(), confidence: 0.9 }).await.unwrap();
        assert_eq!(next_cmd(&mut h).await, CoreCommand::SpeakDelta { id: "u-1".into(), text: "We open ".into() });
        assert_eq!(next_cmd(&mut h).await, CoreCommand::SpeakDelta { id: "u-1".into(), text: "at nine.".into() });
        assert_eq!(next_cmd(&mut h).await, CoreCommand::SpeakDone { id: "u-1".into() });
        assert_eq!(h.brain.seen.lock().unwrap()[0].turn_id, "turn:c1:1");
        h.core_events.send(CoreEvent::SpeakEnded { id: "u-1".into() }).await.unwrap();
        wait_events(&h, 4).await;

        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        let (outcome, evs) = finish(h).await;
        assert_eq!(outcome.reason, "caller_hangup");
        let kinds: Vec<_> = evs.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(
            kinds,
            ["call.started", "call.transcript.delta", "call.transcript.delta", "call.transcript.delta", "call.ended"]
        );
        assert_eq!(evs[1].payload["speaker"], "bot");
        assert!(evs[1].payload["text"].as_str().unwrap().starts_with("Hi, you've reached"));
        assert_eq!(evs[2].payload["speaker"], "caller");
        assert_eq!(evs[3].payload["text"], "We open at nine.");
        assert_eq!(evs[4].payload["reason"], "caller_hangup");
        let seqs: Vec<_> = evs.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn relay_failure_speaks_the_honest_line() {
        let mut h = start(true, vec![Err("relay 503")]);
        let CoreCommand::SpeakDelta { text, .. } = next_cmd(&mut h).await else { panic!() };
        assert!(text.contains("this call may be recorded"));
        next_cmd(&mut h).await;
        h.core_events.send(CoreEvent::TurnEnded { text: "hello?".into(), confidence: 1.0 }).await.unwrap();
        assert_eq!(
            next_cmd(&mut h).await,
            CoreCommand::SpeakDelta { id: "u-1".into(), text: RELAY_UNAVAILABLE_LINE.into() }
        );
        h.room_input.send(RoomInput::CallerLeft).await.unwrap();
        finish(h).await;
    }

    #[tokio::test]
    async fn barge_in_clears_audio_and_controls_ack() {
        let mut h = start(false, vec![]);
        next_cmd(&mut h).await;
        next_cmd(&mut h).await;
        h.core_events.send(CoreEvent::SpeakStarted { id: "u-0".into() }).await.unwrap();
        h.core_events.send(CoreEvent::SpeakInterrupted { id: "u-0".into() }).await.unwrap();
        assert!(matches!(h.room_cmds.recv().await.unwrap(), RoomCommand::ClearAudio));

        // Takeover: bot goes quiet, turns don't reach the brain, transcripts continue.
        h.room_input.send(RoomInput::Control(br#"{"action":"takeover","by":"user_1"}"#.to_vec())).await.unwrap();
        assert_eq!(next_cmd(&mut h).await, CoreCommand::SpeakCancel { id: None });
        h.core_events.send(CoreEvent::TurnEnded { text: "are you there".into(), confidence: 1.0 }).await.unwrap();
        h.core_events.send(CoreEvent::TranscriptDelta { segment_id: "s2".into(), text: "hel".into() }).await.unwrap();
        // Core events and room input arrive on separate channels; wait until the
        // transcript is recorded so the expected order below is deterministic.
        wait_events(&h, 4).await;
        h.room_input.send(RoomInput::Control(br#"{"action":"release","by":"user_1"}"#.to_vec())).await.unwrap();
        h.room_input.send(RoomInput::Control(br#"{"action":"mute","target":"caller"}"#.to_vec())).await.unwrap();
        h.room_input.send(RoomInput::Control(br#"{"action":"dtmf","digits":"42"}"#.to_vec())).await.unwrap();
        h.room_input.send(RoomInput::Dtmf("9".into())).await.unwrap();
        h.room_input.send(RoomInput::Control(br#"{"action":"transfer","to":"+15105550100"}"#.to_vec())).await.unwrap();
        h.room_input.send(RoomInput::TransferResult { to: "+15105550100".into(), ok: true, reason: None }).await.unwrap();
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
        !evs.iter().any(|e| e.payload["speaker"] == "bot" && e.payload["segmentId"] != "u-0")
    }

    #[tokio::test]
    async fn hangup_control_ends_call() {
        let mut h = start(false, vec![]);
        next_cmd(&mut h).await;
        h.room_input.send(RoomInput::Control(br#"{"action":"hangup"}"#.to_vec())).await.unwrap();
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
        let core = CoreHandle { tx: core_tx, rx: core_rx, output_sample_rate: 24_000 };
        let t = tokio::spawn(run_unconfigured_call(core, room_tx, room_rx, Duration::from_secs(5)));
        let CoreCommand::SpeakDelta { text, .. } = core_cmds.recv().await.unwrap() else { panic!() };
        assert_eq!(text, format!("Hi, I'm an AI assistant, and this call isn't recorded. {RELAY_UNAVAILABLE_LINE}"));
        core_events.send(CoreEvent::SpeakEnded { id: "u-0".into() }).await.unwrap();
        t.await.unwrap();
        let mut last = None;
        while let Ok(c) = room_cmds.try_recv() {
            last = Some(c);
        }
        assert!(matches!(last, Some(RoomCommand::HangupAfterPlayout)));
    }

    fn invite_harness(otp: Option<&str>) -> Harness {
        let deps = CallDeps { invite: Some(InviteCall::from_attr(otp)), ..CallDeps::bare(Box::new(NoScreening)) };
        let mut c = ctx(false);
        c.direction = "outbound".into();
        start_with(deps, c, vec![Ok(vec!["brain must not run"])])
    }

    #[tokio::test]
    async fn invite_call_reads_the_code_once_redacted_and_hangs_up() {
        let mut h = invite_harness(Some("427193"));
        let CoreCommand::SpeakDelta { id, text } = next_cmd(&mut h).await else { panic!() };
        assert_eq!(id, "u-0");
        assert_eq!(
            text,
            "Hi, this is an automated call from Acme Plumbing, an AI assistant. This is Acme Plumbing's verification code: \
             4. 2. 7. 1. 9. 3. ... Again, your code is 4. 2. 7. 1. 9. 3. Goodbye."
        );
        assert_eq!(next_cmd(&mut h).await, CoreCommand::SpeakDone { id: "u-0".into() });
        // A "Hello?" does not start a turn and does not cut the code off.
        h.core_events.send(CoreEvent::TurnEnded { text: "hello?".into(), confidence: 1.0 }).await.unwrap();
        h.core_events.send(CoreEvent::SpeakInterrupted { id: "u-0".into() }).await.unwrap();
        h.core_events.send(CoreEvent::SpeakEnded { id: "u-0".into() }).await.unwrap();
        let (outcome, evs) = finish_ref(h).await;
        assert_eq!(outcome.reason, "bot_hangup");
        let kinds: Vec<_> = evs.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(kinds, ["call.started", "call.transcript.delta", "call.ended"]);
        assert_eq!(evs[1].payload["text"], "verification code read");
        assert_eq!(evs[2].payload["reason"], "bot_hangup");
        let all = serde_json::to_string(&evs).unwrap();
        assert!(!all.contains("427193") && !all.contains("4. 2. 7"));
    }

    #[tokio::test]
    async fn invite_call_never_calls_the_brain() {
        let mut h = invite_harness(Some("427193"));
        next_cmd(&mut h).await;
        next_cmd(&mut h).await;
        h.core_events.send(CoreEvent::TurnEnded { text: "who is this".into(), confidence: 1.0 }).await.unwrap();
        h.core_events.send(CoreEvent::SpeakEnded { id: "u-0".into() }).await.unwrap();
        let brain = h.brain.clone();
        finish(h).await;
        assert!(brain.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invite_call_with_a_bad_otp_apologizes_and_fails() {
        let mut h = invite_harness(Some("12ab"));
        let CoreCommand::SpeakDelta { text, .. } = next_cmd(&mut h).await else { panic!() };
        assert!(text.contains("Sorry, I can't read your verification code"));
        assert!(!text.contains("12ab"));
        next_cmd(&mut h).await;
        h.core_events.send(CoreEvent::SpeakEnded { id: "u-0".into() }).await.unwrap();
        let (outcome, evs) = finish_ref(h).await;
        assert_eq!(outcome.reason, "failed");
        assert_eq!(evs.last().unwrap().payload["reason"], "failed");
        assert!(!evs.iter().any(|e| e.event_type == "call.transcript.delta"));
    }

    async fn finish_ref(h: Harness) -> (CallOutcome, Vec<EventEnvelope>) {
        finish(h).await
    }
}
