# Track H: voice integrate notes

**status:** done. Engine (`ao/voice-engine-p1`), Voice Session (`ao/voice-session`) and call worker (`ao/voice-callworker`) are merged on `ao/voice-integrate`, the session runs on the real engine, and everything below was verified. No PR, no merge, no deploy.

## Merge commits

- `fbf883e7c` merge: voice-session into voice-integrate
- `802d0e82d` merge: voice-callworker into voice-integrate
- the wiring commit follows these two (see `git log`).

## Conflicts and how they were resolved

| File | Conflict | Resolution |
|---|---|---|
| `services/voice/Cargo.toml` | dependency blocks on both sides, both merges | Took the engine's deps; added the session's `ort` (now a plain dep, no `sherpa` feature) and the call worker's `futures`, `livekit-protocol`, `prost`, `tokio-tungstenite`, optional `livekit`/`livekit-api` and the `call-worker` feature. One `sha2`, one `futures-util`. `tokio-tungstenite` is a regular dep now, so the session's dev-dep entry was dropped. |
| `src/lib.rs` | mod list | `audio, call_worker, models, server, session, stt, tts` with the engine's docs header. |
| `src/server.rs` | session rewrote `VoiceServiceState::new` | Took the engine's `new`/`with_packs`; the session route is added in `create_router` (see wiring). |
| `src/main.rs` | none textually | Verified all three survive: `into_make_service_with_connect_info::<SocketAddr>()`, `worker`/`serve` subcommands, the `no_espeak` shim. |
| `src/client.rs` | modify/delete (the engine deleted it, both branches touched it) | Deleted; nothing references it. |
| `Cargo.lock` | content | Took ours and let cargo regenerate it. |
| `surfaces/docs/tools/voice.mdx` | "Related pages" list | Kept both links (Voice API, Voice Calls). |
| `surfaces/docs/api/voice.mdx`, notes/task docs | none | auto-merged; every side's sections kept. |

## Wiring changes (Voice Session onto the real engine)

- `engine_sherpa` is always compiled: the `sherpa` feature is gone, `ort` is a plain dep (`sha2` was already one). `UnavailableEngine` stays, as a test placeholder.
- `server::create_router` builds the session route with `SherpaEngine::new(state.packs(), state.stt(), state.tts())`, so the session shares the server's engines and loads no second copy. `ALLTERNIT_VOICE_TOKEN` still gates it. `SessionRouteState::from_env`/`ws::router` were removed (they loaded a second engine).
- STT: `SttEngine::segment_stream` (VAD-less) with the same 0.6 s interim pacing. `transcribe_segment` is no longer used by the session.
- VAD: `SttEngine::new_vad_with(VadConfig { min_silence: 0.2, min_speech: 0.1, .. })`. The adapter feeds it in `VAD_WINDOW_SAMPLES` pieces (the engine's onset rule needs ≤ 512 samples per call).
- TTS: `TtsEngine::synthesize_stream` (the `allternit-tts` child), with each rendered chunk re-framed into 100 ms pieces for the sink; returning `false` from the sink stops after the current chunk. Kokoro renders a sentence in one pass, so the unit of latency is the first chunk (the engine already cuts the first sentence at its first clause).
- Smart Turn: the model is now read from the `small` pack (`SMART_TURN_FILE`, pin and download owned by `PackManager`). The session's own downloader and pin were deleted from `turn.rs`.
- `session.ready` now reports `tts: kokoro-multi-lang-v1_0` (it said the old v0.19).
- `examples/voice_session_client.rs`: added `--barge-wav FILE` / `--barge-after-ms N` (speak over the reply) and a "first speech audio" timeline line.
- `spec/VOICE_SESSION.md` code map updated.

## Verification

```
cargo test -p voice-service
test result: ok. 84 passed (lib)   | ok. 20 passed (integration; model tests skip without the flag)
test result: ok. 12 passed (voice_session) | ok. 2 passed (voice_session_ws)

cargo test -p voice-service --features call-worker
test result: ok. 86 passed (lib)   | ok. 20 | ok. 12 | ok. 2

cargo clippy -p voice-service --all-targets -- -D warnings                       -> Finished, no warnings
cargo clippy -p voice-service --all-targets --features call-worker -- -D warnings -> Finished, no warnings
cargo test -p allternit-tts   -> 0 tests

cargo build --release -p voice-service -p allternit-tts  -> Finished
scripts/check-voice-no-gpl.sh target/release/voice-service
check-voice-no-gpl: OK: no espeak-ng in .../release/voice-service

python3 surfaces/docs/scripts/check_links.py  -> 415 nav entries, 412 pages: 0 problem(s)
```

## End to end (real models: small + tts packs, release build, port 18001)

Port 8001 was held by the installed Desktop app's old whisper service, so the test service ran on 18001. Question: `say "What time is it in Tokyo right now?"` → 16 kHz mono wav. Times are client ms; run 1 includes the cold model load (`session.ready` at 3.1 s), runs 2–3 are warm.

Run 2 (warm):

```
 +675  speech.started (atMs 400)
+1030  transcript.delta "Our time is as."  -> 1602 "Our time is at in Tokyo." -> 2217 "What time is it in Tokyo right now?"
+2560  speech.stopped (atMs 2280)
+2658  transcript.final "A time is at in Tokyo right now."
+2742  turn.ended confidence 0.984   (≈ 180 ms after speech.stopped; ≈ 100 ms after the final)
+5131  speak.started + first speech audio  (2.4 s after turn.ended)
+7005  speak.ended  -> reply.wav 2.13 s at 24 kHz, peak 14165 (real speech)
```

Cold run: turn.ended +5705, speak.started +8481 (2.8 s, TTS child start + Kokoro load), speak.ended +10355.
Another warm run: turn.ended +2794 → first audio +4399 (1.6 s; the first sentence's text had to finish arriving, then render).

Barge-in (long reply; the question WAV "Stop, wait a second." spoken 800 ms after speak.started):

```
+7271  speak.started / first audio
+8075  barge audio starts
+8443  speech.started (atMs 4426)  and  speak.interrupted {sentMs: 1060} at the same ms (368 ms after the barge audio began)
+8797  speech.stopped, +9058 speech.started (the barge speech continues)
reply3.wav: 1.06 s of speech, nothing after the interrupt
```

## Findings

- **Final transcript worse than the last interim.** The interim was right ("What time is it in Tokyo right now?") but the final was "A time is at in Tokyo right now." The same audio through `POST /v1/stt` (which VAD-segments it with the HTTP route's own tuning) gives the right text. The final decodes the whole segment, including the ~0.2 s of trailing silence the turn-taking VAD needs, with Moonshine tiny; it is deterministic across runs. Not fixed here. Next step: trim trailing silence from the segment before the final decode, or re-use the last good interim when the final disagrees; measure on a real-voice WAV set rather than one `say` sample.
- **TTS first-audio latency is the long pole:** 1.6–2.4 s warm from `turn.ended`, 2.8 s cold, with fp32 Kokoro in the child process (the sentence must also finish arriving, ~30 ms per word in the test client).
- **Port clash:** a dev machine with the Desktop app running holds 8001; use `PORT=` for local runs.

Build output (`.voice-target-h`) and `/tmp/h-e2e` were deleted after the run.
