---
doc: spec
updated: 2026-10-02
status: in-progress
handoff: another-agent
implement_this_session: false
---

# Allternit Speech — TTS product spec

**Update 2026-10-02 (voice engine Phase 1, shipped):** the engine/API half of
this spec is done. `POST /v1/tts` returns real Kokoro audio bytes (WAV or
`format=pcm16`), `POST /v1/tts/stream` streams per-sentence pcm16 as
NDJSON, and `GET /v1/voices` lists the 11 installed Kokoro voices — engine
acceptance items **7, 8, 9** below are satisfied, and **15/16** hold
(sherpa-onnx in the Rust binary, static linking, notices in
`services/voice/THIRD_PARTY_NOTICES.md`; note espeak-ng, Kokoro's
phonemiser, is GPL-3.0-or-later and statically linked, an open licensing
decision). The **user-visible product** items
(1–6, 10–12: Desktop speak button, voice picker UI, Gizzi `/speak`,
auto-play, doctor, path aliasing, first-run UX) are **not** part of Phase 1
and remain open — Phase 2 per `HANDOFF-realtime-voice-2026-10-02.md`. STT is
no longer whisper.cpp: it is sherpa-onnx (Silero VAD + Moonshine/Parakeet),
same service, same `/v1/stt` shape.

**This session does not implement the remaining UI product.** The next agent reads this file, gets a human pick where a choice remains, then ships the **full product**. Do not land a stub, a sidecar-only curl demo, or a speak button that 404s.

## Goal

When this lands, Allternit **speaks**. Assistant replies can be read aloud on Desktop. Voice call mode is STT in and TTS out. Gizzi can read the last answer. The sidecar returns **real audio**, not a fake `audio_url`. Packaging ships a local engine. Users do not create a third-party account, pay a metered TTS API, or run Docker to hear a sentence.

STT (`/voice`, Ctrl+Space / F8; whisper.cpp until 2026-10-02, now sherpa-onnx Moonshine/Parakeet) already exists. TTS is the missing half of the same product.

## Why this is not “add Piper”

The last two PRs (#192, #194) made a hard cut: dictation first, Chatterbox Python TTS out. That was the right cut for STT. It is **not** a decision that Piper, Kokoro, or Chatterbox is the TTS product. The next agent must treat engine choice as a **bake-off**, not a default.

## Current reality (do not paper over)

| Surface | What it does today |
|---|---|
| `services/voice` `POST /v1/tts` | **Real audio** (Kokoro-82M int8 via sherpa-onnx), WAV bytes or `format=pcm16`. Shipped in Phase 1 (2026-10-02). |
| `GET /v1/voices` | **Real** list of the 11 installed Kokoro voices. Shipped in Phase 1. |
| `POST /v1/tts/stream` | **Real** NDJSON per-sentence pcm16 stream. Shipped in Phase 1. |
| `POST /v1/stt` | Real STT via sherpa-onnx (Silero VAD + Moonshine default / Parakeet accurate). Whisper.cpp removed. |
| Desktop `VoiceService.speak()` | `POST ${base}/v1/voice/tts` — **different path** than the sidecar’s `/v1/tts`. Still open (item 10). |
| `allternit-api` `/api/v1/voice/tts/stream` | Proxies sidecar `/v1/tts/stream`. Upstream is now real. |
| Chat Speak / voice-call / auto-play | UI exists but is **not** wired to real audio yet: allternit-ai `VoiceService.speak()` and `voice.service.ts previewVoice()` call `/v1/voice/tts` / `/api/v1/voice/tts` (paths that do not exist) and parse a JSON `audio_url`; they must switch to `/v1/tts` (or `/v1/tts/stream`) and read audio bytes. Phase 2. |
| Gizzi `/voice` | STT only. No read-aloud yet (item 4). |
| Chatterbox / FastAPI / pyinstaller | **Removed** (#194). Do not resurrect the Python tree. |

`docs/public/parity/chatgpt-voice.md` still describes the Python wrapper. Update it when the product ships.

## Hard constraints (veto)

Copied from the research pipeline and company rules. A candidate that fails any of these cannot be the **core** path.

| Veto | Meaning |
|---|---|
| Paid API / SaaS required | Core speak path needs ElevenLabs, OpenAI TTS, Cartesia, Picovoice Orca, etc. |
| Account signup required | Core path needs a vendor key or cloud account. |
| Docker required | Build or run requires containers. |
| Closed / NC / CPML / OpenRAIL that blocks Allternit use | Cannot ship in a commercial desktop product. |
| Clone a real person’s voice | Company rule: no likeness without documented on-file permission. Cloning as a **gated, opt-in** feature is allowed only with a real consent file. Default voice must be a **preset**, not a clone. |
| Python + pyinstaller as the shipped engine | Already failed once (macOS 26 vs 23.6 pyexpat). |
| “Simulated” / fake `audio_url` | Not a product. |

Prefer: MIT / Apache-2.0 / BSD, local binary, first-run model download (same pattern as `ggml-tiny.en.bin`), `MACOSX_DEPLOYMENT_TARGET=13.0`.

## What “full product” means

If it does not do all of this, it is not done. Do not merge a half.

### User-visible

1. **Speak last assistant message** on Desktop (the existing speaker control). Real audio plays. Stop works.
2. **Voice picker** with named presets (at least 2 English voices). Default is an Allternit/Gizzi preset, not a celebrity or employee clone.
3. **Voice call / duplex-lite:** user talks (existing STT) → model answers → TTS plays. User can interrupt (stop TTS, start STT). Not a research duplex stack; turn-based with barge-in is enough for v1.
4. **Gizzi read-aloud:** a `/speak` (or `/tts`) command and/or a key that speaks the last assistant reply through the same sidecar. If the sidecar is down, say so — no silent no-op.
5. **Auto-play** setting (already in voice-provider) actually works when on, stays off when off.
6. **Doctor / health:** sidecar reports `tts_ok`, voice id, model path, RTF or latency of last synth. Gizzi `/doctor` Voice section includes TTS.

### Engine / API

7. `POST /v1/tts` returns **audio bytes** (wav) or a JSON body plus a **fetchable** `audio_url` that `GET`s wav. No dangling UUIDs.
8. `POST /v1/tts/stream` streams pcm/wav chunks (or documented SSE of audio frames). Desktop can start playback before the sentence ends.
9. `GET /v1/voices` lists **installed** voices only.
10. One public HTTP shape. Collapse `/v1/tts` and `/v1/voice/tts` (alias both). The UI and the API gateway must hit a path that exists.
11. First-run download of the default voice model into `~/.allternit/models/tts/` (or next to the binary). Not git-vendored.
12. Packaged desktop includes the TTS binary + default voice, or a first-run download that `/doctor` can explain.

### Quality / ops

13. Fixture test: known sentence → wav with energy above silence, duration in a sane band. CI does not need a GPU.
14. macOS 23.x (Ventura/13+) is a supported runtime. Do not build native libs for macOS 26-only.
15. No new Python sidecar. A C/C++/Rust binary or ONNX runtime is the ship form.
16. License file / THIRD_PARTY notice for the chosen engine **and** G2P (espeak-ng is GPLv3 — see Kokoro note below).

### Explicitly not enough

- Curl against a research Python venv on the developer’s laptop.
- Wrapping `say` on macOS only (fine as a **dev fallback**, not the product).
- Re-adding Chatterbox as “the” product without a bake-off.
- Speak button that plays silence or 404.

## Landscape — ten systems to look at

Numbers below are **published / community**, often vendor-run. MOS/Elo disagree across posts. Treat them as **orientation**, not a ranking Allternit has measured. The bake-off is the ranking.

Sources sampled 2026-09-09: CodeSOTA TTS catalog, OpenSpeech community picks, tts-bench CPU/Mac tables, Picovoice on-device bench, Hugging Face model cards / GitHub licenses as cited in those roundups. Re-verify license and repo **on the day of the bake-off**.

### The ten

| # | System | License (verify) | Runtime shape | Hardware | Voices | Cloning | Why it is on the list |
|---|---|---|---|---|---|---|---|
| 1 | **Kokoro-82M** (hexgrad) | Apache-2.0 weights; ONNX runtimes exist (MIT) | ONNX (`kokoro-onnx`) or Python | CPU, ~80–340 MB model | ~54 presets, ~8 languages | No | Small, commercial-ok, often #1 in “runs anywhere” roundups. G2P: espeak-ng is **GPLv3** if used — ship path must not quietly copyleft the app. |
| 2 | **Piper** (rhasspy / OHF-voice) | **MIT on original rhasspy**; some forks **GPLv3** | Single ONNX CLI / C API | CPU, ~20–60 MB per voice | 30–40+ langs, many named voices | No | Fastest CPU TTFA in tts-bench (tens of ms). Pin the **MIT** tree. |
| 3 | **Kitten TTS** (KittenML) | Apache-2.0 | Tiny ONNX (~15–80M, ~24–42 MB quant) | CPU | 8 English | No | Smallest “neural and shippable” English. Quality below Kokoro in published benches. |
| 4 | **sherpa-onnx TTS** (k2-fsa) | Apache-2.0 runtime | One C++ binary, many model families | CPU | Catalog: Piper, Kokoro, Kitten, Matcha, Supertonic, ZipVoice, Pocket | ZipVoice/Pocket yes | Not a model — a **runtime**. Matches “one sidecar, many voices.” Already in-tree as an OpenClaw TTS skill (separate copy). |
| 5 | **Chatterbox / Turbo** (Resemble AI) | MIT | Python today; GPU-class | GPU ~4 GB+; CPU is slow (tts-bench ~0.3×) | Clone or default | Yes (~5 s) | Quality/emotion. We **just removed** the Python tree because it does not package. Only return if there is a **non-Python** ship form that survives macOS 23. |
| 6 | **Qwen3-TTS** (Alibaba) | Apache-2.0 | GPU serving (vLLM-Omni etc.) | GPU 6 GB+ class | Built-in + instruct style; 10 langs | Yes (~3 s) | Strong 2026 all-rounder. Heavy for a desktop sidecar. Fine as an **optional local-engine / Fabric** voice, not the default BYOC binary unless bake-off proves CPU. |
| 7 | **OpenVoice v2** (MyShell) | MIT | Python-centric | GPU typical | Multilingual | Yes | Cloning + permissive license. Same packaging risk as Chatterbox. |
| 8 | **StyleTTS 2** | MIT (code) | Research Python | GPU/CPU | Training-dependent | Style control, not 5s clone | Kokoro’s lineage. Only interesting if we train/own a voice. Do not ship the research repo as the product. |
| 9 | **Dia / Dia2** (Nari Labs) | Apache-2.0 (verify tag) | GPU, dialogue / multi-speaker | GPU ~6–8 GB | English dialogue | Limited | Interesting for **agent conversation**, not for “read this paragraph.” |
| 10 | **Supertonic / Pocket / ZipVoice** (ONNX / k2-fsa orbit) | Mix: Supertonic often OpenRAIL-M (restrictive); Pocket/ZipVoice Apache-2.0 | ONNX / sherpa | CPU–GPU | Multi-lang bundles; ZipVoice/Pocket clone | Some yes | Treat as **catalog entries under sherpa**, not three separate products. **OpenRAIL-M is a veto** unless legal clears it. |

### Looked at, not in the ten (and why)

| System | Why not a core candidate |
|---|---|
| ElevenLabs / OpenAI TTS / Cartesia / Picovoice Orca | Paid API or commercial SDK. Hard veto. |
| Coqui XTTS v2 | CPML **non-commercial**. Coqui shut down. |
| Fish Speech / OpenAudio S2 Pro / Breeze TTS 2 / Higgs Audio | Often research / NC / commercial-extra. |
| Bark (Suno) | MIT, slow, non-deterministic, SFX-oriented. |
| Orpheus 3B | Quality/emotion, but Llama-3.2 **community license** on the backbone is not a clean Apache ship. |
| Sesame CSM | High MOS in catalogs; 1B+ dialogue model; confirm license + runtime before promoting. |
| macOS `say` / `AVSpeechSynthesizer` | Zero deps, Apple-only. Dev fallback, not the Allternit product. |
| Microsoft VibeVoice | Watermark / withdrawn variants. Skip. |

## Architecture the product must have (engine-agnostic)

```
Gizzi /speak ─┐
Desktop Speak ┼─► voice-service :8001 ─► TTS runtime binary ─► voice model file
Voice call   ─┘         POST /v1/tts
                        POST /v1/tts/stream
                        GET  /v1/voices
                        GET  /v1/audio/...   (if URL style)
```

- **Runtime** (one binary we own the spawn of): e.g. sherpa-onnx, piper-cli, kokoro-onnx C ABI, or a thin Rust crate that dlopens ONNX. Chosen by bake-off.
- **Default model** (one file we download): English, preset speaker, CPU-real-time on M-series and x86_64.
- **Catalog** (optional extra voices): same runtime, more files. Never require a second engine for v1 extras.
- ~~STT stays whisper.cpp.~~ Superseded 2026-10-02: STT and TTS both run on sherpa-onnx (separate models) in the one `voice-service` process on port 8001.

## Bake-off (mandatory before writing product code)

The next agent **does not pick from this table in a vacuum**. On Eoj’s machine (macOS 23.6 class):

1. Shortlist **four** that survive the veto table. Suggested starting four (not locked): Piper (MIT pin), Kokoro-ONNX, Kitten, sherpa-onnx hosting one of those. If GPU is present, add Qwen3-TTS or Chatterbox **only** as a quality reference, not as the default ship.
2. Same three scripts: (a) UI string “Allternit is ready.” (b) 2-sentence assistant reply. (c) numbers + code identifier (`PR-194`, `8013`).
3. Record: time-to-first-audio, real-time factor, peak RSS, model bytes, license of **binary + weights + G2P**, whether streaming works, whether it runs with **no Python**.
4. Eoj listens. Human pick of **default runtime + default voice**. Write the pick into this spec’s “Decision” section. Then implement.

Do not skip step 4. MOS from blogs is not a product decision.

## Surfaces and files the executor will touch

After the pick:

- `services/voice/src/server.rs` — real TTS, real audio GET, health `tts_ok`
- ~~`services/voice/src/whisper.rs` pattern~~ (removed): `src/tts.rs` runs Kokoro in-process via sherpa-onnx; models come from `src/models.rs` packs
- `services/voice/build-*.sh` — build the TTS binary with `MACOSX_DEPLOYMENT_TARGET=13.0`
- `scripts/build-desktop.sh` + `verify-packaged-resources.cjs` — stage TTS binary + default voice
- `surfaces/allternit-desktop/src/main/voice-manager.ts` — already spawns the sidecar; pass `TTS_BIN` / `TTS_MODEL` env
- `surfaces/ai.allternit.com/src/services/voice/VoiceService.ts` — hit `/v1/tts`, play returned wav; fix path mismatch
- `surfaces/ai.allternit.com/src/providers/voice-provider.tsx` — auto-play, stop, queue
- Chat speak button + voice-call mode barge-in
- `cmd/gizzi-code` — `/speak` builtin (same registration style as `/voice`)
- `cmd/allternit-api` voice proxy — keep `/api/v1/voice/*` working
- `docs/public/parity/chatgpt-voice.md` — rewrite to the real product
- `THIRD-PARTY-NOTICES.md`

## Phased scope

**Phase 1 (the handoff — this is the product):** bake-off → human pick → real `/v1/tts` + `/v1/voices` + Desktop speak + stop + voice picker + Gizzi `/speak` + package default voice + tests + doctor.

**Phase 2 (not required to call Phase 1 done):** streaming TTS, extra catalog voices, duplex interruption polish.

**Phase 3 (explicit human gate):** any cloning / custom Gizzi voice. Consent file required. Default remains a preset.

## Gate checklist

- [ ] Client-facing copy? Register 1. Do not claim “indistinguishable from human” or “beats ElevenLabs.”
- [ ] Money-adjacent? No Stripe. Model download is free/local.
- [ ] Deploy? Desktop packaging only; preview the build command.
- [ ] Voice clone of a real person? Forbidden without on-file permission.
- [ ] License of G2P / ONNX runtime reviewed and noticed.

## Acceptance criteria

- [ ] `POST /v1/tts` with `{"text":"Allternit is ready."}` yields playable wav (HTTP 200, non-silent PCM).
- [ ] Desktop speaker control plays that audio; Stop silences it within 300 ms.
- [ ] `/v1/voices` lists ≥2 installed English presets; picker changes the voice on the next speak.
- [ ] Gizzi `/speak` reads the last assistant message or says the sidecar is down.
- [ ] Packaged app: TTS works offline after first model fetch (or ships the default voice).
- [ ] `cargo test -p voice-service` includes a non-silent wav fixture test (skip if binary missing, fail in packaging CI if missing).
- [ ] No Python voice tree reintroduced.
- [ ] Path alias: UI `/v1/voice/tts` and sidecar `/v1/tts` both work.

## Executor model tier

- Task class: local ML sidecar + desktop/TUI product wiring
- Model tier: fill via `model_route` at execute time — never guessed here
- Concrete backend: TBD after bake-off

## Open questions (human)

1. Is v1 **English-only** (recommended) or must we ship a second language on day one?
2. Is cloning in v1 at all, even gated? Default recommendation: **no**.
3. Should Gizzi speak automatically, or only on `/speak`? Default: **command only**.
4. Default speaker: existing stub names vs a named Allternit/Gizzi preset (still a third-party model voice, not a clone).

## Decision

_Empty until bake-off + Eoj pick. Do not fill this with a blog ranking._

- Runtime:
- Default model / voice id:
- Ship form (binary name):
- G2P / license notes:
- Date / picker:

## Source of truth

- Workspace: `allternit-platform`
- Spec: `docs/specs/tts-product.md`
- Sidecar: `services/voice/`
- STT already landed: PR #192; Python TTS tree removed: PR #194
- Tracking PR: _this spec PR_

## Sources (landscape, not measurements)

- https://www.codesota.com/guides/tts-models
- https://www.codesota.com/speech/best-open-source
- https://www.openspeech.dev/
- https://github.com/5uck1ess/tts-bench/blob/master/docs/results.md
- https://picovoice.ai/blog/on-device-tts/
- https://huggingface.co/hexgrad/Kokoro-82M
- https://github.com/rhasspy/piper
- Resemble AI Chatterbox (MIT); Qwen3-TTS (Apache-2.0) as cited in 2026 self-host roundups

Re-open each license and README on bake-off day.
