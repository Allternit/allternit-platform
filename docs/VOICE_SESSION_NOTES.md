# Track B: Voice Session notes

**status:** done. The Voice Session core, WebSocket route, Smart Turn, tests, CLI client and docs are complete and verified. The sherpa adapter is written against `ao/voice-engine-p1`, but it can't be compiled against that branch yet, because the branch's WIP head doesn't compile on its own (see "Wiring once Phase 1 merges").

## Commits (branch `ao/voice-session`, pushed; no PR, no merge)

- `6c88cfb29` voice: Voice Session core, WS route, Smart Turn, mock engine + tests
- `08dd500ab` voice: voice_session_client example (WAV in, events out, speech to WAV)
- `b2c4f51a3` docs: Voice Session protocol spec in repo + Mintlify Voice Session section
- `796c2d448` voice: Smart Turn warm-up at load, fix ort error mapping, log turn timing
- (this notes commit)

## Files touched (`git diff --stat origin/main...HEAD`)

New:
- `services/voice/src/session/{mod,core,engine,engine_sherpa,mock,protocol,resample,sentence,turn,ws}.rs`
- `services/voice/tests/voice_session.rs` (12 core scenarios)
- `services/voice/tests/voice_session_ws.rs` (2 WebSocket tests)
- `services/voice/examples/voice_session_client.rs`
- `services/voice/spec/VOICE_SESSION.md`: the protocol copied verbatim, plus an "Implementation notes" appendix
- `docs/VOICE_SESSION_NOTES.md`

Edited (kept small for the merge):
- `services/voice/src/lib.rs`: `pub mod session;`
- `services/voice/src/server.rs`:
  - one line, `.merge(crate::session::ws::router())`, after `.with_state(state)`;
  - plus a clippy fix (`vec![]` instead of push-after-new) in `VoiceServiceState::new`, a block Phase 1 rewrites anyway. **On merge, take Phase 1's version of that hunk.**
- `services/voice/src/main.rs`: serves with `into_make_service_with_connect_info::<SocketAddr>()`, needed for the loopback check.
- `services/voice/src/client.rs`: `VoiceClient::default()` is now `impl Default` (a pre-existing clippy error).
- `services/voice/Cargo.toml`:
  - `futures-util`;
  - optional `ort =2.0.0-rc.13` (`alternative-backend`, `api-24`) and `sha2`;
  - feature `sherpa = ["dep:ort", "dep:sha2"]`;
  - dev-dep `tokio-tungstenite`.
- `Cargo.lock`
- `surfaces/docs/api/voice.mdx`: a new "Voice Session (realtime)" section, inserted before "Related pages". No other section was touched.

## Test and clippy output (last lines)

```
cargo test -p voice-service
test result: ok. 19 passed; 0 failed   (lib unit: protocol, sentence, resample, turn features/FFT)
test result: ok. 8 passed; 0 failed    (existing tests/integration.rs)
test result: ok. 12 passed; 0 failed   (tests/voice_session.rs)
test result: ok. 2 passed; 0 failed    (tests/voice_session_ws.rs)

cargo clippy -p voice-service --all-targets -- -D warnings
(no output) exit 0

python3 surfaces/docs/scripts/check_links.py
checked 414 nav entries, 411 pages: 0 problem(s)
```

The core scenarios were stable across 3 consecutive runs.

The core scenarios cover:
- a simple turn, then a token-streamed reply, with every audio sample delivered;
- `vad` mode waits for silence, and resumed speech continues the same turn;
- `smart` with low probability waits `silenceMs`;
- `smart` with no model falls back to `vad` with a `turn_unavailable` error;
- **barge-in**: `speak.interrupted` arrives in under 200 ms; `sentMs` equals the audio actually sent (paced, so about 500 ms of a 3.7 s reply); no audio after the interrupt; late deltas for the id are ignored; the user's turn still ends;
- barge-in disabled;
- mute, which ignores audio, discards the partial turn and keeps the `atMs` clock running;
- cancel of a queued and a current utterance;
- 8 kHz and 48 kHz input resampling, checked via `atMs`;
- forced 8 kHz output for phone legs;
- protocol errors;
- an engine-less build refusing clearly.

The WebSocket tests run a real `tokio-tungstenite` client through the full flow `session.start` → `session.ready` → binary mic → `turn.ended` → `speak.delta`/`speak.done` → `speak.started` + binary speech + `speak.ended` → `session.end` → close. They also cover the auth rules: token, missing or wrong token, loopback, the default ticket rejection, and a plugged-in verifier.

## Smart Turn: `smart` is the default, and it works

- **Model:** `smart-turn-v3.2-cpu.onnx`, BSD-2-Clause.
  - The pipecat-ai/smart-turn repo has **no GitHub release assets**. The weights are published on Hugging Face, so the pin is `huggingface.co/pipecat-ai/smart-turn-v3` at revision `f766f81d3cfdf7737ac64aad813d91bbfd56bf93`.
  - sha256 `2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f`, 8,679,182 bytes.
  - The download is verified before it's written (`turn::SmartTurn::ensure_model`).
- **Runtime:** sherpa-onnx 1.13.8 links onnxruntime **1.28.2 statically**; there's no dylib. The sherpa-onnx crate has no generic session API. So `ort` 2.0.0-rc.13 is built with `alternative-backend`, which links nothing, and pointed at the linked runtime through `OrtGetApiBase()` → `ort::set_api`. Result: one onnxruntime in the binary (`otool -L` shows no onnxruntime dylib).
- **Features:** the Whisper log-mel (80×800, Slaney mel, periodic Hann, reflect-pad, normalised) is in pure Rust (`turn::WhisperFeatures`), with a mixed-radix FFT checked against a naive DFT. No new runtime deps.
- **Validated on real speech** (macOS `say`, 16 kHz), in a scratch probe that compiles these exact files with real Silero VAD and real Smart Turn:

  | Phrase | p(end of turn) |
  |---|---|
  | "What time is it in Tokyo right now?" | 0.890 |
  | "Please book me a table for two at seven." | 0.988 |
  | "I was wondering if you could maybe" | 0.069 |
  | "Can you tell me about the" | 0.024 |

- **End to end on real audio:** `voice_session_client` streamed a 48 kHz spoken question in real time to the probe server (real Silero VAD + Smart Turn, mock STT/TTS). Every event was correct: `speech.started`, interims, `speech.stopped`, `transcript.final`, `turn.ended` p=0.957, then a paced reply with all 1.46 s of speech received.
- **Latency:** `speech.stopped` → `turn.ended` was **267 ms**, all of it Smart Turn inference, while this Mac was at load average 53–71 on 10 cores from parallel agent builds. Features take about 20 ms. The first ONNX run is slow (~1 s), so `SmartTurn::load` now warms up once.
  - The protocol target (≤ 300 ms after the user stops) also includes the VAD's 200 ms `min_silence`. So under that load it's about 470 ms, and the idle 2-core target-machine number must come from the Phase 1 bench. Smart Turn uses 1 intra-op thread.

## Wiring once Phase 1 (`ao/voice-engine-p1`) merges

The Phase 1 head (`adf5faa0a` WIP + `33bcd6de1` handover) doesn't compile on its own. Its `services/voice/src` has these problems:
- `stt.rs` lacks `use std::sync::Arc`;
- `stt.rs:176` and `tts.rs:197` use `.and_then(|g| g.is_some())`, which should be `.map(...)`;
- `models.rs:258` `bytes_stream()` needs reqwest's `stream` feature;
- `PackManager` isn't `Clone`, but its own `server.rs` calls `packs.clone()`.

`engine_sherpa.rs` assumes `PackManager: Clone`, like that server code. After both land:

1. **Turn the adapter on.** Phase 1 makes `sherpa-onnx` a non-optional dependency. So either add `sherpa` to `default` features, or drop the `#[cfg(feature = "sherpa")]` gates and make `ort` and `sha2` plain deps (Phase 1 already adds `sha2 = "0.10"`; dedupe it). Until then, `cargo clippy --all-features` can't compile on this branch alone, because `engine_sherpa` needs `crate::{stt,tts,models}`.
2. **Share the loaded engines.** Replace `.merge(crate::session::ws::router())` with `router_with(SessionRouteState::new(Arc::new(SherpaEngine::new(packs, state.stt.clone(), state.tts.clone())), std::env::var(TOKEN_ENV).ok()))`, using the server's `Arc<SttEngine>`/`Arc<TtsEngine>`. Otherwise `SherpaEngine::from_env()` loads a second copy of every model.
3. **STT.** The adapter uses `SttEngine::transcribe_segment` per VAD segment:
   - it re-decodes the growing segment for an interim every 0.6 s, up to 12 s;
   - the final is one decode at segment end.

   Phase 1 now also has `SttStream` (`begin_stream`/`feed`/`partial`/`finish`), but that runs its **own** VAD inside. Using it here would mean two VADs. Either add a VAD-less streaming entry point (feed samples, ask for a partial, finish), or keep `transcribe_segment`. `language` is ignored today, since Moonshine tiny is English-only.
4. **VAD.** `stt::build_vad` is private, so the adapter builds its own Silero instance from the small pack's `silero_vad.onnx`, tuned for turn-taking: `min_silence 0.2 s`, `min_speech 0.1 s`. Phase 1 could expose a builder that takes these values.
5. **TTS.** `TtsEngine::synthesize` returns a whole sentence, which the adapter chunks afterwards. Passing sherpa's `generate_with_config` progress callback through would cut first-audio latency on long sentences.
6. **Smart Turn download.** It currently downloads into `<model root>/smart-turn/` on its own. It should become a `PackFile` in Phase 1's pack manifest, with the same pin, so it gets that manager's progress reporting and `/v1/models` status.
7. **Expected merge conflicts.** All are mechanical:
   - `Cargo.toml` dependency lines;
   - `lib.rs` mod list;
   - `server.rs` (the `.merge` line, plus take Phase 1's `VoiceServiceState::new`);
   - `main.rs` serve line (keep `into_make_service_with_connect_info`);
   - `Cargo.lock` (regenerate);
   - `surfaces/docs/api/voice.mdx` (only if Phase 1 also edits the area just before "Related pages").
8. **Then verify end to end:** run the service, then `cargo run -p voice-service --example voice_session_client -- --wav q.wav --say "Reply text." --out reply.wav`.

## Points for the voice session's sign-off

These are implementation choices that clarify the protocol. They're written in the `spec/VOICE_SESSION.md` appendix and don't change the frozen table.
- `speak.cancel` is answered with `speak.ended {id}`.
- Barge-in sends `speak.interrupted` for every live utterance, including queued ones (`sentMs` 0), and later deltas for those ids are ignored.
- An unknown or unparseable client frame gets a non-fatal `error` with code `bad_message`.
- `atMs` is mic-audio time, muted audio included.
- `vad` mode's effective wait is the VAD's 200 ms `min_silence` plus `silenceMs`.
- Speech is paced at real time plus 250 ms of lead, so on barge-in the client flushes at most about 250 ms.

## Not in this track (for later phases)

- **Desktop sidecar token.** Desktop must pass `ALLTERNIT_VOICE_TOKEN` to the sidecar. Otherwise only loopback connections are allowed, which is fine for the sidecar but is the fallback rather than the design.
- **Cloud tickets.** The ticket verifier is a stub that rejects every ticket, with a clear 401. Cloud needs a verifier for allternit-api tickets.
- **Bind address.** `main.rs` binds 127.0.0.1, so a cloud deployment needs a bind-address setting (Phase 3).

Build output (`.voice-target-b`) and the scratch probe were deleted after the run.
