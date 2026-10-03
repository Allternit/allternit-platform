# Voice engine, Phase 1: real STT + TTS in `services/voice` on sherpa-onnx

You are a Kimi executor. The orchestrator (Claude session joe-9e) wrote this spec, reviews your work, and merges. Eoj approved the plan on 2026-10-02. The full plan is in `~/Desktop/allternit-workspace/HANDOFF-realtime-voice-2026-10-02.md` §8. Read §8 before you start.

## Budget (hard)

- Tool-call cap: **350**. At 300, stop and write the notes file with what's done and what's left.
- Work only in this worktree (`allternit-ao-voice-engine-p1`, branch `ao/voice-engine-p1`). Never touch the shared checkout `allternit/`.
- Rust builds use the shared target: `export CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.shared-target`. Do not create a `target/` here.
- Commit to the branch as you go (small commits). Push the branch when done. **Do not open or merge a PR.** The orchestrator does that.

## The standard (acceptance is measured against it)

Voice must run well on a ~5-year-old laptop: 8 GB RAM, 4 cores, no GPU, Windows/Mac/Linux.
- Models download on first use (not bundled): small pack ≤ ~150 MB.
- While running: ≤ ~1 GB RAM, ≤ **2 threads** for inference.
- STT finalises ≤ ~300 ms after end of speech for a typical 5–10 s utterance.
- TTS first audio chunk ≤ ~300 ms for the first sentence, faster than real time overall.

## What exists now (origin/main 646770988)

- `services/voice` (crate `voice-service`, binary shipped as `allternit-voice-service` in Desktop `resources/bin`):
  - `src/whisper.rs`: shells out to `whisper-cli` + `ggml-tiny.en`. One shot.
  - `src/server.rs`: axum routes. `/v1/stt` (multipart) works via whisper. `/v1/tts` and `/v1/tts/stream` are **stubs** that return metadata.
  - `README.md`, `spec/API.md`, `build-whisper.sh`, `tests/integration.rs`.
- Callers that must keep working: `surfaces/allternit-desktop/src/main/voice-manager.ts` (spawns the binary) and `scripts/build-desktop.sh` (builds it, copies `whisper-cli`). The `/voice` command in gizzi-code and Desktop calls `POST /v1/stt`. Grep for every caller of `/v1/stt`, `/v1/tts` and `/v1/voices` across this repo **and** `~/Desktop/allternit-workspace/allternit-ai` (read only there) before you change any response shape. Keep the shapes compatible.
- `docs/specs/tts-product.md` says "bake-off, then ship. Do not add a stub." The bake-off is done; this phase ships.

## Build this

1. **Engine:** add the official k2-fsa `sherpa-onnx` crate (crates.io 1.13.x, Apache-2.0). Do not use the community `sherpa-rs`. Every model runs with `num_threads = 2` (configurable via env `ALLTERNIT_VOICE_THREADS`), CPU provider.
2. **Model packs** (`src/models.rs`): download on first use into `~/.allternit/models/voice/<pack>/`.
   - Verify the sha256 of each file and keep a manifest with the hashes pinned in code.
   - Make the source base URL an env var (`ALLTERNIT_VOICE_MODEL_BASE`). For now it defaults to the upstream sherpa-onnx GitHub release assets; Phase 2 moves it to runtime.allternit.com.
   - Download is atomic (temp file then rename), resumes safely, and reports progress through `GET /v1/models` (state: missing | downloading{pct} | ready | error).
   - Packs:
     - `small` (default): Silero VAD + Moonshine (base or tiny, English) + Kokoro-82M int8 (en, the voices sherpa ships).
     - `accurate`: Parakeet TDT 0.6B v3 int8 (sherpa-onnx transducer export).
3. **STT** (`src/stt.rs`): Silero VAD segments the audio, then the offline recogniser (Moonshine or Parakeet, picked by pack or request `model`) transcribes each segment.
   - Accept any sample rate. Resample to 16 kHz. 8 kHz phone audio must work.
   - Replace the whisper path behind `POST /v1/stt` with the same response shape.
   - Make `/v1/stt/stream` real: chunked PCM in, partial and final segments out (SSE or NDJSON, whichever matches the existing types in `types.rs`; document it in `spec/API.md`).
4. **TTS** (`src/tts.rs`): Kokoro through sherpa-onnx.
   - `POST /v1/tts` returns real audio: WAV by default, plus a `format` param for pcm16.
   - `POST /v1/tts/stream` splits the text into sentences and streams each sentence's audio as soon as it's synthesised, so the first chunk isn't held back for the whole text.
   - `GET /v1/voices` lists the real Kokoro voices.
5. **Remove whisper completely** (fix-it-now rule): `src/whisper.rs`, `build-whisper.sh`, the whisper-cli build/copy lines in `scripts/build-desktop.sh`, any whisper references in `voice-manager.ts`, and the README and spec. Replace them with how the onnxruntime/sherpa shared libraries get packaged.
   - **Packaging check:** if the sherpa-onnx crate links onnxruntime dynamically, `build-desktop.sh` must copy the dylib/dll/so next to the binary, and the binary must find it (rpath / `@loader_path` on macOS, same dir on Windows). Prefer static linking if the crate offers it. State which you chose and why in the notes.
   - The Windows release build runs on GitHub's `release-desktop.yml` (never cross-compile on Mac). Make sure nothing in your change assumes macOS.
6. **Bench** (`services/voice/examples/voice_bench.rs`, `cargo run --release -p voice-service --example voice_bench -- <args>`):
   - `--data ~/.allternit/voice-bench` contains `wb/*.wav` (clean 16 kHz), `pstn/*.wav` (phone line: 300–3400 Hz band, 8 kHz μ-law, back to 16 kHz), `pstn/*.8k.wav` (raw 8 kHz) and `refs.json` (key → reference text, LibriSpeech test-clean).
   - For each STT model × condition, report: WER (lowercase, strip punctuation, hyphens become spaces), p50/p95 finalisation latency per utterance, real-time factor, and peak RSS.
   - For TTS, report time to first audio chunk for 10 fixed sentences, real-time factor, and peak RSS.
   - Run everything with 2 threads. Write JSONL to `services/voice/bench/results-<host>.jsonl` and commit the results from this Mac (M1 Pro) **CPU-only, 2 threads**.
   - Reference numbers from the GPU bake-off are in `~/.allternit/voice-bench/reference_m1pro_gpu.jsonl` (parakeet 2.46/2.72% WER, whisper-tiny 5.3/6.7%). Your CPU WER for Parakeet should land near 2.5–3%. If it's far off, investigate before you report.
7. **Tests:** update `tests/integration.rs`.
   - Real model tests go behind an env flag (`ALLTERNIT_VOICE_MODEL_TESTS=1`) so CI without models stays green.
   - Unit tests for the manifest/sha check, resampling and sentence splitting.
   - `cargo test -p voice-service` passes, and so does `cargo clippy -p voice-service -- -D warnings`.
8. **Docs** (same change):
   - `services/voice/README.md` and `spec/API.md`.
   - `docs/specs/tts-product.md`: mark what this phase shipped.
   - Mintlify `surfaces/docs/api/voice.mdx` and `surfaces/docs/tools/voice.mdx`: real STT and TTS, models download on first use, licences with attribution (Parakeet is CC-BY-4.0 and needs attribution; Kokoro Apache-2.0; Moonshine MIT; Silero MIT; sherpa-onnx Apache-2.0).
   - Add a `services/voice/THIRD_PARTY_NOTICES.md` with every model and library licence.
   - Run `python3 surfaces/docs/scripts/check_links.py` until it reports 0 problems.

## Not in this phase (don't build these)

Voice Session WebSocket protocol, Smart Turn, barge-in, LiveKit, SIP, cloud deploy, Pocket TTS, any UI, the WASM build, runtime.allternit.com hosting. Those are Phases 1b–4.

## Done = write `docs/VOICE_ENGINE_PHASE_1_NOTES.md` with

- **status:** done | partial
- the commits and pushed branch
- the bench table (paste the JSONL summary), and whether each number meets the standard above
- the linking/packaging choice for onnxruntime per OS
- every file changed, and every caller you checked for API compatibility
- test, clippy and check_links output (last lines)
- anything you could not do, with the exact error

Then run `touch docs/VOICE_ENGINE_PHASE_1_NOTES.sentinel`.
