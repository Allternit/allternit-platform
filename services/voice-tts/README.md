# allternit-tts

Kokoro text-to-speech as a small child process. This is the **GPL-3.0-or-later**
component of Allternit voice.

sherpa-onnx's Kokoro frontend turns text into phonemes with espeak-ng, which is
GPL-3.0-or-later. So TTS lives in this separate program, licensed GPL-3.0-or-later
(see `LICENSE`). `allternit-voice-service` (services/voice, speech-to-text and the
HTTP API, Apache/MIT dependencies only) starts it as a child process and talks to
it over a pipe. The voice service itself contains no espeak-ng code;
`scripts/check-voice-no-gpl.sh` checks that on every release build.

## What it does

- Loads Kokoro-82M v1.0 fp32 (`kokoro-multi-lang-v1_0`, from the voice service's
  `tts` model pack) with sherpa-onnx, on `--threads` CPU threads (default
  `ALLTERNIT_VOICE_THREADS` or 2).
- Synthesises one request at a time and streams the audio of each rendered piece
  as soon as sherpa-onnx hands it to its progress callback.
- No network access, no model downloads, no HTTP. The parent passes the model
  directory and does voice selection, text splitting and download management.

## Build and run

```bash
cargo build --release -p allternit-tts
allternit-tts --model-dir ~/.allternit/models/voice/tts/kokoro-multi-lang-v1_0 [--threads 2]
```

Desktop ships it next to `allternit-voice-service` in `resources/bin`; the voice
service looks for it in its own directory (override with `ALLTERNIT_TTS_BIN`).

## Protocol (stdin/stdout)

**Requests** (stdin): one JSON object per line.

```json
{"id": "7", "text": "Thank you for calling,", "sid": 3, "speed": 1.0}
```

`sid` is the Kokoro speaker id (0–53; the voice service sends 0–27, the English
voices), `speed` is 0.5–2.0 (default 1.0).

**Responses** (stdout): binary frames, each `[kind: u8][length: u32 little-endian][payload]`.

| kind | payload |
|---|---|
| `J` (0x4A) | UTF-8 JSON event |
| `P` (0x50) | s16le mono PCM, belongs to the `chunk` event right before it |

JSON events:

| event | meaning |
|---|---|
| `{"type":"ready","sample_rate":24000,"num_speakers":54}` | sent once, after the model loaded |
| `{"type":"chunk","id":"7","index":0,"samples":21600}` | followed by one `P` frame with `samples` × 2 bytes |
| `{"type":"done","id":"7","samples":21600}` | request finished |
| `{"type":"error","id":"7","error":"..."}` | request failed (`id` is null for a bad request line or a model that failed to load; a load failure is followed by exit code 1) |

**Lifetime:** the process exits when stdin reaches EOF, so it ends with its
parent even if the parent is killed. Logs go to stderr.

## Licence

GPL-3.0-or-later (`LICENSE`). Third-party components: `THIRD_PARTY_NOTICES.md`.
Its published source is https://github.com/Allternit/allternit-tts. This directory is the complete corresponding source of the `allternit-tts`
binary, together with sherpa-onnx 1.13.8, whose static libraries it links.
