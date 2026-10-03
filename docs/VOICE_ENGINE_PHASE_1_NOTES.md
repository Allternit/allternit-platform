# Voice engine Phase 1: notes

**status: partial.** The engine, API, packaging, whisper removal, tests and docs are done and verified. STT meets the standard. **TTS does not**: Kokoro-82M int8 runs slower than real time on 2 CPU threads (RTF 1.53). That needs a model decision from Eoj (see "Open decisions"). Everything else is in place, so swapping or adding a TTS model is a small change.

Finished by Claude (session joe-voice-p1), working from Kimi's WIP `adf5faa0a`. That WIP did not compile; most of it was rewritten (see "What changed from the WIP").

## Branch and commits

Branch `ao/voice-engine-p1`, pushed to origin. No PR opened.

```
8d2156f78 docs(voice): Phase 1 notes and M1 Pro CPU bench results
b5441bee3 fix(voice): pace streaming partials by their own decode cost
f09e4ec24 fix(voice): finish removing whisper and package the sherpa sidecar on every OS
5cbed3348 feat(voice): engine hooks for the voice session track
085853ddf fix(voice): make the sherpa-onnx engine compile and work end to end
33bcd6de1 docs: Phase 1 handover note (Kimi -> Claude)
adf5faa0a wip(voice): Phase 1 engine work in progress (Kimi)
```

## Bench (M1 Pro, CPU only, 2 threads, 40 LibriSpeech test-clean utterances)

`services/voice/bench/results-joes-MBP.jsonl`, produced by `cargo run --release -p voice-service --example voice_bench -- --data ~/.allternit/voice-bench`. The machine was shared with other agents' builds (load average 11–21 on 10 cores), so latency and RTF are pessimistic. WER is unaffected by load.

| Model | Condition | WER % | Finalise p50 / p95 ms | RTF | Peak RSS MB |
|---|---|---|---|---|---|
| Moonshine tiny (small) | wb, clean 16 kHz | **5.69** | **91 / 333** | 0.035 | 193 |
| Moonshine tiny | pstn, phone line @16 kHz | 7.24 | 81 / 280 | 0.024 | 193 |
| Moonshine tiny | pstn8k, raw 8 kHz μ-law | 10.61 | 83 / 214 | 0.026 | 193 |
| Parakeet 0.6B v3 int8 (accurate) | wb | **2.59** | 1044 / 3195 | 0.245 | 1005 |
| Parakeet | pstn | 2.98 | 856 / 2078 | 0.206 | 1005 |
| Parakeet | pstn8k | 5.17 | 561 / 1679 | 0.170 | 1005 |

| TTS | n | First audio p50 / p95 / max ms | RTF | Peak RSS MB |
|---|---|---|---|---|
| Kokoro-82M int8 en v0.19 | 10 sentences | 5229 / 6143 / 6143 | **1.533** | 348 |

The GPU reference (`reference_m1pro_gpu.jsonl`) has Parakeet at 2.46 / 2.72 % and whisper-tiny at 5.3 / 6.7 %. Parakeet on CPU int8 is 2.59 / 2.98 %, inside the expected 2.5–3 %.

### Against the standard

| Standard | Result |
|---|---|
| Small pack ≤ ~150 MB, downloaded on first use | **Meets.** 142.4 MB (VAD 0.6 + Moonshine 29.9 + Kokoro 103.2 + Smart Turn 8.7). A unit test enforces it. |
| ≤ ~1 GB RAM while running | **Meets** with the small pack (193 MB STT, 348 MB TTS, separate processes in the bench). Parakeet peaks at 1005 MB, right at the limit, and it is the opt-in "accurate" pack. |
| ≤ 2 inference threads | **Meets.** Every model uses `num_threads = ALLTERNIT_VOICE_THREADS` (default 2), CPU provider. One service-wide inference lock means only one model decodes at a time, so the total stays at 2. The VAD uses 1 thread per stream. |
| STT finalises ≤ ~300 ms after end of speech | **Meets with Moonshine**: p50 91 ms, p95 333 ms (borderline at p95, under load). Parakeet does not meet it on 2 CPU threads (p50 0.9–1.0 s). It is the accurate/cloud option. "Finalise" means: last audio sample in, to final transcript out (see the bench doc comment). It excludes the VAD's 0.3 s end-of-speech hangover; Phase 1b's Smart Turn decides end of turn. |
| TTS first chunk ≤ ~300 ms, faster than real time | **Fails.** Kokoro int8 RTF 1.53, first sentence ~5 s. See "Open decisions". |
| 8 kHz phone audio works | **Meets.** Raw 8 kHz μ-law: Moonshine 10.6 %, Parakeet 5.2 %. Our decode and resample matched the pre-resampled phone file at correlation 0.996 with zero lag, so the gap to `pstn` is the model's sensitivity, not a resampling bug. |

## Open decisions for Eoj

1. **TTS model (blocks the TTS half of the standard).** Measured on this Mac, 2 threads, same sentences:

   | Model | RTF | Download | Licence |
   |---|---|---|---|
   | Kokoro int8 en v0.19 (shipped) | 1.39–1.53 | 103 MB | Apache-2.0 |
   | Kokoro fp32 en v0.19 | 0.60 | 320 MB | Apache-2.0 |
   | Kitten TTS nano fp16 (sherpa-onnx) | 0.23 | 27 MB | Apache-2.0 |

   Kokoro's int8 quantised ops are slow in onnxruntime on ARM. On x86 with AVX2/VNNI, int8 is usually faster than fp32, but I could not measure x86 here (Rosetta has no AVX). Options:
   - (a) Kitten nano as the default (fast, small, lower quality) with Kokoro as a "better voice" choice for fast machines and Cloud Voice.
   - (b) Kokoro fp32: breaks the ~150 MB budget and is still not under ~300 ms to first audio.
   - (c) Keep Kokoro and rely on Cloud Voice for weak devices.

   The engine already streams per sentence, so any of these is a model-swap-sized change.
2. **espeak-ng is GPL-3.0-or-later and statically linked into `allternit-voice-service`.** sherpa-onnx's Kokoro and Kitten frontends phonemise with espeak-ng (`build.rs` links `espeak-ng` and `ucd`). Shipping the sidecar in Desktop carries GPL-3.0 obligations for that binary (offer its corresponding source). The Desktop app and other sidecars only talk to it over HTTP. Recorded in `services/voice/THIRD_PARTY_NOTICES.md`. Needs a licensing call: accept it (publish the voice-service source), or move to a TTS without espeak.

## Linking and packaging (onnxruntime and sherpa-onnx)

**Static on every OS.** The `sherpa-onnx` crate's default `static` feature makes `sherpa-onnx-sys` download upstream static archives at build time and link them into the binary: sherpa-onnx, onnxruntime, kaldi-native-fbank, kaldi-decoder, OpenFst, kissfft, sentencepiece, piper-phonemize, espeak-ng and ucd. I chose static because it leaves nothing to stage, no rpath, and no DLL search issues.

- **macOS:** `osx-arm64` / `osx-x64` static libs, lipo'd universal in `release-desktop.yml`. Verified with `otool -L`: only `/usr/lib` and system frameworks (libc++, Foundation, Security, SystemConfiguration, CoreFoundation, libiconv, libSystem). libbz2 was also system-linked at first; `bzip2` now uses the `static` feature.
- **Windows:** `win-x64-static-MT` libs, built with the static CRT. The Windows voice step now sets `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C target-feature=+crt-static` (scoped to that step) so the CRTs match. This also removes the need for a VC++ redistributable. **Not built here.** The first `release-desktop.yml` run verifies it. Never cross-compiled on the Mac.
- **Linux:** `linux-x64` static libs, plus system libstdc++/libm/libpthread/libdl. The Linux desktop job never built the voice sidecar; it now does (`Build voice service sidecar (Linux)`).
- `build-desktop.sh`, `prepare-platform-static.cjs` and `verify-packaged-resources.cjs` copy or require only `allternit-voice-service`. `release-preflight.mjs` checks the macOS job cargo-builds it. Release binary: 22 MB (macOS arm64).

## What changed from the WIP

- **Did not compile:** missing `Arc`, `.and_then(bool)`, `PackManager` not `Clone`, ambiguous `StreamExt`. Fixed by rewriting `stt.rs` and parts of `server.rs`, `tts.rs` and `models.rs`.
- **STT accuracy bug (biggest):** the VAD was fed 1 s chunks. sherpa-onnx dates a segment's start from the end of the chunk in which speech was confirmed, so onsets were up to 1 s late and most utterances lost their first word or two (Moonshine WER 54 %). Now the VAD gets one 512-sample window per call, with a 0.3 s pre-roll clamped to the previous segment. WER is 5.7 %.
- **Moonshine v2 tiny fails on segments over ~9.3 s:** onnxruntime throws a broadcast error in `encoder_attn` and sherpa returns empty text. Long segments are now split at the quietest point between 5 and 8 s.
- **Kokoro would not have loaded:** `find_file` could not find the `espeak-ng-data` *directory*. `tokens.txt` was also ambiguous between Moonshine and Kokoro in the same pack dir. Each component now resolves inside its own extracted dir.
- **Resampler:** the hand-written sinc used the wrong cutoff scaling and cut 8 kHz input at ~1.8 kHz. It is replaced by sherpa-onnx's windowed-sinc `LinearResampler`, plus a streaming variant. The μ-law decoder had a bias error (now matches G.711 / `audioop`).
- **Models:** extraction streamed (the WIP read the 487 MB archive into RAM); `error` state on any failure; progress across the whole pack; pack state detected from disk at startup; HTTP 416 resume path; `POST /v1/models/:pack`. Tested live: a real first-use download of the small pack from GitHub, and the accurate pack resumed from a pre-staged `.part` via 416.
- **Streaming STT:** the WIP allowed one stream at a time (409). Now each stream has its own VAD, an odd byte is carried across chunks, `?sample_rate=` is supported, and partials are paced by their decode cost.
- Deleted the dead Chatterbox-era `client.rs` / `types.rs`. They called `/v1/voice/*` routes that do not exist, including voice cloning.

## Track B integration (asked by the orchestrator)

Done in `5cbed3348`, without touching `src/session/` or `src/call_worker/`:

- `SttEngine::segment_stream(model)` returns a VAD-less `SegmentStream { feed, partial, finish, reset, duration_secs }`.
- `transcribe_segment(samples, model)` is kept (alias of `decode`).
- Public `VadConfig { threshold, min_silence, min_speech, max_speech }` and `SttEngine::new_vad_with(cfg)`, plus `VAD_WINDOW_SAMPLES`. Feed ≤ 512 samples per call; see the onset bug above.
- `TtsEngine::synthesize_stream(text, voice, speed, |i, sentence, samples, rate| -> bool)`. Kokoro renders a sentence in one pass, so its progress callback cannot stream sub-sentence audio.
- Smart Turn v3.2 is a `PackFile` in the small pack (`SMART_TURN_FILE`, pinned HF revision `f766f81…`, sha256 `2bb02631…`, 8,679,182 bytes, BSD-2-Clause; hash verified). `PackFile.upstream` carries the full HF URL; a custom `ALLTERNIT_VOICE_MODEL_BASE` mirror serves it at `<base>/smart-turn/smart-turn-v3.2-cpu.onnx`.
- `PackManager` is `Clone` (clones share state).
- Item (6), the `vec![]` lint, does not apply: this `server.rs` has no `Vec::new()` + push.

## Callers checked for API compatibility

This repo, plus `allternit-ai` (read only):

- **`POST /v1/stt`** (shape unchanged: `text`, `confidence`, `language`, `segments[]`): gizzi `localVoiceSTT.ts`, Desktop `voice-manager.ts` (`voice:transcribe` IPC), allternit-ai `useComposerVoice.ts` (WAV at AudioContext rate; resampled server side now).
- **`/v1/stt/transcribe`**: allternit-ai `SpeechToText.ts` reads `transcript` (kept). It uploads **WebM**, which gets a clear 415. It was already non-functional; the fix is client-side, in Phase 2 UI.
- **`/v1/voices`**: allternit-api proxy `v1_routes.rs:577` (bare array, passed through, unchanged). Added `label`, `engine`, `assetReady` because allternit-ai `RuntimeStep.tsx` calls `voice.engine.toUpperCase()` and the pickers read `label`.
- **`/v1/tts`, `/v1/tts/stream`, `/v1/stt/stream`**: only allternit-api's pass-through proxies (`v1_routes.rs:611-680`; the forced `Content-Type: application/json` is ignored by the stream endpoint). allternit-ai `VoiceService.ts` and `voice.service.ts` call non-existent `/v1/voice/tts` / `/api/v1/voice/tts` and parse JSON `audio_url`. They were broken before this change and need moving to `/v1/tts` bytes in Phase 2. Recorded in `docs/specs/tts-product.md`.
- **`/health`**: gizzi `doctorChecks.ts` (updated to read `engine` and pack states), `voice-manager.ts` and `localVoiceSTT.ts` (`ok` only).
- Dead gateway paths, not changed: `services/gateway/unified/main.py` (no prefix strip) and `services/gateway/http/runtime/index.ts` (`/v1/voice/*`) forward to routes the sidecar never had.

## Files changed (`646770988..HEAD`)

- **Engine:** `services/voice/src/{audio,models,stt,tts,server,lib,main}.rs`, deleted `{whisper,client,types}.rs`, `Cargo.toml`, `Cargo.lock`, `tests/integration.rs`, `examples/voice_bench.rs`, `bench/results-joes-MBP.jsonl`.
- **Whisper removal:** `services/voice/build-whisper.sh` (deleted), `start.sh`, `scripts/build-desktop.sh`, `scripts/release-preflight.mjs`, `.github/workflows/release-desktop.yml`, `surfaces/allternit-desktop/{src/main/voice-manager.ts, scripts/prepare-platform-static.cjs, scripts/verify-packaged-resources.cjs, docs/KNOWN-ISSUES.md}`, `cmd/gizzi-code/src/cli/{commands/doctorChecks.ts, ui/ink-app/services/localVoiceSTT.ts, ui/ink-app/commands/voice/voice.ts, ui/ink-app/hooks/useLocalVoice.ts, ui/ink-app/voice/voiceModeEnabled.ts}`, `cmd/gizzi-code/test/commands/doctor.test.ts`.
- **Docs:** `services/voice/{README.md, spec/API.md, THIRD_PARTY_NOTICES.md}`, `docs/specs/tts-product.md`, `docs/public/parity/chatgpt-voice.md`, `surfaces/docs/api/voice.mdx`, `surfaces/docs/tools/voice.mdx`, this file.

## Verification output (last lines)

```
$ cargo test -p voice-service
test result: ok. 30 passed; 0 failed   (unit)
test result: ok. 20 passed; 0 failed   (integration; model tests skip without the flag)

$ ALLTERNIT_VOICE_MODEL_TESTS=1 cargo test --release -p voice-service --test integration
test stt_transcribes_8k_phone_audio ... ok
test stt_stream_emits_final_and_done ... ok
test tts_stream_sends_one_audio_event_per_sentence ... ok
test result: ok. 20 passed; 0 failed; 0 ignored; finished in 21.15s

$ cargo clippy -p voice-service --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s)

$ python3 surfaces/docs/scripts/check_links.py
checked 413 nav entries, 411 pages: 0 problem(s)

$ node scripts/release-preflight.mjs
release-preflight: 50 passed, 0 failed

$ bun test test/commands/doctor.test.ts   (cmd/gizzi-code)
 18 pass  0 fail
```

## Not done / caveats

- TTS standard (above): a model decision.
- Windows and Linux builds not run locally (no cross-compile). The first `release-desktop.yml` run is their check, especially `+crt-static` on Windows.
- The latency numbers were taken under heavy machine load from other agents. Re-run the bench on an idle machine and a real 5-year-old x86 laptop before calling the standard met or missed for Parakeet.
- Merging ai main does not ship any of this. The sidecar ships in the next Desktop build. No UI changed in this phase, so the three-surface rule does not apply yet; Phase 2 must fix the allternit-ai TTS callers listed above.
- Model files on this Mac: `~/.allternit/models/voice/{small,accurate}` (874 MB unpacked). Kimi's `~/.allternit/voice-models-dl/` (~730 MB) is now redundant and can be deleted.
- Rust builds used the shared target (`.shared-target`). There is no `target/` or `node_modules/` in this worktree.

---

# Phase 1.1 results (Eoj's decisions, 2026-10-03)

**Status: done.** Both decisions are implemented, tested and pushed. TTS is much faster than in Phase 1, but it still misses the strict ~300 ms first-audio target on this machine (numbers below).

## Commits (on `ao/voice-engine-p1`, pushed)

```
<docs commit>  docs(voice): Phase 1.1 results
96e3c6616 docs(voice): Kokoro v1.0 fp32, the tts pack and the GPL allternit-tts split
fc17cf868 build(voice): ship allternit-tts next to the voice service on mac/win/linux
377ef7765 feat(voice): Kokoro v1.0 fp32 in a GPL-3.0 allternit-tts child; no espeak in the voice service
```

## 1. Kokoro full-size (fp32)

Bench of both fp32 candidates, 2 threads, same 5 sentences, standalone probe:

| Model | RTF | Download | Voices |
|---|---|---|---|
| `kokoro-en-v0_19` fp32 | 0.579 | 319.6 MB | 11 |
| **`kokoro-multi-lang-v1_0` fp32 (shipped)** | **0.538** | 349.9 MB | 54 (28 English exposed) |

v1.0 is faster and has more and better voices (RTF ≤ 0.7, so v1.0 as the spec prefers). Its speaker order was read from the model's own `speaker_names` metadata. The default voice is `af_heart`; the old ids `af`, `default` and `en-us-female` map to it, and `en-us-male` maps to `am_adam`. Pinned sha256 is `c5f7e2d2…3298`, size 349,906,910 bytes.

**Packs re-split** so dictation stays light:

| Pack | Contents | Download | Budget test |
|---|---|---|---|
| `small` | VAD + Moonshine + Smart Turn | 39.2 MB | ≤ 40 MB |
| `tts` | Kokoro v1.0 fp32 | 349.9 MB | ≤ 350 MB (actual size, per Eoj) |
| `accurate` | Parakeet | 487.2 MB | ≤ 490 MB |

**First-audio work:**
- The first sentence is cut at its first clause (`,` `;` `:` or a dash, 2–8 words in). Otherwise, if it is over 8 words, it is cut near the middle (3–8 words), before a phrase-starting word: "The quick brown fox jumps | over the lazy dog."
- Every piece sherpa-onnx hands to `generate_with_config`'s progress callback is streamed out at once. With Kokoro (non-autoregressive) that is one piece per sentence per call, so the short first chunk is what moves first audio.

**TTS bench** (`--only tts`, 10 fixed sentences, 2 threads, machine load average ~11):

| | First audio p50 / p95 / max | RTF | Peak RSS |
|---|---|---|---|
| Phase 1: Kokoro int8 v0.19, whole sentence | 5229 / 6143 / 6143 ms | 1.533 | 348 MB |
| **Phase 1.1: Kokoro v1.0 fp32 via allternit-tts, clause-split** | **1376 / 1974 / 1974 ms** | **0.821** | 605 MB (child) + 8 MB (parent) |

- **Faster than real time:** yes, overall RTF 0.82 under load. Splitting adds a per-call overhead, so the RTF is above the probe's 0.54.
- **First audio ≤ ~300 ms:** no. Kokoro has roughly 0.4 s of fixed cost per call on 2 threads here, plus about 0.4× the chunk's audio length. A 3–5 word chunk therefore takes ≥ 0.8 s even on an idle machine. Reaching ~300 ms on a 4-core laptop would take a smaller or streaming TTS model (or Cloud Voice), not more tuning of Kokoro.
- **RAM:** the voice service plus the TTS child stay under ~1 GB with the small and tts packs (193 + 605 MB). Model load takes ~4 s on the first TTS request.

## 2. GPL split: `allternit-tts`

- **New crate** `services/voice-tts` → binary `allternit-tts`, `license = "GPL-3.0-or-later"`, with `LICENSE` (the canonical gnu.org GPL-3.0 text), `README.md` (purpose, build, protocol) and `THIRD_PARTY_NOTICES.md`. It only loads Kokoro from the pack dir it is given and synthesises; it has no network access and does no downloads.
- **Protocol:** stdin takes JSON lines `{"id","text","sid","speed"}`. stdout sends frames of the form `[kind u8][len u32 LE][payload]`: `J` frames carry a JSON event (`ready`, `chunk`, `done`, `error`) and `P` frames carry s16le PCM for the preceding `chunk`. Documented in its README.
- **Parent** (`services/voice/src/tts.rs`, a client now):
  - It spawns the child from its own directory (override with `ALLTERNIT_TTS_BIN`), passing `--model-dir` and `--threads`.
  - One chunk at a time is sent under the service-wide inference lock, so TTS and STT still never run a model at the same moment.
  - `/v1/tts`, `/v1/tts/stream` and the `synthesize` / `synthesize_stream` signatures (Track B's `Tts` adapter) are unchanged.
- **Lifeline and restart, verified live:**
  - `kill -9` of the child: the next request logs `allternit-tts died: … Broken pipe; restarting`, restarts the child and returns 200 WAV.
  - `kill -9` of the parent: the child exits too (stdin EOF).
  - The child is also killed and reaped when the engine is dropped.
- **Keeping espeak out of the main binary.** Dropping the TTS code was enough on macOS (`-dead_strip`), but not on Linux. In the upstream `linux-x64` static libs, `SherpaOnnxCreateOfflineTts` and `SherpaOnnxCreateOfflineRecognizer` sit in the same `.text` section of `c-api.cc.o` (checked with `objdump -t`), so `--gc-sections` cannot drop the TTS path and `libespeak-ng.a` would be pulled in. The only espeak symbols any other sherpa/piper/onnxruntime library references are `espeak_Initialize`, `espeak_SetVoiceByName` and `espeak_TextToPhonemesWithTerminator`. The voice-service binary now defines those three (`no_espeak` in `src/main.rs`; inert, never called), so the linker never pulls an espeak-ng member. If sherpa-onnx ever needs another espeak symbol, the link fails on a duplicate definition instead of silently shipping GPL code. There is no cargo feature for an STT-only sherpa-onnx build.
- **`nm` result** (macOS arm64, unstripped release build, `CARGO_PROFILE_RELEASE_STRIP=false`):
  ```
  == voice-service:   95055 symbols; espeak: 0; piper: 2; OfflineTts: 0
  == allternit-tts:   63725 symbols; espeak: 29; piper: 33; OfflineTts: 658
  ```
  The 2 "piper" symbols in voice-service are `__GLOBAL__sub_I_piper_phonemize_lexicon.cc` and `piper::DEFAULT_PHONEME_MAP`: sherpa-onnx's lexicon code and piper-phonemize's phoneme table, Apache-2.0/MIT, not espeak. `grep -a -ci espeak` gives voice-service 0 and allternit-tts 10.
- **CI guard:** `scripts/check-voice-no-gpl.sh <binary>` fails if the voice-service binary contains any `espeak` bytes. Every release job (mac universal, Windows, Linux) and `build-desktop.sh` run it. Linux and Windows have not been built here (no Docker, no cross-compile), so the first `release-desktop.yml` run is the real check there.
- **Packaging:**
  - `build-desktop.sh` builds and stages `allternit-tts` and runs the guard.
  - `release-desktop.yml` builds both binaries in the mac (lipo'd universal), Windows (`+crt-static`) and Linux jobs.
  - `prepare-platform-static.cjs` and `verify-packaged-resources.cjs` require `allternit-tts`.
  - `release-preflight.mjs` checks every job that ships voice-service also builds and stages `allternit-tts` and runs the guard: 56 passed, 0 failed.
  - electron-builder already ships all of `resources/bin/`.
  - The app now bundles `licenses/allternit-tts-GPL-3.0.txt`, and `THIRD-PARTY-NOTICES.md` has a voice section naming the corresponding source.

## Files changed in 1.1

- **New:** `services/voice-tts/{Cargo.toml, LICENSE, README.md, THIRD_PARTY_NOTICES.md, src/main.rs}`, `scripts/check-voice-no-gpl.sh`.
- **Changed:**
  - Workspace: `Cargo.toml`, `Cargo.lock`.
  - Voice service: `services/voice/src/{tts.rs, models.rs, server.rs, main.rs}`, `services/voice/{tests/integration.rs, examples/voice_bench.rs, bench/results-joes-MBP.jsonl, README.md, spec/API.md, THIRD_PARTY_NOTICES.md}`.
  - Packaging: `scripts/build-desktop.sh`, `scripts/release-preflight.mjs`, `.github/workflows/release-desktop.yml`, `surfaces/allternit-desktop/{package.json, scripts/prepare-platform-static.cjs, scripts/verify-packaged-resources.cjs, docs/KNOWN-ISSUES.md, src/main/voice-manager.ts}`, `THIRD-PARTY-NOTICES.md`.
  - Other callers and docs: `cmd/gizzi-code/src/cli/ui/ink-app/services/localVoiceSTT.ts`, `surfaces/docs/api/voice.mdx`, `surfaces/docs/tools/voice.mdx`, `docs/specs/tts-product.md`, `docs/public/parity/chatgpt-voice.md`.

## Verification (last lines)

```
$ cargo clippy -p voice-service -p allternit-tts --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s)
$ cargo test -p voice-service -p allternit-tts
test result: ok. 33 passed; 0 failed   (unit)
test result: ok. 20 passed; 0 failed   (integration)
$ ALLTERNIT_VOICE_MODEL_TESTS=1 cargo test --release -p voice-service --test integration
test result: ok. 20 passed; 0 failed; 0 ignored; finished in 14.43s   (TTS through allternit-tts)
$ node scripts/release-preflight.mjs
release-preflight: 56 passed, 0 failed
$ python3 surfaces/docs/scripts/check_links.py
checked 413 nav entries, 411 pages: 0 problem(s)
$ scripts/check-voice-no-gpl.sh <release>/voice-service
check-voice-no-gpl: OK: no espeak-ng in .../voice-service
```

## Still open

- **First audio is ~1.4 s, not ~300 ms** (see above). That takes a model decision: a smaller or streaming TTS on weak devices, or Cloud Voice.
- **GPL source availability.** `allternit-tts` is GPL-3.0-or-later, so whoever distributes Desktop must make its corresponding source available: `services/voice-tts/` plus sherpa-onnx v1.13.8. If the platform repo is private, that means publishing `services/voice-tts` (and the build recipe) or shipping a written offer. Eoj's call.
- **Linux and Windows builds** are verified only by the first release workflow run, including `check-voice-no-gpl.sh` on Linux, where the `no_espeak` stubs do the real work.
- The upgrade from Phase 1 leaves an orphaned `small/kokoro-int8-en-v0_19` directory (~200 MB) on any machine that ran the Phase 1 build. Phase 1 never shipped to users, so nothing cleans it up automatically; I deleted it on this Mac.
