//! `VoiceSession`: the transport-agnostic Voice Session state machine.
//!
//! Inputs arrive on one channel ([`SessionInput`]: mic PCM16 bytes and
//! protocol control messages); outputs leave on another ([`SessionOutput`]:
//! protocol events and speech PCM16 bytes). The WebSocket route and the
//! phone call worker are both thin adapters over this.
//!
//! Threads per session:
//! - **main loop** (async task): owns protocol state, utterance queue,
//!   barge-in, and paces speech audio to real time plus a small lead.
//! - **ear** (OS thread): VAD + turn detection over 16 kHz mic audio. Kept
//!   separate from STT so a slow decode never delays barge-in.
//! - **scribe** (OS thread): streaming STT per speech segment; joins the
//!   final segments of a turn into `turn.ended`.
//! - **mouth** (OS thread): TTS sentence by sentence.
//!
//! Clocks: `atMs` is mic-audio time (ms of mic audio received since
//! `session.start`, including muted audio), so it is deterministic and
//! matches what the client sent. `sentMs` is ms of speech audio sent for
//! that utterance.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, warn};

use super::engine::{
    EngineError, EngineFactory, EngineInfo, StreamingStt, SttOptions, Tts, TurnDetector, Vad,
    VadEvent, ENGINE_SAMPLE_RATE,
};
use super::protocol::{
    codes, ClientMessage, ModelIds, ServerEvent, SessionOptions, TurnMode, PROTOCOL_VERSION,
};
use super::resample::{f32_to_pcm16le, pcm16le_to_f32, Resampler};
use super::sentence::SentenceSplitter;

/// Smart Turn scores at most this much audio.
pub const TURN_WINDOW_SECS: usize = 8;
/// Smart Turn probability at or above which the turn ends at once.
pub const TURN_THRESHOLD: f32 = 0.5;
pub const DEFAULT_SILENCE_MS_VAD: u32 = 700;
pub const DEFAULT_SILENCE_MS_SMART: u32 = 1500;
/// Mic audio before the VAD's speech start that is still sent to STT
/// (Silero fires a little after the first phoneme).
const PRE_ROLL_MS: usize = 300;
const MAX_REMEMBERED_IDS: usize = 256;

/// Settings that are not part of the wire protocol (the transport decides).
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Force the speech output rate (the phone worker wants 8/48 kHz).
    /// `None` uses the TTS's native rate.
    pub output_sample_rate: Option<u32>,
    /// How far ahead of real-time playback speech audio may be sent.
    pub playback_lead_ms: u64,
    /// Speech audio frame size.
    pub output_frame_ms: u32,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            output_sample_rate: None,
            playback_lead_ms: 250,
            output_frame_ms: 20,
        }
    }
}

#[derive(Debug)]
pub enum SessionInput {
    /// Mic audio: PCM16 LE mono at the session's `inputSampleRate`.
    Audio(Vec<u8>),
    Control(ClientMessage),
    /// A frame the transport could not parse; answered with a non-fatal
    /// `bad_message` error.
    BadMessage(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionOutput {
    Event(ServerEvent),
    /// Speech audio: PCM16 LE mono at `outputSampleRate`.
    Audio(Vec<u8>),
    /// The session is over; the transport should close.
    Close,
}

pub struct SessionHandle {
    pub input: mpsc::Sender<SessionInput>,
    pub output: mpsc::Receiver<SessionOutput>,
}

pub struct VoiceSession;

impl VoiceSession {
    /// Start a session task. It waits for `session.start`, then runs until
    /// `session.end`, a fatal error, or the input sender is dropped.
    pub fn spawn(factory: Arc<dyn EngineFactory>, config: CoreConfig) -> SessionHandle {
        let (in_tx, in_rx) = mpsc::channel(512);
        let (out_tx, out_rx) = mpsc::channel(1024);
        tokio::spawn(run(factory, config, in_rx, out_tx));
        SessionHandle {
            input: in_tx,
            output: out_rx,
        }
    }
}

// ---------------------------------------------------------------- workers

/// Messages from worker threads to the main loop.
#[derive(Debug)]
enum Internal {
    SpeechStarted { at_ms: u64 },
    SpeechStopped { at_ms: u64 },
    TranscriptDelta { segment_id: String, text: String },
    TranscriptFinal { segment_id: String, text: String },
    TurnEnded { text: String, confidence: f32 },
    TtsAudio { seq: u64, samples: Vec<f32> },
    TtsSentenceDone { seq: u64 },
    EngineError { error: EngineError },
}

type InternalTx = mpsc::UnboundedSender<Internal>;

#[derive(Debug, Clone, Copy, PartialEq)]
struct TurnConfig {
    mode: TurnMode,
    silence_ms: u32,
}

enum EarMsg {
    Frame(Vec<f32>),
    /// Muted audio: advance the clock only.
    Advance(usize),
    /// Mute: drop any in-progress speech and turn.
    Reset,
    Configure(TurnConfig),
}

enum ScribeMsg {
    Begin,
    Samples(Vec<f32>),
    End,
    TurnEnd { confidence: f32 },
    DiscardTurn,
    Replace(Box<dyn StreamingStt>),
}

struct MouthJob {
    seq: u64,
    text: String,
    voice: String,
    cancelled: Arc<AtomicBool>,
}

fn ms_of(samples: u64, rate: u32) -> u64 {
    samples * 1000 / rate as u64
}

struct Ear {
    vad: Box<dyn Vad>,
    detector: Option<Box<dyn TurnDetector>>,
    turn: TurnConfig,
    scribe: std_mpsc::Sender<ScribeMsg>,
    main: InternalTx,
    ring: VecDeque<f32>,
    clock: u64,
    in_speech: bool,
    turn_open: bool,
    /// (deadline in samples, confidence) of a pending turn end.
    pending_end: Option<(u64, f32)>,
}

impl Ear {
    fn run(mut self, rx: std_mpsc::Receiver<EarMsg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                EarMsg::Frame(frame) => self.frame(&frame),
                EarMsg::Advance(n) => self.clock += n as u64,
                EarMsg::Reset => self.reset(),
                EarMsg::Configure(t) => self.turn = t,
            }
        }
    }

    fn at_ms(&self) -> u64 {
        ms_of(self.clock, ENGINE_SAMPLE_RATE)
    }

    fn frame(&mut self, frame: &[f32]) {
        self.clock += frame.len() as u64;
        self.ring.extend(frame.iter().copied());
        let cap = TURN_WINDOW_SECS * ENGINE_SAMPLE_RATE as usize;
        if self.ring.len() > cap {
            let excess = self.ring.len() - cap;
            self.ring.drain(..excess);
        }
        let event = self.vad.accept(frame);
        if self.in_speech {
            let _ = self.scribe.send(ScribeMsg::Samples(frame.to_vec()));
        }
        match event {
            Some(VadEvent::SpeechStart) if !self.in_speech => {
                self.in_speech = true;
                self.turn_open = true;
                self.pending_end = None;
                let _ = self.main.send(Internal::SpeechStarted {
                    at_ms: self.at_ms(),
                });
                let pre = (PRE_ROLL_MS * ENGINE_SAMPLE_RATE as usize / 1000).max(frame.len());
                let start = self.ring.len().saturating_sub(pre);
                let _ = self.scribe.send(ScribeMsg::Begin);
                let _ = self.scribe.send(ScribeMsg::Samples(
                    self.ring.range(start..).copied().collect(),
                ));
            }
            Some(VadEvent::SpeechStop) if self.in_speech => {
                self.in_speech = false;
                let _ = self.main.send(Internal::SpeechStopped {
                    at_ms: self.at_ms(),
                });
                let _ = self.scribe.send(ScribeMsg::End);
                self.on_speech_stopped();
            }
            _ => {}
        }
        if let Some((deadline, confidence)) = self.pending_end {
            if !self.in_speech && self.clock >= deadline {
                self.end_turn(confidence);
            }
        }
    }

    fn on_speech_stopped(&mut self) {
        let silence = self.turn.silence_ms as u64 * ENGINE_SAMPLE_RATE as u64 / 1000;
        let mut confidence = 1.0;
        if self.turn.mode == TurnMode::Smart {
            if let Some(det) = self.detector.as_mut() {
                let window: Vec<f32> = self.ring.iter().copied().collect();
                let started = std::time::Instant::now();
                let result = det.predict(&window);
                debug!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    ?result,
                    "smart turn"
                );
                match result {
                    Ok(p) if p >= TURN_THRESHOLD => {
                        self.end_turn(p);
                        return;
                    }
                    Ok(p) => confidence = p,
                    Err(error) => {
                        let _ = self.main.send(Internal::EngineError { error });
                    }
                }
            }
        }
        self.pending_end = Some((self.clock + silence, confidence));
    }

    fn end_turn(&mut self, confidence: f32) {
        self.pending_end = None;
        if self.turn_open {
            self.turn_open = false;
            let _ = self.scribe.send(ScribeMsg::TurnEnd { confidence });
        }
    }

    fn reset(&mut self) {
        self.vad.reset();
        self.ring.clear();
        self.pending_end = None;
        if self.in_speech {
            let _ = self.main.send(Internal::SpeechStopped {
                at_ms: self.at_ms(),
            });
        }
        self.in_speech = false;
        self.turn_open = false;
        let _ = self.scribe.send(ScribeMsg::DiscardTurn);
    }
}

struct Scribe {
    stt: Box<dyn StreamingStt>,
    main: InternalTx,
    segment: u64,
    in_segment: bool,
    finals: Vec<String>,
}

impl Scribe {
    fn run(mut self, rx: std_mpsc::Receiver<ScribeMsg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                ScribeMsg::Begin => {
                    self.segment += 1;
                    self.in_segment = true;
                    self.stt.begin();
                }
                ScribeMsg::Samples(mut samples) => {
                    // Catch up in one decode if audio queued while we were busy.
                    let mut next = None;
                    while let Ok(m) = rx.try_recv() {
                        match m {
                            ScribeMsg::Samples(more) => samples.extend(more),
                            other => {
                                next = Some(other);
                                break;
                            }
                        }
                    }
                    self.samples(&samples);
                    if let Some(m) = next {
                        self.handle_after_samples(m);
                    }
                }
                other => self.handle_after_samples(other),
            }
        }
    }

    fn handle_after_samples(&mut self, msg: ScribeMsg) {
        match msg {
            ScribeMsg::Begin => {
                self.segment += 1;
                self.in_segment = true;
                self.stt.begin();
            }
            ScribeMsg::Samples(s) => self.samples(&s),
            ScribeMsg::End => self.end(),
            ScribeMsg::TurnEnd { confidence } => {
                if self.in_segment {
                    self.end();
                }
                let text = self.finals.join(" ");
                self.finals.clear();
                if !text.trim().is_empty() {
                    let _ = self.main.send(Internal::TurnEnded { text, confidence });
                }
            }
            ScribeMsg::DiscardTurn => {
                if self.in_segment {
                    self.stt.abort();
                    self.in_segment = false;
                }
                self.finals.clear();
            }
            ScribeMsg::Replace(stt) => {
                if self.in_segment {
                    self.stt.abort();
                    self.in_segment = false;
                }
                self.stt = stt;
            }
        }
    }

    fn segment_id(&self) -> String {
        format!("seg-{}", self.segment)
    }

    fn samples(&mut self, samples: &[f32]) {
        if !self.in_segment {
            return;
        }
        match self.stt.accept(samples) {
            Ok(Some(text)) if !text.trim().is_empty() => {
                let _ = self.main.send(Internal::TranscriptDelta {
                    segment_id: self.segment_id(),
                    text,
                });
            }
            Ok(_) => {}
            Err(error) => {
                let _ = self.main.send(Internal::EngineError { error });
            }
        }
    }

    fn end(&mut self) {
        if !self.in_segment {
            return;
        }
        self.in_segment = false;
        match self.stt.finish() {
            Ok(text) => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    self.finals.push(text.clone());
                    let _ = self.main.send(Internal::TranscriptFinal {
                        segment_id: self.segment_id(),
                        text,
                    });
                }
            }
            Err(error) => {
                let _ = self.main.send(Internal::EngineError { error });
            }
        }
    }
}

fn run_mouth(mut tts: Box<dyn Tts>, rx: std_mpsc::Receiver<MouthJob>, main: InternalTx) {
    while let Ok(job) = rx.recv() {
        if !job.cancelled.load(Ordering::Acquire) {
            let cancelled = job.cancelled.clone();
            let seq = job.seq;
            let main2 = main.clone();
            let mut sink = |chunk: &[f32]| {
                if cancelled.load(Ordering::Acquire) {
                    return false;
                }
                main2
                    .send(Internal::TtsAudio {
                        seq,
                        samples: chunk.to_vec(),
                    })
                    .is_ok()
            };
            if let Err(error) = tts.synthesize(&job.text, &job.voice, &mut sink) {
                let _ = main.send(Internal::EngineError { error });
            }
        }
        let _ = main.send(Internal::TtsSentenceDone { seq: job.seq });
    }
}

// ---------------------------------------------------------------- main loop

/// Resolved session settings.
#[derive(Debug, Clone)]
struct Settings {
    voice: String,
    stt: SttOptions,
    input_rate: u32,
    barge_in: bool,
    turn: TurnConfig,
}

struct Utterance {
    id: String,
    seq: u64,
    cancelled: Arc<AtomicBool>,
    splitter: SentenceSplitter,
    done: bool,
    pending_sentences: usize,
    started: bool,
    queue: VecDeque<f32>,
    sent_samples: u64,
    resampler: Resampler,
}

impl Utterance {
    fn finished(&self) -> bool {
        self.done && self.pending_sentences == 0 && self.queue.is_empty()
    }
}

struct Running {
    settings: Settings,
    info: EngineInfo,
    has_detector: bool,
    out_rate: u32,
    tts_rate: u32,
    muted: bool,
    resampler: Resampler,
    odd_byte: Option<u8>,
    ear: std_mpsc::Sender<EarMsg>,
    scribe: std_mpsc::Sender<ScribeMsg>,
    mouth: std_mpsc::Sender<MouthJob>,
    utterances: VecDeque<Utterance>,
    next_seq: u64,
    /// Ids that are finished, cancelled or interrupted: late deltas are ignored.
    retired: VecDeque<String>,
    retired_set: HashSet<String>,
    /// When the client will have played everything sent so far.
    play_end: Option<Instant>,
}

struct Ctx {
    factory: Arc<dyn EngineFactory>,
    config: CoreConfig,
    out: mpsc::Sender<SessionOutput>,
    internal_tx: InternalTx,
}

impl Ctx {
    async fn emit(&self, ev: ServerEvent) -> bool {
        self.out.send(SessionOutput::Event(ev)).await.is_ok()
    }
}

async fn run(
    factory: Arc<dyn EngineFactory>,
    config: CoreConfig,
    mut input: mpsc::Receiver<SessionInput>,
    out: mpsc::Sender<SessionOutput>,
) {
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();
    let ctx = Ctx {
        factory,
        config,
        out,
        internal_tx,
    };
    let mut state: Option<Running> = None;
    let mut warned_not_started = false;

    loop {
        let deadline = state.as_ref().and_then(|s| next_pump_at(s, &ctx.config));
        let sleep = tokio::time::sleep_until(
            deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600)),
        );
        tokio::pin!(sleep);

        tokio::select! {
            msg = input.recv() => {
                let Some(msg) = msg else { break };
                match (msg, state.as_mut()) {
                    (SessionInput::BadMessage(why), _) => {
                        ctx.emit(ServerEvent::error(codes::BAD_MESSAGE, why, false)).await;
                    }
                    (SessionInput::Control(ClientMessage::SessionStart(opts)), None) => {
                        match start(&ctx, &opts).await {
                            Ok(running) => state = Some(running),
                            Err(ev) => {
                                ctx.emit(ev).await;
                                break;
                            }
                        }
                    }
                    (SessionInput::Control(ClientMessage::SessionEnd), _) => break,
                    (SessionInput::Control(_), None) | (SessionInput::Audio(_), None) => {
                        if !warned_not_started {
                            warned_not_started = true;
                            ctx.emit(ServerEvent::error(codes::NOT_STARTED, "send session.start first", false)).await;
                        }
                    }
                    (SessionInput::Control(ClientMessage::SessionStart(_)), Some(_)) => {
                        ctx.emit(ServerEvent::error(codes::ALREADY_STARTED, "session already started; use session.update", false)).await;
                    }
                    (SessionInput::Control(msg), Some(s)) => {
                        if !control(&ctx, s, msg).await {
                            break;
                        }
                    }
                    (SessionInput::Audio(bytes), Some(s)) => mic_audio(s, bytes),
                }
            }
            Some(msg) = internal_rx.recv() => {
                if let Some(s) = state.as_mut() {
                    if !internal(&ctx, s, msg).await {
                        break;
                    }
                }
            }
            _ = &mut sleep, if deadline.is_some() => {}
        }

        if let Some(s) = state.as_mut() {
            if !pump(&ctx, s).await {
                break;
            }
        }
    }
    // Dropping `state` closes the worker channels; the threads exit.
    let _ = ctx.out.send(SessionOutput::Close).await;
}

fn resolve(
    info_voices: &[String],
    default_voice: &str,
    base: Option<&Settings>,
    o: &SessionOptions,
    has_detector: bool,
) -> Result<(Settings, Option<ServerEvent>), String> {
    let mut warning = None;
    let voice = match &o.voice {
        Some(v) if !info_voices.is_empty() && !info_voices.contains(v) => {
            return Err(format!(
                "unknown voice '{v}' (available: {})",
                info_voices.join(", ")
            ))
        }
        Some(v) => v.clone(),
        None => base
            .map(|b| b.voice.clone())
            .unwrap_or_else(|| default_voice.to_string()),
    };
    let model = match o.stt_model.as_deref() {
        Some(m @ ("light" | "accurate")) => m.to_string(),
        Some(other) => {
            return Err(format!(
                "unknown sttModel '{other}' (expected light or accurate)"
            ))
        }
        None => base
            .map(|b| b.stt.model.clone())
            .unwrap_or_else(|| "light".into()),
    };
    let language = o
        .language
        .clone()
        .or_else(|| base.map(|b| b.stt.language.clone()))
        .unwrap_or_else(|| "en".into());
    let input_rate = match o.input_sample_rate {
        Some(r) if !(8_000..=96_000).contains(&r) => {
            return Err(format!("inputSampleRate {r} out of range 8000..96000"))
        }
        Some(r) => r,
        None => base.map(|b| b.input_rate).unwrap_or(16_000),
    };
    let barge_in = o.barge_in.or(base.map(|b| b.barge_in)).unwrap_or(true);
    let mut turn = base.map(|b| b.turn).unwrap_or(TurnConfig {
        mode: TurnMode::Smart,
        silence_ms: DEFAULT_SILENCE_MS_SMART,
    });
    if let Some(t) = &o.turn {
        let mode = t.mode.unwrap_or(turn.mode);
        let silence_default = match mode {
            TurnMode::Smart => DEFAULT_SILENCE_MS_SMART,
            TurnMode::Vad => DEFAULT_SILENCE_MS_VAD,
        };
        let silence_ms = t
            .silence_ms
            .unwrap_or(if mode == turn.mode && t.mode.is_none() {
                turn.silence_ms
            } else {
                silence_default
            });
        if !(100..=10_000).contains(&silence_ms) {
            return Err(format!(
                "turn.silenceMs {silence_ms} out of range 100..10000"
            ));
        }
        turn = TurnConfig { mode, silence_ms };
    }
    if turn.mode == TurnMode::Smart && !has_detector {
        let silence_ms = if o.turn.as_ref().and_then(|t| t.silence_ms).is_some() {
            turn.silence_ms
        } else {
            DEFAULT_SILENCE_MS_VAD
        };
        turn = TurnConfig {
            mode: TurnMode::Vad,
            silence_ms,
        };
        warning = Some(ServerEvent::error(
            codes::TURN_UNAVAILABLE,
            "smart turn detection is not available on this engine; using vad",
            false,
        ));
    }
    Ok((
        Settings {
            voice,
            stt: SttOptions { model, language },
            input_rate,
            barge_in,
            turn,
        },
        warning,
    ))
}

struct Engines {
    stt: Box<dyn StreamingStt>,
    tts: Box<dyn Tts>,
    vad: Box<dyn Vad>,
    detector: Option<Box<dyn TurnDetector>>,
}

async fn start(ctx: &Ctx, opts: &SessionOptions) -> Result<Running, ServerEvent> {
    let factory = ctx.factory.clone();
    let stt_opts = SttOptions {
        model: opts.stt_model.clone().unwrap_or_else(|| "light".into()),
        language: opts.language.clone().unwrap_or_else(|| "en".into()),
    };
    let info = factory.info(&stt_opts);
    let fatal = |e: EngineError| ServerEvent::error(e.code, e.message, true);

    // Validate before loading anything.
    let _ = resolve(&info.voices, &info.default_voice, None, opts, true)
        .map_err(|m| ServerEvent::error(codes::BAD_OPTION, m, true))?;

    let engines = tokio::task::spawn_blocking(move || -> Result<Engines, EngineError> {
        Ok(Engines {
            stt: factory.stt(&stt_opts)?,
            tts: factory.tts()?,
            vad: factory.vad()?,
            // Loaded even in vad mode so session.update can switch to smart.
            detector: factory.turn_detector()?,
        })
    })
    .await
    .map_err(|e| {
        ServerEvent::error(
            codes::ENGINE_ERROR,
            format!("engine start panicked: {e}"),
            true,
        )
    })?
    .map_err(fatal)?;

    let has_detector = engines.detector.is_some();
    let (settings, warning) = resolve(&info.voices, &info.default_voice, None, opts, has_detector)
        .map_err(|m| ServerEvent::error(codes::BAD_OPTION, m, true))?;

    let tts_rate = engines.tts.sample_rate();
    let out_rate = ctx.config.output_sample_rate.unwrap_or(tts_rate);

    let (scribe_tx, scribe_rx) = std_mpsc::channel();
    let (ear_tx, ear_rx) = std_mpsc::channel();
    let (mouth_tx, mouth_rx) = std_mpsc::channel();
    let ear = Ear {
        vad: engines.vad,
        detector: engines.detector,
        turn: settings.turn,
        scribe: scribe_tx.clone(),
        main: ctx.internal_tx.clone(),
        ring: VecDeque::new(),
        clock: 0,
        in_speech: false,
        turn_open: false,
        pending_end: None,
    };
    let scribe = Scribe {
        stt: engines.stt,
        main: ctx.internal_tx.clone(),
        segment: 0,
        in_segment: false,
        finals: Vec::new(),
    };
    let main = ctx.internal_tx.clone();
    let tts = engines.tts;
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| -> Result<(), String> {
        std::thread::Builder::new()
            .name(format!("voice-{name}"))
            .spawn(f)
            .map(drop)
            .map_err(|e| format!("spawn {name} thread: {e}"))
    };
    spawn("ear", Box::new(move || ear.run(ear_rx)))
        .and_then(|_| spawn("scribe", Box::new(move || scribe.run(scribe_rx))))
        .and_then(|_| spawn("mouth", Box::new(move || run_mouth(tts, mouth_rx, main))))
        .map_err(|m| ServerEvent::error(codes::ENGINE_ERROR, m, true))?;

    let turn_model = if settings.turn.mode == TurnMode::Smart {
        info.turn_model.clone()
    } else {
        "vad".to_string()
    };
    let ready = ServerEvent::SessionReady {
        protocol: PROTOCOL_VERSION,
        session_id: uuid::Uuid::new_v4().to_string(),
        engine: info.kind,
        output_sample_rate: out_rate,
        models: ModelIds {
            stt: info.stt_model.clone(),
            tts: info.tts_model.clone(),
            vad: info.vad_model.clone(),
            turn: turn_model,
        },
        voices: info.voices.clone(),
    };
    ctx.emit(ready).await;
    if let Some(w) = warning {
        ctx.emit(w).await;
    }
    debug!(?settings, "voice session started");
    Ok(Running {
        resampler: Resampler::new(settings.input_rate, ENGINE_SAMPLE_RATE),
        settings,
        info,
        has_detector,
        out_rate,
        tts_rate,
        muted: false,
        odd_byte: None,
        ear: ear_tx,
        scribe: scribe_tx,
        mouth: mouth_tx,
        utterances: VecDeque::new(),
        next_seq: 0,
        retired: VecDeque::new(),
        retired_set: HashSet::new(),
        play_end: None,
    })
}

fn mic_audio(s: &mut Running, mut bytes: Vec<u8>) {
    if let Some(b) = s.odd_byte.take() {
        bytes.insert(0, b);
    }
    if bytes.len() % 2 == 1 {
        s.odd_byte = bytes.pop();
    }
    let samples = s.resampler.process(&pcm16le_to_f32(&bytes));
    if samples.is_empty() {
        return;
    }
    let msg = if s.muted {
        EarMsg::Advance(samples.len())
    } else {
        EarMsg::Frame(samples)
    };
    let _ = s.ear.send(msg);
}

/// Handle a control message. Returns false when the session must end.
async fn control(ctx: &Ctx, s: &mut Running, msg: ClientMessage) -> bool {
    match msg {
        ClientMessage::SessionStart(_) | ClientMessage::SessionEnd => {}
        ClientMessage::SessionUpdate(opts) => update(ctx, s, opts).await,
        ClientMessage::SpeakDelta { id, text } => {
            if s.retired_set.contains(&id) {
                return true;
            }
            let idx = utterance_index(s, &id);
            let u = &mut s.utterances[idx];
            let sentences = u.splitter.push(&text);
            queue_sentences(s, idx, sentences);
        }
        ClientMessage::SpeakDone { id } => {
            if s.retired_set.contains(&id) {
                return true;
            }
            let Some(idx) = s.utterances.iter().position(|u| u.id == id) else {
                return true; // done for an id that never had text
            };
            let u = &mut s.utterances[idx];
            u.done = true;
            let last: Vec<String> = u.splitter.flush().into_iter().collect();
            queue_sentences(s, idx, last);
        }
        ClientMessage::SpeakCancel { id } => {
            let idx = match id {
                Some(id) => s.utterances.iter().position(|u| u.id == id),
                None => (!s.utterances.is_empty()).then_some(0),
            };
            if let Some(idx) = idx {
                if let Some(u) = s.utterances.remove(idx) {
                    u.cancelled.store(true, Ordering::Release);
                    if idx == 0 {
                        s.play_end = None;
                    }
                    retire(s, &u.id);
                    return ctx.emit(ServerEvent::SpeakEnded { id: u.id }).await;
                }
            }
        }
        ClientMessage::MicMute => {
            if !s.muted {
                s.muted = true;
                let _ = s.ear.send(EarMsg::Reset);
            }
        }
        ClientMessage::MicUnmute => s.muted = false,
    }
    true
}

async fn update(ctx: &Ctx, s: &mut Running, opts: SessionOptions) {
    let (next, warning) = match resolve(
        &s.info.voices,
        &s.info.default_voice,
        Some(&s.settings),
        &opts,
        s.has_detector,
    ) {
        Ok(v) => v,
        Err(m) => {
            ctx.emit(ServerEvent::error(codes::BAD_OPTION, m, false))
                .await;
            return;
        }
    };
    if let Some(w) = warning {
        if opts.turn.is_some() {
            ctx.emit(w).await;
        }
    }
    if next.stt != s.settings.stt {
        let factory = ctx.factory.clone();
        let o = next.stt.clone();
        match tokio::task::spawn_blocking(move || factory.stt(&o)).await {
            Ok(Ok(stt)) => {
                let _ = s.scribe.send(ScribeMsg::Replace(stt));
                s.info = ctx.factory.info(&next.stt);
            }
            Ok(Err(e)) => {
                ctx.emit(ServerEvent::error(e.code, e.message, false)).await;
                return;
            }
            Err(e) => {
                ctx.emit(ServerEvent::error(
                    codes::ENGINE_ERROR,
                    e.to_string(),
                    false,
                ))
                .await;
                return;
            }
        }
    }
    if next.input_rate != s.settings.input_rate {
        s.resampler = Resampler::new(next.input_rate, ENGINE_SAMPLE_RATE);
        s.odd_byte = None;
    }
    if next.turn != s.settings.turn {
        let _ = s.ear.send(EarMsg::Configure(next.turn));
    }
    s.settings = next;
}

fn utterance_index(s: &mut Running, id: &str) -> usize {
    if let Some(i) = s.utterances.iter().position(|u| u.id == id) {
        return i;
    }
    s.next_seq += 1;
    s.utterances.push_back(Utterance {
        id: id.to_string(),
        seq: s.next_seq,
        cancelled: Arc::new(AtomicBool::new(false)),
        splitter: SentenceSplitter::new(),
        done: false,
        pending_sentences: 0,
        started: false,
        queue: VecDeque::new(),
        sent_samples: 0,
        resampler: Resampler::new(s.tts_rate, s.out_rate),
    });
    s.utterances.len() - 1
}

fn queue_sentences(s: &mut Running, idx: usize, sentences: Vec<String>) {
    let u = &mut s.utterances[idx];
    for text in sentences {
        u.pending_sentences += 1;
        let _ = s.mouth.send(MouthJob {
            seq: u.seq,
            text,
            voice: s.settings.voice.clone(),
            cancelled: u.cancelled.clone(),
        });
    }
}

fn retire(s: &mut Running, id: &str) {
    if s.retired_set.insert(id.to_string()) {
        s.retired.push_back(id.to_string());
        if s.retired.len() > MAX_REMEMBERED_IDS {
            if let Some(old) = s.retired.pop_front() {
                s.retired_set.remove(&old);
            }
        }
    }
}

/// Handle a worker message. Returns false when the session must end.
async fn internal(ctx: &Ctx, s: &mut Running, msg: Internal) -> bool {
    match msg {
        Internal::SpeechStarted { at_ms } => {
            if !ctx.emit(ServerEvent::SpeechStarted { at_ms }).await {
                return false;
            }
            if s.settings.barge_in && !s.utterances.is_empty() {
                return barge_in(ctx, s).await;
            }
            true
        }
        Internal::SpeechStopped { at_ms } => ctx.emit(ServerEvent::SpeechStopped { at_ms }).await,
        Internal::TranscriptDelta { segment_id, text } => {
            ctx.emit(ServerEvent::TranscriptDelta {
                segment_id,
                text,
                is_final: false,
            })
            .await
        }
        Internal::TranscriptFinal { segment_id, text } => {
            ctx.emit(ServerEvent::TranscriptFinal { segment_id, text })
                .await
        }
        Internal::TurnEnded { text, confidence } => {
            ctx.emit(ServerEvent::TurnEnded { text, confidence }).await
        }
        Internal::TtsAudio { seq, samples } => {
            if let Some(u) = s.utterances.iter_mut().find(|u| u.seq == seq) {
                if !u.cancelled.load(Ordering::Acquire) {
                    let out = u.resampler.process(&samples);
                    u.queue.extend(out);
                }
            }
            true
        }
        Internal::TtsSentenceDone { seq } => {
            if let Some(u) = s.utterances.iter_mut().find(|u| u.seq == seq) {
                u.pending_sentences = u.pending_sentences.saturating_sub(1);
            }
            true
        }
        Internal::EngineError { error } => {
            warn!(%error, "voice engine error");
            ctx.emit(ServerEvent::error(error.code, error.message, false))
                .await
        }
    }
}

/// The user spoke over the bot: stop at once and drop everything queued.
async fn barge_in(ctx: &Ctx, s: &mut Running) -> bool {
    let dropped: Vec<Utterance> = s.utterances.drain(..).collect();
    s.play_end = None;
    for u in dropped {
        u.cancelled.store(true, Ordering::Release);
        retire(s, &u.id);
        let sent_ms = ms_of(u.sent_samples, s.out_rate);
        debug!(id = %u.id, sent_ms, dropped_samples = u.queue.len(), "barge-in");
        if !ctx
            .emit(ServerEvent::SpeakInterrupted { id: u.id, sent_ms })
            .await
        {
            return false;
        }
    }
    true
}

fn frame_samples(s: &Running, config: &CoreConfig) -> usize {
    (s.out_rate as usize * config.output_frame_ms as usize / 1000).max(1)
}

/// When the pump may next send audio, if it is waiting on the pacing clock.
fn next_pump_at(s: &Running, config: &CoreConfig) -> Option<Instant> {
    let u = s.utterances.front()?;
    let more_coming = !u.done || u.pending_sentences > 0;
    if u.queue.is_empty() || (u.queue.len() < frame_samples(s, config) && more_coming) {
        return None; // the next TtsAudio wakes the loop
    }
    let lead = Duration::from_millis(config.playback_lead_ms);
    Some(
        s.play_end
            .map(|p| p.checked_sub(lead).unwrap_or(p))
            .unwrap_or_else(Instant::now),
    )
}

/// Send paced speech audio for the current utterance and retire finished
/// utterances. Returns false when the output is gone.
async fn pump(ctx: &Ctx, s: &mut Running) -> bool {
    let frame = frame_samples(s, &ctx.config);
    let lead = Duration::from_millis(ctx.config.playback_lead_ms);
    let frame_dur = Duration::from_micros(ctx.config.output_frame_ms as u64 * 1000);
    loop {
        let out_rate = s.out_rate;
        let Some(u) = s.utterances.front_mut() else {
            return true;
        };
        // Send while the client is no more than `lead` ahead of real time.
        while !u.queue.is_empty() {
            let more_coming = !u.done || u.pending_sentences > 0;
            if u.queue.len() < frame && more_coming {
                break; // wait for a full frame
            }
            let now = Instant::now();
            let play_end = s.play_end.unwrap_or(now).max(now);
            if play_end > now + lead {
                break;
            }
            if !u.started {
                u.started = true;
                let id = u.id.clone();
                if ctx
                    .out
                    .send(SessionOutput::Event(ServerEvent::SpeakStarted { id }))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            let n = frame.min(u.queue.len());
            let chunk: Vec<f32> = u.queue.drain(..n).collect();
            u.sent_samples += n as u64;
            let dur = if n == frame {
                frame_dur
            } else {
                Duration::from_micros(n as u64 * 1_000_000 / out_rate as u64)
            };
            s.play_end = Some(play_end + dur);
            if ctx
                .out
                .send(SessionOutput::Audio(f32_to_pcm16le(&chunk)))
                .await
                .is_err()
            {
                return false;
            }
        }
        if !u.finished() {
            return true;
        }
        let u = s.utterances.pop_front().expect("front exists");
        if !u.started {
            // Text with nothing speakable: still bracket it for the client.
            if !ctx
                .emit(ServerEvent::SpeakStarted { id: u.id.clone() })
                .await
            {
                return false;
            }
        }
        retire(s, &u.id);
        if !ctx.emit(ServerEvent::SpeakEnded { id: u.id }).await {
            return false;
        }
    }
}
