# Voice Session protocol, v1 (frozen 2026-10-02 by the voice session, joe-9e)

One protocol on every surface (Desktop, ai.allternit.com, m.allternit.com, iOS later). The engine behind it runs either on the user's device (`allternit-voice-service` sidecar, or WASM later) or on Allternit Cloud. The app can't tell the difference. Changes need the voice session's sign-off.

## Principle: voice is input/output, the agent loop stays where it is

- **In-app:** the app owns the conversation. The voice engine turns speech into final transcripts and turns text into speech.
  - The app sends each final user transcript to gizzi-code as a normal chat message, the same path as typing, so tools, approvals and the thread work unchanged.
  - The app streams the assistant's reply text back to the engine (`speak.delta`) to be spoken while it streams.
- **Phone calls:** the call worker plays the app's role (it holds the LLM turn loop via cloud-api → relay → runtime). It uses the same engine core.

## Transport

- WebSocket at `GET /v1/voice/session` on the voice service.
  - Local sidecar: `ws://127.0.0.1:<port>/v1/voice/session?token=<sidecar token>`.
  - Cloud: `wss://…/v1/voice/session?ticket=<short-lived ticket from allternit-api>`.
- **Text frames** are JSON control/events, each with a `"type"`.
- **Binary frames** are audio.
  - Client → server: mic audio, PCM16 LE mono at the `inputSampleRate` declared in `session.start` (default 16000). Frames should be 20 ms; any size is accepted.
  - Server → client: speech audio, PCM16 LE mono at `outputSampleRate` from `session.ready` (Kokoro: 24000). It always belongs to the utterance named by the latest `speak.started`.

## Client → server (JSON)

| type | fields | meaning |
|---|---|---|
| `session.start` | `voice?`, `sttModel?` (`light`\|`accurate`), `language?` (default `en`), `inputSampleRate?`, `bargeIn?` (default true), `turn?` {`mode`: `smart`\|`vad`, `silenceMs?`}, `fillers?` (default false), `fillerMs?` (200–10000, default 1200) | First frame. The server answers `session.ready` or `error`. |
| `session.update` | any `session.start` field | Change settings mid-session. |
| `speak.delta` | `id`, `text` | Append reply text for utterance `id`. The server speaks it sentence by sentence as it arrives. |
| `speak.prepare` | `texts` [string] | Pre-render `texts` in the session's voice into the phrase cache, so a later `speak.delta` with the same text starts at once. No reply. The phone call worker sends its disclosure and greeting with it before speaking them. Engines without a cache ignore it. |
| `speak.done` | `id` | No more text for `id`. Flush the last sentence. |
| `speak.cancel` | `id?` | Stop speaking (`id` or the current one) and drop queued audio. |
| `mic.mute` / `mic.unmute` | — | While muted, the server ignores mic audio and doesn't detect turns. |
| `session.end` | — | Close cleanly. |

## Server → client (JSON)

| type | fields | meaning |
|---|---|---|
| `session.ready` | `sessionId`, `engine` (`device`\|`cloud`), `outputSampleRate`, `models` {stt, tts, vad, turn}, `voices` [ids] | Ready. |
| `speech.started` | `atMs` | The user started talking (VAD). |
| `speech.stopped` | `atMs` | VAD silence. This isn't end of turn yet. |
| `transcript.delta` | `segmentId`, `text`, `final: false` | Interim text while the user talks (for live captions). |
| `transcript.final` | `segmentId`, `text` | Final text for a segment. |
| `turn.ended` | `text` (all final segments of the turn), `confidence` | **The user's turn is over: the app sends `text` to the agent now.** |
| `speak.started` | `id` | The audio frames that follow belong to `id`. |
| `speak.ended` | `id` | All audio for `id` has been sent. |
| `speak.interrupted` | `id`, `sentMs` | Barge-in: the user spoke over the bot. The server stopped sending, and the client must flush its playback buffer at once. The app should also abort the agent's in-flight turn. |
| `error` | `code`, `message`, `fatal` | `fatal: true` means the server closes the socket. |

## Timing targets (target machine: 8 GB RAM, 4 cores, no GPU, 2 inference threads)

- `turn.ended` ≤ 300 ms after the user stops (`smart` mode), excluding the model's think time.
- First `speak` audio ≤ 300 ms after the first full sentence arrives in `speak.delta`.
- `speak.interrupted` ≤ 200 ms after the user starts talking over the bot.

## Turn detection

- `vad`: Silero VAD (sherpa-onnx). The turn ends after `silenceMs` of silence (default 700).
- `smart` (default): VAD plus Smart Turn v3.2 (BSD-2, 8 MB ONNX, audio-native).
  - At each `speech.stopped`, Smart Turn scores the last ≤ 8 s of audio. ≥ 0.5 ends the turn at once; below that, wait up to `silenceMs` (default 1500) for more speech.
  - Smart Turn ships in the model pack with a sha256 pin.

## Versioning

`session.ready` carries `"protocol": 1`. Clients must ignore unknown event types and fields.

## Implementation notes (`services/voice/src/session`)

These describe how this service implements v1. They clarify the protocol; they don't change it.

- **Code map.** `core.rs` is the transport-agnostic `VoiceSession` (channels in, channels out). `ws.rs` is the WebSocket route. `engine.rs` holds the engine traits (`StreamingStt`, `Tts`, `Vad`, `TurnDetector`, `EngineFactory`), `engine_sherpa.rs` implements them on the Phase 1 sherpa-onnx engine (always compiled, sharing the server's loaded engines), and `mock.rs` is the scripted test engine. `turn.rs` is Smart Turn v3.2. The phone call worker drives `core.rs` directly with 8 kHz or 48 kHz audio and can force the speech output rate (`CoreConfig.output_sample_rate`).
- **Auth.** A `ticket` is redeemed by the configured `TicketVerifier` before the upgrade. `CloudTicketVerifier` is configured when env `ALLTERNIT_CLOUD_API_URL` and `ALLTERNIT_VOICE_WORKER_TOKEN` are both set: it calls cloud-api `POST /api/v1/voice/tickets/redeem` (bearer worker token, 5 s timeout) and gets `{sub, plan, maxSeconds}`; tickets are single-use. With either variable unset, every ticket is rejected. A `token` must equal env `ALLTERNIT_VOICE_TOKEN`. With neither, and `ALLTERNIT_VOICE_TOKEN` unset, only loopback peers are accepted. A failure is HTTP 401 before the upgrade. Tickets and tokens are never logged.
- **Cloud sessions.** A ticket session reports `engine: "cloud"` in `session.ready`. At `maxSeconds` of wall-clock time the service sends `error {code: "session_limit", fatal: true}` and closes. On close, for any reason, a background task posts `{sub, sessionId, seconds, engine: "cloud"}` to cloud-api `POST /api/v1/voice/usage` (seconds = wall-clock duration rounded up, minimum 1; five attempts over about a minute; cloud-api is idempotent on `(engine, sessionId)`). A session that never reached `session.ready` reports nothing. Token and loopback sessions (Desktop sidecar, call worker) never report usage.
- **`atMs`** is mic-audio time: ms of mic audio received since `session.start`, including audio sent while muted.
- **`sentMs`** is ms of speech audio sent for that utterance before the interrupt.
- **Pacing.** Speech audio is sent in 20 ms frames, at most 250 ms ahead of real-time playback. On barge-in, the queued audio that hasn't been sent is dropped, so the client flushes at most about 250 ms.
- **Barge-in** interrupts every live utterance, including ones that are queued or still being synthesized. Each gets `speak.interrupted` (`sentMs` 0 if nothing was sent). Later `speak.delta`/`speak.done` for an interrupted, cancelled or finished `id` are ignored.
- **`speak.cancel`** answers with `speak.ended` for the cancelled `id`.
- **Utterances** play in the order their first `speak.delta` arrived. An utterance with no speakable text still gets `speak.started` + `speak.ended`.
- **Sentences** are split on `.`, `!`, `?`, `…` and newlines. They aren't split on decimals, initialisms, single initials, common abbreviations, or a period or ellipsis followed by a lowercase word. Text with no boundary for 280 characters is cut at a clause break. Markdown emphasis, code ticks, headings and bullets are stripped before TTS.
- **Turn confidence.** In `smart` mode it's the Smart Turn probability at the last speech stop. In `vad` mode it's `1.0`. A turn whose final segments are all empty doesn't emit `turn.ended`.
- **Muting** mid-speech emits `speech.stopped` and discards the turn in progress.
- **`smart` without a turn model** falls back to `vad` and sends a non-fatal `error` with code `turn_unavailable`. `models.turn` is then `"vad"`.
- **Error codes:** `bad_message` (unparseable or unknown frame, non-fatal), `not_started`, `already_started`, `bad_option` (fatal on `session.start`, non-fatal on `session.update`), `engine_unavailable` (fatal at start), `engine_error`, `turn_unavailable`.
- **Smart Turn model:** `smart-turn-v3.2-cpu.onnx`, BSD-2-Clause, from `huggingface.co/pipecat-ai/smart-turn-v3` at revision `f766f81d3cfdf7737ac64aad813d91bbfd56bf93`, sha256 `2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f` (8,679,182 bytes). It runs on the onnxruntime that sherpa-onnx links, so the binary carries one runtime.
- **Test client:** `cargo run -p voice-service --example voice_session_client -- --wav in.wav --say "text" --out reply.wav`.

## Faster first speech (phrase cache and fillers)

- **Phrase cache.** Fixed phrases are rendered once and replayed from disk, so they start in about a millisecond instead of 0.3–2 s of Kokoro time. The cache lives in `<model dir>/phrase-cache/`, keyed by (voice id, speed, text, model version), with an LRU cap of `ALLTERNIT_VOICE_PHRASE_CACHE_MB` (default 64; `0` turns it off). The text is matched per streamed chunk. Stored: the built-in acknowledgements ("Sure.", "Okay.", "Got it.", …), the fillers, and any text sent with `speak.prepare`. Ordinary replies are never written to disk. Starting a session pre-renders the built-in phrases for the session voice in the background (a no-op once cached).
- **`fillers`.** With `fillers: true`, if no `speak.delta` has arrived `fillerMs` after `turn.ended`, the server speaks a short cached filler ("One moment.", "Let me check that.", …) as an utterance with id `filler-N` (`speak.started`/`speak.ended` as usual). If the reply still hasn't started, another filler follows every 6 s, at most 3 per turn; the first `speak.delta` or new caller speech stops them. A filler is interruptible like any speech: barge-in and `speak.cancel` stop it. A reply that arrives while it plays queues behind it. A turn that follows a turn answered only by fillers gets none: the next filler needs a real reply in between. Default off; the phone call worker turns it on.
- **First chunk.** The first sentence is cut at its first clause mark, or after 2–4 words (before a phrase-starting word where possible), so the first audio is a 2–4 word render.
- **Warm start.** `allternit-tts` renders a short sentence before it reports ready, so the first real request isn't a cold ONNX session.
