//! Voice Session core scenarios driven by the mock engine (no models).

use std::sync::Arc;
use std::time::{Duration, Instant};

use voice_service::session::mock::{MockConfig, MockEngine};
use voice_service::session::protocol::{
    ClientMessage, ServerEvent, SessionOptions, TurnMode, TurnOptions,
};
use voice_service::session::{
    CoreConfig, SessionHandle, SessionInput, SessionOutput, VoiceSession,
};

const FRAME_MS: u32 = 20;

fn tone(ms: u32, rate: u32, amp: f32) -> Vec<u8> {
    let n = (rate * ms / 1000) as usize;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = (2.0 * std::f32::consts::PI * 300.0 * i as f32 / rate as f32).sin() * amp;
        out.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
    }
    out
}

struct Harness {
    h: SessionHandle,
    rate: u32,
    /// Audio bytes received since the last `take_audio`.
    audio: usize,
}

impl Harness {
    fn new(engine: MockEngine, config: CoreConfig) -> Self {
        Self {
            h: VoiceSession::spawn(Arc::new(engine), config),
            rate: 16_000,
            audio: 0,
        }
    }

    async fn send(&self, m: ClientMessage) {
        self.h.input.send(SessionInput::Control(m)).await.unwrap();
    }

    async fn start(&mut self, opts: SessionOptions) -> ServerEvent {
        if let Some(r) = opts.input_sample_rate {
            self.rate = r;
        }
        self.send(ClientMessage::SessionStart(opts)).await;
        self.expect("session.ready").await
    }

    /// Stream `ms` of speech (or silence) in 20 ms frames, as fast as possible.
    async fn audio(&self, ms: u32, speech: bool) {
        for _ in 0..ms / FRAME_MS {
            let frame = tone(FRAME_MS, self.rate, if speech { 0.3 } else { 0.0 });
            self.h.input.send(SessionInput::Audio(frame)).await.unwrap();
        }
    }

    async fn next(&mut self, timeout: Duration) -> Option<SessionOutput> {
        loop {
            match tokio::time::timeout(timeout, self.h.output.recv()).await {
                Ok(Some(SessionOutput::Audio(b))) => self.audio += b.len(),
                Ok(other) => return other,
                Err(_) => return None,
            }
        }
    }

    async fn next_event(&mut self, timeout: Duration) -> Option<ServerEvent> {
        match self.next(timeout).await {
            Some(SessionOutput::Event(ev)) => Some(ev),
            Some(SessionOutput::Close) | None => None,
            Some(SessionOutput::Audio(_)) => unreachable!(),
        }
    }

    /// Wait for an event of `kind`, returning it. Panics on timeout.
    async fn expect(&mut self, kind: &str) -> ServerEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            if let Some(ev) = self.next_event(Duration::from_millis(100)).await {
                if ev.kind() == kind {
                    return ev;
                }
                seen.push(ev.kind());
            }
        }
        panic!("timed out waiting for {kind}; saw {seen:?}");
    }

    /// Collect events for `dur`.
    async fn drain(&mut self, dur: Duration) -> Vec<ServerEvent> {
        let deadline = Instant::now() + dur;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return out;
            }
            match self.next(left).await {
                Some(SessionOutput::Event(ev)) => out.push(ev),
                Some(SessionOutput::Close) | None => return out,
                Some(SessionOutput::Audio(_)) => unreachable!(),
            }
        }
    }

    fn take_audio(&mut self) -> usize {
        std::mem::take(&mut self.audio)
    }
}

fn kinds(evs: &[ServerEvent]) -> Vec<&'static str> {
    evs.iter().map(|e| e.kind()).collect()
}

fn vad_turn(silence_ms: u32) -> Option<TurnOptions> {
    Some(TurnOptions {
        mode: Some(TurnMode::Vad),
        silence_ms: Some(silence_ms),
    })
}

#[tokio::test]
async fn simple_turn_then_reply() {
    let engine = MockEngine::new(MockConfig {
        transcripts: vec!["hello there".into()],
        ..Default::default()
    });
    let mut t = Harness::new(engine.clone(), CoreConfig::default());
    let ready = t.start(SessionOptions::default()).await;
    let ServerEvent::SessionReady {
        protocol,
        output_sample_rate,
        models,
        voices,
        ..
    } = ready
    else {
        unreachable!()
    };
    assert_eq!(protocol, 1);
    assert_eq!(output_sample_rate, 24_000);
    assert_eq!(models.turn, "mock-turn"); // smart is the default
    assert_eq!(voices, vec!["mock", "mock-2"]);

    t.audio(200, false).await;
    t.audio(600, true).await;
    t.audio(400, false).await;

    // Events from the VAD thread and the STT thread interleave freely; only
    // per-source order and "turn.ended last" are guaranteed.
    let mut evs = Vec::new();
    while !matches!(evs.last(), Some(ServerEvent::TurnEnded { .. })) {
        evs.push(
            t.next_event(Duration::from_secs(5))
                .await
                .expect("event before turn.ended"),
        );
    }
    let ServerEvent::SpeechStarted { at_ms } = &evs[0] else {
        panic!("first event {:?}", evs[0])
    };
    assert!((200..=240).contains(at_ms), "speech started at {at_ms}");
    assert!(evs.contains(&ServerEvent::TranscriptDelta {
        segment_id: "seg-1".into(),
        text: "hello".into(),
        is_final: false
    }));
    let pos = |kind: &str| {
        evs.iter()
            .position(|e| e.kind() == kind)
            .unwrap_or_else(|| panic!("no {kind}"))
    };
    assert!(pos("speech.stopped") < pos("turn.ended"));
    assert_eq!(
        evs[pos("transcript.final")],
        ServerEvent::TranscriptFinal {
            segment_id: "seg-1".into(),
            text: "hello there".into()
        }
    );
    // Smart Turn (mock p = 0.9) ends the turn at the speech stop.
    let ServerEvent::TurnEnded { text, confidence } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(text, "hello there");
    assert!((confidence - 0.9).abs() < 1e-6);

    // The app streams the reply back token by token.
    for tok in ["Hi", "! How can", " I help", " you today?"] {
        t.send(ClientMessage::SpeakDelta {
            id: "r1".into(),
            text: tok.into(),
        })
        .await;
    }
    t.send(ClientMessage::SpeakDone { id: "r1".into() }).await;
    assert_eq!(
        t.expect("speak.started").await,
        ServerEvent::SpeakStarted { id: "r1".into() }
    );
    assert_eq!(
        t.expect("speak.ended").await,
        ServerEvent::SpeakEnded { id: "r1".into() }
    );
    assert_eq!(engine.spoken(), vec!["Hi!", "How can I help you today?"]);
    // 28 speakable chars × 20 ms at 24 kHz PCM16, every sample delivered.
    let chars = "Hi!".len() + "How can I help you today?".len();
    assert_eq!(t.take_audio(), chars * 20 * 24 * 2);
}

#[tokio::test]
async fn vad_mode_waits_for_silence() {
    let engine = MockEngine::new(MockConfig {
        transcripts: vec!["one".into(), "two".into()],
        turn_probability: None,
        ..Default::default()
    });
    let mut t = Harness::new(engine, CoreConfig::default());
    let ServerEvent::SessionReady { models, .. } = t
        .start(SessionOptions {
            turn: vad_turn(400),
            ..Default::default()
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(models.turn, "vad");

    t.audio(400, true).await;
    t.audio(300, false).await; // 100 ms VAD hangover + 200 ms: not yet 400 ms
    t.audio(300, true).await; // speech resumes: same turn
    t.audio(300, false).await;
    let evs = t.drain(Duration::from_millis(400)).await;
    assert!(
        !kinds(&evs).contains(&"turn.ended"),
        "turn ended early: {:?}",
        kinds(&evs)
    );
    assert_eq!(
        kinds(&evs)
            .iter()
            .filter(|k| **k == "transcript.final")
            .count(),
        2
    );

    t.audio(300, false).await;
    let ServerEvent::TurnEnded { text, confidence } = t.expect("turn.ended").await else {
        unreachable!()
    };
    assert_eq!(text, "one two");
    assert_eq!(confidence, 1.0);
}

#[tokio::test]
async fn smart_mode_low_probability_waits_for_silence() {
    let engine = MockEngine::new(MockConfig {
        transcripts: vec!["so I was thinking".into()],
        turn_probability: Some(0.2),
        ..Default::default()
    });
    let mut t = Harness::new(engine, CoreConfig::default());
    t.start(SessionOptions {
        turn: Some(TurnOptions {
            mode: Some(TurnMode::Smart),
            silence_ms: Some(600),
        }),
        ..Default::default()
    })
    .await;
    t.audio(400, true).await;
    t.audio(400, false).await;
    let evs = t.drain(Duration::from_millis(300)).await;
    assert!(!kinds(&evs).contains(&"turn.ended"));
    t.audio(400, false).await;
    let ServerEvent::TurnEnded { confidence, .. } = t.expect("turn.ended").await else {
        unreachable!()
    };
    assert!((confidence - 0.2).abs() < 1e-6);
}

#[tokio::test]
async fn smart_without_model_falls_back_to_vad_and_says_so() {
    let engine = MockEngine::new(MockConfig {
        turn_probability: None,
        ..Default::default()
    });
    let mut t = Harness::new(engine, CoreConfig::default());
    let ServerEvent::SessionReady { models, .. } = t.start(SessionOptions::default()).await else {
        unreachable!()
    };
    assert_eq!(models.turn, "vad");
    let ServerEvent::Error { code, fatal, .. } = t.expect("error").await else {
        unreachable!()
    };
    assert_eq!((code.as_str(), fatal), ("turn_unavailable", false));
}

#[tokio::test]
async fn barge_in_stops_speech_and_drops_queue() {
    let engine = MockEngine::new(MockConfig {
        transcripts: vec!["wait stop".into()],
        tts_ms_per_char: 40,
        ..Default::default()
    });
    let config = CoreConfig {
        playback_lead_ms: 100,
        ..Default::default()
    };
    let mut t = Harness::new(engine.clone(), config);
    t.start(SessionOptions {
        turn: vad_turn(300),
        ..Default::default()
    })
    .await;

    let reply = "This is a long answer that takes a while to say. It has a second sentence too. And a third.";
    t.send(ClientMessage::SpeakDelta {
        id: "r1".into(),
        text: reply.into(),
    })
    .await;
    t.send(ClientMessage::SpeakDone { id: "r1".into() }).await;
    t.expect("speak.started").await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    // The user talks over the bot.
    let spoke_at = Instant::now();
    t.audio(100, true).await;
    t.expect("speech.started").await;
    let ServerEvent::SpeakInterrupted { id, sent_ms } = t.expect("speak.interrupted").await else {
        unreachable!()
    };
    let latency = spoke_at.elapsed();
    assert_eq!(id, "r1");
    assert!(
        latency < Duration::from_millis(200),
        "interrupt took {latency:?}"
    );

    let total_ms = reply.chars().count() as u64 * 40;
    // Paced: ≈ 400 ms played + 100 ms lead were sent, not the whole reply.
    assert!(
        (300..=800).contains(&sent_ms),
        "sentMs {sent_ms} of {total_ms}"
    );
    let bytes_before = t.take_audio();
    assert_eq!(
        bytes_before as u64,
        sent_ms * 24 * 2,
        "sentMs matches audio actually sent"
    );

    // Nothing more for r1: no audio, no speak.ended, late deltas ignored.
    t.send(ClientMessage::SpeakDelta {
        id: "r1".into(),
        text: " More text.".into(),
    })
    .await;
    let evs = t.drain(Duration::from_millis(400)).await;
    assert_eq!(t.take_audio(), 0, "audio after speak.interrupted");
    assert!(
        !kinds(&evs).iter().any(|k| k.starts_with("speak.")),
        "{:?}",
        kinds(&evs)
    );

    // The user's turn still completes normally.
    t.audio(500, false).await;
    let ServerEvent::TurnEnded { text, .. } = t.expect("turn.ended").await else {
        unreachable!()
    };
    assert_eq!(text, "wait stop");
}

#[tokio::test]
async fn barge_in_disabled_keeps_speaking() {
    let engine = MockEngine::new(MockConfig {
        tts_ms_per_char: 10,
        ..Default::default()
    });
    let mut t = Harness::new(engine, CoreConfig::default());
    t.start(SessionOptions {
        barge_in: Some(false),
        turn: vad_turn(300),
        ..Default::default()
    })
    .await;
    t.send(ClientMessage::SpeakDelta {
        id: "r1".into(),
        text: "Keep talking please, all the way.".into(),
    })
    .await;
    t.send(ClientMessage::SpeakDone { id: "r1".into() }).await;
    t.expect("speak.started").await;
    t.audio(100, true).await;
    t.expect("speech.started").await;
    let ended = t.expect("speak.ended").await;
    assert_eq!(ended, ServerEvent::SpeakEnded { id: "r1".into() });
}

#[tokio::test]
async fn mute_ignores_audio_and_discards_partial_turn() {
    let engine = MockEngine::new(MockConfig {
        transcripts: vec!["muted half".into(), "after unmute".into()],
        turn_probability: None,
        ..Default::default()
    });
    let mut t = Harness::new(engine, CoreConfig::default());
    t.start(SessionOptions {
        turn: vad_turn(200),
        ..Default::default()
    })
    .await;

    // Speech, then mute mid-utterance: the partial turn is dropped.
    t.audio(300, true).await;
    t.expect("speech.started").await;
    t.send(ClientMessage::MicMute).await;
    t.expect("speech.stopped").await;
    t.audio(500, true).await; // ignored while muted
    t.audio(500, false).await;
    let evs = t.drain(Duration::from_millis(400)).await;
    assert!(
        !kinds(&evs)
            .iter()
            .any(|k| matches!(*k, "speech.started" | "turn.ended" | "transcript.final")),
        "events while muted: {:?}",
        kinds(&evs)
    );

    t.send(ClientMessage::MicUnmute).await;
    t.audio(300, true).await;
    t.audio(400, false).await;
    let ServerEvent::SpeechStarted { at_ms } = t.expect("speech.started").await else {
        unreachable!()
    };
    // The mic clock kept running while muted (300 + 500 + 500 ms).
    assert!((1300..=1340).contains(&at_ms), "atMs {at_ms}");
    // The aborted segment was never decoded, so this turn gets the first
    // scripted transcript.
    let ServerEvent::TurnEnded { text, .. } = t.expect("turn.ended").await else {
        unreachable!()
    };
    assert_eq!(text, "muted half");
}

#[tokio::test]
async fn cancel_stops_current_and_queued_utterances() {
    let engine = MockEngine::new(MockConfig {
        tts_ms_per_char: 40,
        ..Default::default()
    });
    let mut t = Harness::new(
        engine,
        CoreConfig {
            playback_lead_ms: 100,
            ..Default::default()
        },
    );
    t.start(SessionOptions::default()).await;

    t.send(ClientMessage::SpeakDelta {
        id: "a".into(),
        text: "First reply is fairly long to say out loud.".into(),
    })
    .await;
    t.send(ClientMessage::SpeakDone { id: "a".into() }).await;
    t.send(ClientMessage::SpeakDelta {
        id: "b".into(),
        text: "Second reply.".into(),
    })
    .await;
    t.send(ClientMessage::SpeakDone { id: "b".into() }).await;
    assert_eq!(
        t.expect("speak.started").await,
        ServerEvent::SpeakStarted { id: "a".into() }
    );

    // Cancel the queued one by id, then the current one without an id.
    t.send(ClientMessage::SpeakCancel {
        id: Some("b".into()),
    })
    .await;
    assert_eq!(
        t.expect("speak.ended").await,
        ServerEvent::SpeakEnded { id: "b".into() }
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    t.send(ClientMessage::SpeakCancel { id: None }).await;
    assert_eq!(
        t.expect("speak.ended").await,
        ServerEvent::SpeakEnded { id: "a".into() }
    );
    let sent = t.take_audio();
    assert!(sent > 0 && sent < 43 * 40 * 24 * 2, "sent {sent}");

    let evs = t.drain(Duration::from_millis(300)).await;
    assert_eq!(t.take_audio(), 0);
    assert!(evs.is_empty(), "{:?}", kinds(&evs));
}

#[tokio::test]
async fn resamples_8k_and_48k_input() {
    for rate in [8_000u32, 48_000] {
        let engine = MockEngine::new(MockConfig {
            transcripts: vec!["phone".into()],
            turn_probability: None,
            ..Default::default()
        });
        let mut t = Harness::new(engine, CoreConfig::default());
        t.start(SessionOptions {
            input_sample_rate: Some(rate),
            turn: vad_turn(200),
            ..Default::default()
        })
        .await;
        t.audio(500, false).await;
        t.audio(400, true).await;
        t.audio(400, false).await;
        let ServerEvent::SpeechStarted { at_ms } = t.expect("speech.started").await else {
            unreachable!()
        };
        assert!((500..=540).contains(&at_ms), "{rate} Hz: atMs {at_ms}");
        let ServerEvent::SpeechStopped { at_ms } = t.expect("speech.stopped").await else {
            unreachable!()
        };
        // 900 ms of audio + 100 ms mock hangover, ± filter delay.
        assert!(
            (990..=1040).contains(&at_ms),
            "{rate} Hz: stop atMs {at_ms}"
        );
        t.expect("turn.ended").await;
    }
}

#[tokio::test]
async fn output_rate_override_for_phone_legs() {
    let engine = MockEngine::new(MockConfig::default());
    let mut t = Harness::new(
        engine,
        CoreConfig {
            output_sample_rate: Some(8_000),
            ..Default::default()
        },
    );
    let ServerEvent::SessionReady {
        output_sample_rate, ..
    } = t.start(SessionOptions::default()).await
    else {
        unreachable!()
    };
    assert_eq!(output_sample_rate, 8_000);
    t.send(ClientMessage::SpeakDelta {
        id: "x".into(),
        text: "Hello.".into(),
    })
    .await;
    t.send(ClientMessage::SpeakDone { id: "x".into() }).await;
    t.expect("speak.ended").await;
    // 6 chars × 20 ms at 8 kHz PCM16, within resampler rounding.
    let got = t.take_audio() as i64;
    assert!((got - 6 * 20 * 8 * 2).abs() <= 8, "bytes {got}");
}

#[tokio::test]
async fn protocol_errors() {
    let engine = MockEngine::new(MockConfig::default());
    let mut t = Harness::new(engine.clone(), CoreConfig::default());
    // Audio before session.start.
    t.audio(20, false).await;
    let ServerEvent::Error { code, fatal, .. } = t.expect("error").await else {
        unreachable!()
    };
    assert_eq!((code.as_str(), fatal), ("not_started", false));
    t.h.input
        .send(SessionInput::BadMessage("nope".into()))
        .await
        .unwrap();
    let ServerEvent::Error { code, .. } = t.expect("error").await else {
        unreachable!()
    };
    assert_eq!(code, "bad_message");

    // A bad option on session.start is fatal and closes the session.
    t.send(ClientMessage::SessionStart(SessionOptions {
        voice: Some("nobody".into()),
        ..Default::default()
    }))
    .await;
    let ServerEvent::Error { code, fatal, .. } = t.expect("error").await else {
        unreachable!()
    };
    assert_eq!((code.as_str(), fatal), ("bad_option", true));
    assert_eq!(
        t.next(Duration::from_secs(2)).await,
        Some(SessionOutput::Close)
    );

    // A bad option on session.update is not.
    let mut t = Harness::new(engine, CoreConfig::default());
    t.start(SessionOptions::default()).await;
    t.send(ClientMessage::SessionUpdate(SessionOptions {
        stt_model: Some("huge".into()),
        ..Default::default()
    }))
    .await;
    let ServerEvent::Error { code, fatal, .. } = t.expect("error").await else {
        unreachable!()
    };
    assert_eq!((code.as_str(), fatal), ("bad_option", false));
    t.send(ClientMessage::SessionEnd).await;
    assert_eq!(
        t.next(Duration::from_secs(2)).await,
        Some(SessionOutput::Close)
    );
}

#[tokio::test]
async fn unavailable_engine_refuses_clearly() {
    let (tx, mut rx) = {
        let h = VoiceSession::spawn(
            Arc::new(voice_service::session::engine::UnavailableEngine),
            CoreConfig::default(),
        );
        (h.input, h.output)
    };
    tx.send(SessionInput::Control(ClientMessage::SessionStart(
        SessionOptions::default(),
    )))
    .await
    .unwrap();
    let Some(SessionOutput::Event(ServerEvent::Error {
        code,
        fatal,
        message,
    })) = rx.recv().await
    else {
        panic!("expected error")
    };
    assert_eq!((code.as_str(), fatal), ("engine_unavailable", true));
    assert!(message.contains("no speech engine"));
    assert_eq!(rx.recv().await, Some(SessionOutput::Close));
}
