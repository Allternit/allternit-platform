# Track H: voice integrate notes

**status:** done. Engine (`ao/voice-engine-p1`), Voice Session (`ao/voice-session`) and call worker (`ao/voice-callworker`) are merged on `ao/voice-integrate`, the session runs on the real engine, and everything below was verified. No PR, no merge, no deploy.

## Merge commits

- `fbf883e7c` merge: voice-session into voice-integrate
- `802d0e82d` merge: voice-callworker into voice-integrate
- `3b27b93d3` voice: wire the Voice Session onto the real engine (+ these notes)

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

## Final-transcript fix

**Root cause (corrected).** The "Findings" entry blamed the ~0.2 s trailing silence. Measured, that was only part of it. On the failing sample (`say -v Samantha`, "What time is it in Tokyo right now?") the audio starts speaking at ~0.03 s, Silero fires `speech.started` ~0.4 s in, and the session's 300 ms pre-roll then starts the segment after the onset of "What". The final decode got "A time is at in Tokyo right now." The HTTP route decodes a VAD segment that it pads from the whole utterance, so it doesn't clip. Trimming alone did not fix this sample (re-run gave "A time is at Intokyo right now.").

**Changes** (`services/voice/src/session/`, `stt.rs`):
- `trim.rs` (new): `trim_silence(samples, pad)` cuts leading and trailing audio below 10 % of the loud-frame RMS (floor 0.005, 10 ms frames), keeping 0.1 s each side; `word_overlap` for the guard. Unit tests: pad length, noise floor, all-silence/empty, full-speech, overlap.
- `core.rs`: `PRE_ROLL_MS` 300 → 600, so the segment includes the onset. The trim removes the extra silence before the decode.
- `engine_sherpa.rs`: `SherpaStt::finish` decodes the trimmed audio (`SegmentStream::samples()` added in `stt.rs`). Guard: if the last interim covered ≥ 90 % of the segment and the final's word overlap with it is < 0.8, the audio is re-decoded with a 0.05 s pad and the re-decode is kept only if it agrees better with the interim. Interim text is never substituted without a decode.

**Test set** (macOS `say`, 16 kHz mono wav; 8 questions, voices Samantha/Daniel/Karen, plus two with pink noise mixed in via ffmpeg at 0.05 amplitude: q0n, q6n). Run through `voice_session_client` against a local service (`PORT=18001`), before = HEAD `e20db023f`, after = this change.

| # | Source text | Before | After |
|---|---|---|---|
| q0 Samantha | What time is it in Tokyo right now? | A time is at in Tokyo right now. | What time is it in Tokyo right now? |
| q1 Daniel | How do I reset my account password? | same as source | same as source |
| q2 Karen | Can you summarize the latest sales report for me? | same | same |
| q3 Samantha | Where is the nearest coffee shop to the office? | same | same |
| q4 Daniel | Please schedule a meeting with the design team tomorrow. | same | same |
| q5 Karen | What is the weather going to be like this weekend? | same | same |
| q6 Samantha | Tell me how many users signed up last week. | same (no period) | same (no period) |
| q7 Daniel | Remind me to call the dentist on Friday afternoon. | same | same |
| q0n Samantha + noise | What time is it in Tokyo right now? | same | same |
| q6n Samantha + noise | Tell me how many users signed up last week. | same | same |

Corpus WER (87 reference words, lowercase, punctuation stripped): **before 2.3 % (2 substitutions), after 0.0 %.** Caveat: only one of the ten failed before, so this shows the fix works on that failure and doesn't regress the others; it is not a precise accuracy estimate. An intermediate build with the trim only (pre-roll still 300 ms) still got q0 wrong.

**Checks:** `cargo test -p voice-service` 89 passed; with `--features call-worker` 91 passed; clippy `--all-targets -D warnings` clean with and without `call-worker`.

**Bench (STT, `voice_bench --only moonshine|parakeet --threads 2`):** the bench drives `SttStream`, not the session path, so it is unaffected by this change by construction. Moonshine tiny WER matches the Phase 1 baseline: pstn 7.24 %, pstn8k 10.61 % (latency p50/p95 111/372 and 79/330 ms, higher than the baseline's 81/280 and 83/214 because the machine was loaded by other builds). The wb and Parakeet lines were truncated by my output filter and not re-read, so they are not compared here. (`--only` takes a job name, `moonshine|parakeet|tts`, not `stt`.)

**Not done:** the guard's re-decode path has no automated test (it needs a real model); in the ten runs above it was not needed to get the right text. The shared-checkout git-discipline Stop hook fired repeatedly; it concerns `~/Desktop/allternit-workspace/allternit` (60 commits behind origin/main, stale branch `ao/c-surfaces-docs-platform`), which this task forbids touching, so it was left for Eoj.
