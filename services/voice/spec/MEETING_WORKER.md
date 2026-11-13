# Meeting worker: `allternit-voice-service meeting-worker`

The meeting worker transcribes Allternit Meetings calls, and it speaks Gizzi's replies when a meeting has spoken replies on. It registers with LiveKit as agent `allternit-meeting` and joins each `meeting-<session id>` room that cloud-api's meeting launcher creates for a meeting pod (`services/meeting-bot`).

In the room, it:

- subscribes to the pod's `call-audio` track;
- transcribes the audio continuously with Silero VAD and sherpa-onnx STT;
- sends every segment to the pod on data topic `allternit.meeting.stt`;
- sends a turn event on `allternit.meeting.turn` when someone says "Gizzi, …";
- speaks what the pod sends on `allternit.meeting.speak`, publishing the audio on the `tts` track.

The pod labels each segment's speaker from its active-speaker timeline, assigns `seq` and ingests the segment. Only the pod calls `/ingest`. The pod also labels the asker of each turn and runs the in-pod agent. The worker talks only to the pod, inside the room. It calls no cloud-api route and holds no credential other than its LiveKit job token.

Contract: `docs/meetings/CONTRACT.md` "Change 2026-11-13: pod ↔ worker names", plus the turn and speak topics from the coordinator binding of 2026-11-13, documented below. Plan: Architecture items 4 to 6. Source: `services/voice/src/meeting_worker/`.

The phone call worker (`call_worker/`, agent `allternit-voice`, `call-*` rooms) is a separate module, and neither worker takes the other's rooms. The meeting worker reuses the phone worker's LiveKit dispatch protocol and audio helpers without changing them.

## Build and run

The LiveKit binding sits behind the same `call-worker` cargo feature as the phone worker, because both pull in libwebrtc. Without the feature, `meeting-worker` exits with a rebuild hint.

```bash
cargo build --release -p voice-service --features call-worker
allternit-voice-service meeting-worker
```

| Variable | Required | Meaning |
|---|---|---|
| `LIVEKIT_URL` | yes | LiveKit server, `http(s)://` or `ws(s)://`. |
| `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` | yes | The server's key pair. Used only to mint the `video.agent` worker JWT. |
| `MEETING_WORKER_MAX_SESSIONS` | no | How many meetings run at once before the worker declines offers. Default 4. |
| `MEETING_WORKER_STT_MODEL` | no | `moonshine` (default, from the small pack) or `parakeet` (from the accurate pack, 25 languages). |
| `ALLTERNIT_VOICE_THREADS`, `ALLTERNIT_VOICE_MODEL_DIR` | no | Same as for the voice service (see the README). |

Spoken replies need `allternit-tts` next to the binary (or at `ALLTERNIT_TTS_BIN`) and the `tts` pack, the same as the voice service.

## Dispatch

The launcher creates the room `meeting-<sid>` with an agent dispatch: `create_room_with_agent(room, "allternit-meeting", metadata)` in `routes/livekit_admin.rs`. The session id comes from the room name. The metadata is optional; when present, it is this JSON object:

```json
{ "session_id": "<meeting_sessions.id>", "voice": "af_heart" }
```

No token goes in the metadata. The worker refuses a job in any of these cases:

- the room is not `meeting-<sid>`;
- the metadata names a different session;
- the metadata is not valid JSON;
- the worker is already running `MEETING_WORKER_MAX_SESSIONS` meetings.

## In the room

| Direction | What | Name |
|---|---|---|
| pod → worker | Mixed remote call audio | audio track `call-audio` |
| worker → pod | STT segments `{start_ms, end_ms, text, start_at}`, one per message, reliable | data topic `allternit.meeting.stt` |
| worker → pod | Wake-word turns `{turn_id, text, asker_label?, confidence, start_ms}` (see below), reliable | data topic `allternit.meeting.turn` |
| pod → worker | Text to speak `{turn_id, text}` | data topic `allternit.meeting.speak` |
| worker → pod | Spoken replies, 48 kHz mono | audio track `tts` |

`start_ms` and `end_ms` count from the first audio the worker received on `call-audio`. The pod publishes with DTX off, so this point is when the track went live. `start_at` is the speech start in epoch ms, and the pod prefers it. The worker sends no `confidence`, because sherpa's offline recognisers give no per-segment score. The pod then uses its speaker confidence.

The worker reads only tracks named `call-audio`. A newer subscription, for example after the pod reconnects, replaces the old reader. Agent participants are ignored.

## Transcription

The worker uses the voice service's `SttStream`:

- the VAD gets one window per feed;
- 0.3 s of silence ends a segment;
- the longest segment is 20 s;
- a Moonshine segment longer than 8 s is split at its quietest point.

Every segment is transcribed and sent. There is no end-of-turn model and no barge-in. The STT stream runs on its own thread because it is not `Send`. If that thread falls behind, the reader drops 10 ms frames and logs a warning, so the queue cannot grow without limit.

Times come from the sample count from the start of each stream. Suppose audio arrives more than 500 ms later than the sample count says it should, for example because the pod muted the track for "stop listening" or reconnected. Then the worker flushes the current stream and starts a new one at the real time, so segment times stay true. A pending wake word carries over the gap.

"Stop listening" is handled by the pod. It mutes `call-audio`, and it drops any segment that starts inside a paused window, so nothing said while paused is kept.

## Wake word

Source: `wake.rs`. STT doesn't know the name, so it writes what it hears. A token counts as "Gizzi" when, with hyphens and apostrophes removed, it is:

- `g` or `j`, then
- `i`, then
- one or two of `s`/`z`, then
- a vowel ending: `y`, `i`, `ie`, `ee` or `ey`.

The split form "Giz ee" also counts.

This covers Gizzi, Gizzy, Gizzie, Giz-ee, Jizzy (what the spike heard), Jizzie and Gissy. It excludes gizmo, gizzard, jazzy, dizzy, Jesse and "is he".

The name must stand in one of these places:

- at the start of a segment;
- right after a pause that STT marked with punctuation (`, . ? ! ; :` or a dash);
- after "hey", "hi", "okay", "ok", "yo" or "hello", anywhere in the segment.

A possessive ("Gizzi's notes") never triggers. The question is the text after the name. After a bare "Gizzi?", the worker waits up to 6 s and uses the next segment as the question.

Turn event (the `allternit.meeting.turn` payload):

```json
{"turn_id":"wake-<sid>-1794575078656","text":"please take note of that action item.","confidence":0.75,
 "start_ms":23302,"end_ms":30132,"start_at":1794575078656,"heard":"Jizzy"}
```

| Field | Meaning |
|---|---|
| `turn_id` | `wake-<session id>-<start_at>`. The same turn always gets the same id, so a resend can't start a second agent turn. |
| `text` | The question, with the wake word removed. |
| `asker_label` | Always left out. The worker has no speaker timeline. |
| `confidence` | The wake-word confidence: how close the spelling is, and where the name stood. |
| `end_ms`, `start_at`, `heard` | Extra fields. The pod may ignore them. |

The pod labels the asker from `start_ms`..`end_ms` with the same timeline it uses for segments. It then posts the turn to the in-pod agent: `POST /api/v1/meeting-turns/turn` on its allternit-api, as a `MeetingTurnRequest` with `trigger: {kind: "wake_word", text}`. Display names are not authenticated. An unclear speaker gets a clarifying question before any personal tool runs. The worker makes no cloud call for turns.

## Spoken replies

When the in-pod agent's reply should be spoken (the session has `speak_replies` on), the pod sends `{turn_id, text}` on `allternit.meeting.speak`. The worker handles it like this:

1. It synthesizes the text with Kokoro onto the `tts` track. Synthesis goes sentence by sentence, so the first sentence plays while the rest renders.
2. Replies play one at a time, in the order they arrive. The queue holds 16; past that, a reply is dropped with a warning.
3. Text longer than 2,000 characters is cut at the last sentence end before that limit.

Requests from agent participants are ignored, so only the pod can make Gizzi talk. Chat delivery of the same reply stays with the pod. The pod gates `speak_replies`.

## Offline check

`examples/meeting_fixture.rs` runs a recorded call through the same pipeline, without LiveKit or cloud-api. It prints the exact `STT` and `TURN` payloads. To get labelled segments, pipe them through the pod's own `SpeakerTimeline` (`services/meeting-bot/dist/ingest.js`), fed with the recorded speaker timeline, the same way `runner.ts` `ingestStt` does:

```bash
cargo run -p voice-service --example meeting_fixture -- \
  services/meeting-bot/spike/out/local-capture/call.webm \
  | node label-with-pod.mjs services/meeting-bot/dist/ingest.js \
      services/meeting-bot/spike/out/local-capture/speakers.jsonl \
      services/meeting-bot/spike/out/local-capture/audio/capture-start-ms
```

`label-with-pod.mjs` is a 40-line script. It loads `SpeakerTimeline`, folds the spike's `{t, name}` rows into polls, and labels each payload.

## Open contract items

1. **Turn and speak topics.** `allternit.meeting.turn` and `allternit.meeting.speak` come from the coordinator binding of 2026-11-13. They are not yet in CONTRACT.md's "pod ↔ worker names" section, which lists only `call-audio`, `tts` and `allternit.meeting.stt`. The pod (`services/meeting-bot/src/livekit.ts`) subscribes only to the STT topic so far.
2. **Dispatch.** The launcher (lane 2.1) has to create `meeting-<sid>` with agent `allternit-meeting`. The metadata is optional.
