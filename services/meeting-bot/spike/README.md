# Meeting bot spike (plan step 0.3)

This spike tests whether the pod design works. One headed Chrome on Xvfb joins a Google Meet call as a guest. Its camera is our real bot (the allternit-ai `/meeting-tile` page, the Bot Mode stage, screencast from a second window) and its mic is a WebAudio track, both supplied by a `getUserMedia` override. The call audio comes out of a PulseAudio null-sink monitor and goes to the repo's sherpa-onnx STT. The result is a transcript with a speaker on each line, plus measured CPU and memory.

The spike is throwaway code. Step 2.1 builds the real runner, image and LiveKit path. Step 2.2 builds the Meet adapter.

## Files

| File | What it does |
|---|---|
| `gizzi-media.ts` | The init script. It replaces `getUserMedia` and `enumerateDevices`. The camera is a 1280x720 canvas streamed with `canvas.captureStream(15)`; it draws the newest tile frame handed to `window.__gizziTileFrame(jpegBase64)` (graphite until the first frame). The mic is a `MediaStreamDestination` track that stays silent until a file plays into it. It also adds `window.__gizziSpeak(url)`, which plays an audio file into the mic, and `window.__gizziOutLevel()`, that voice's level for the tile. |
| `peer-capture.ts` | The call recorder, a second init script. It wraps `window.RTCPeerConnection` and collects every remote audio track (the `track` event, receivers present after `setRemoteDescription`, and a 1 s rescan for tracks added later). Only receiving transceivers count, so the bot's own outbound mic and camera are never recorded. The tracks are mixed into one `MediaStreamAudioDestinationNode`, with each stream also on a muted `<audio>` element (Chrome feeds remote WebRTC audio to WebAudio only then), and recorded with MediaRecorder (`audio/webm;codecs=opus`, 1 s chunks). `drainPeerCapture()` pulls the chunks into `<out>/call.webm` and logs `peer_track_added` / `peer_track_ended` into `events.jsonl`. It replaced the old `CAPTURE_INPAGE` capture, which mixed `<audio>/<video>` element `srcObject`s and recorded nothing in a real Meet call. |
| `tile-feed.ts` | Opens `/meeting-tile` in a second window of the same Chrome (a background tab would be hidden and the stage pauses when hidden), sets a 1280x720 viewport, runs CDP `Page.startScreencast` (JPEG, acks spaced to 15 fps) and hands each frame to the meeting page. `setState()` calls the page's `__setTileState` (joining, listening, thinking, speaking, paused); `setLevel()` its `__setTileLevel`. |
| `meet-selectors.ts` | Every Meet selector, each with its source. ARIA roles and labels come first. Obfuscated class names are used only for the active speaker, because Meet gives no ARIA for it. |
| `join.ts` | Joins the call. Usage: `node dist/join.js --url <meet link> --name "Gizzi (Allternit)" --minutes N [--out dir] [--speak file.wav] [--page-url URL] [--tile-url URL] [--agent base64-json] [--no-tile]`. `--tile-url` defaults to the local allternit-ai dev server (`http://127.0.0.1:3013/meeting-tile`; production is `https://ai.allternit.com/meeting-tile`). Without a tile the camera stays off. Tile state follows the call (joining, then listening; speaking while `--speak` plays), and stdin lines `thinking` / `listening` / `paused` / `speak <file>` drive it. It opens Meet with a throwaway persistent profile, dismisses the sign-in prompt, fills the name, turns mic and camera on, clicks "Ask to join" and waits up to 10 min for admission. On live it posts the notice in chat. It then polls tiles every 250 ms and chat every 1 s. It leaves after N minutes, or after 5 minutes alone, or when it is removed or the call ends. Exit codes: 3 for denied, guest_blocked or timeout; 1 for an error. |
| `setup-linux.sh` | Idempotent pod setup. It installs PulseAudio, ffmpeg and Node 20, and runs `npm ci` and `tsc`. It starts PulseAudio with the null sink `meet_out` as the default sink and writes `spike.env`. No kernel modules. |
| `capture.sh` | Records `meet_out.monitor` as 16 kHz mono WAV segments (30 s by default) with ffmpeg. If ffmpeg has no pulse input, it falls back to `parec`. It also writes `segments.csv` and `capture-start-ms`. |
| `stats.sh` | Every 5 s, writes CPU% (from `/proc` tick deltas; 100 = one core), RSS and PSS for Chrome, node, ffmpeg and pulse into `stats.csv`. |
| `transcribe.sh` | Sends each segment to voice-service `POST /v1/stt` (Moonshine by default, or `parakeet`). It uses a running service if there is one. Otherwise it starts the `allternit-voice-service` bundled with the Allternit app in the image, or falls back to `cargo run`. Then it runs the merge. |
| `merge-transcript.ts` | Writes `transcript.jsonl` and `transcript.txt`. `speaker` is the active speaker nearest the segment midpoint (within 1 s). `speakers_seen` lists everyone marked as speaking during the segment. |
| `local-test/` | The Mac proof (below). |

## Local proof (Mac)

```bash
cd services/meeting-bot/spike
npm install          # local node_modules only; playwright-core uses /Applications/Google Chrome.app
npm run local-test   # add -- --headed to watch
```

The test loads `local-test/index.html` in headless Chrome with the same init script. It checks that both tracks are live, that the video is 1280x720 and the camera canvas keeps drawing, and that the mic is silent before `__gizziSpeak`, carries sound during it (AnalyserNode RMS) and is silent again after. The tile screenshot goes to `dist/local-test/tile.png`.

The same run then checks call capture over a real WebRTC connection. A tester page and a bot page (separate browser contexts, the bot with both init scripts) connect on 127.0.0.1. The tester plays `out/audio/tester1.wav`, adds a second audio track with a renegotiation, plays `out/audio/tester.wav` and stops that track. Meanwhile the bot sends a tone on its own mic. The check needs two tracks added, the second ended, the bot's own track not recorded, a recording as long as the call and a mean volume above -50 dB. Output goes to `out/local-capture/` (`call.webm`, `events.jsonl`, and `audio/seg-000.wav` + `segments.csv` + `capture-start-ms` + `speakers.jsonl`), so `bash transcribe.sh out/local-capture` transcribes it. `--capture-only` / `--media-only` run one half; `--wav1` / `--wav2` pick other files. MediaRecorder WebM has no duration header, so `ffprobe` prints `duration=N/A`; decode it (`ffmpeg -i call.webm -f null -`) or read the WAV for the length.

`node dist/local-test/tile.js --tile-url <url> [--headed] [--cpu 60]` proves the camera is the real tile: with the allternit-ai dev server up it starts the tile feed, reads the stand-in page's camera track in every state and checks each frame matches a screenshot of the tile window (plus a negative control). It writes `out/tile/camera-frames.png` and `out/tile/tile-test.json`; `--cpu N` samples node and Chrome CPU time for N seconds.

`transcribe.sh` and the merge were also run on the Mac. The input was a two-voice `say` fixture with a fake speaker timeline, sent to the Desktop's voice service. Both lines came back with the right speaker.

## Live run (Linux container)

You need these first: a container from the headless computer image (Xvfb :0, Google Chrome) with outbound internet, and a test Meet call whose host can admit a guest. Use a personal Google account or a Workspace that allows external guests.

```bash
# 1. Copy this folder into the container (no node_modules/dist), then:
cd /root/spike && bash setup-linux.sh && . ./spike.env

# 2. Three terminals (or tmux panes). Use the same OUT in each.
OUT=out/meet-$(date +%Y%m%d-%H%M)
bash stats.sh   $OUT 5          # pane A: start first, idle baseline
bash capture.sh $OUT 30         # pane B
node dist/join.js --url https://meet.google.com/xxx-xxxx-xxx \
  --name "Gizzi (Allternit)" --minutes 30 --out $OUT   # pane C
# 3. The host admits "Gizzi (Allternit)". Two or more people talk, taking turns, then overlapping.
#    Someone types a chat message. After it leaves: Ctrl-C capture and stats.

# 4. Transcribe (Moonshine; repeat with parakeet to compare)
bash transcribe.sh $OUT moonshine
```

Watch the live screen with `x11vnc -display :0` or with `DISPLAY=:0 scrot` screenshots. `join.ts` also saves `state-*.png` at every state change. It also records the call from the peer connections into `$OUT/call.webm` (no Pulse needed); `CAPTURE_PEER=0` turns that off.

## What to measure (goes into the spec)

- **Cost:** from `stats.csv`, the mean and p95 of `total_cpu_pct`, plus `chrome_pss_mb` and `total_pss_mb`, in three phases: idle before join, waiting, and live with incoming video. Also note the core count and the RAM limit of the container. These numbers set Eoj gate 5 (bot-minutes, concurrency).
- **STT cost:** how long `transcribe.sh` takes against audio length (RTF), for Moonshine and for Parakeet, on the same host.
- **Transcript quality:** compare `transcript.txt` with what was said. Count words wrong out of words said, and count lines with the wrong speaker. `speakers_seen` with more than one name shows overlap.
- **Selectors:** which steps ran unattended, according to `events.jsonl`. Check that `speakers.jsonl` is not empty, that `chat.jsonl` caught the test message, and that the notice showed up in the call. If speakers came out empty, read `tile-sample.html` for the current speaking-indicator classes and update `meet-selectors.ts`.
- **Tile and mic:** whether the other people see the tile and whether it updates. Optionally run `--speak hello.wav` and check that the others hear it.

## Risks seen while building

1. **The selectors are all unverified.** They were written from memory of the 2025 Meet DOM, and the source line on each says so. The speaker indicator is the weakest: it uses obfuscated class names with no ARIA fallback. If it fails, `speakers.jsonl` is empty and every transcript line gets `speaker: null`. Fallback ideas for 2.2: watch which tile's border or class changes per frame (that is what `tile-sample.html` is for), or turn on Meet captions, which name the speaker, as a second source.
2. **Participants come from the tiles, not the People panel.** Chat and People share one side panel, so the spike keeps chat open. Tiles can undercount in big calls or when someone presents. The 5-minute alone check depends on that count.
3. **Guest blocking.** A Workspace host who restricts the call to "people in the org" or "signed-in users" blocks every guest. The bot ends in `guest_blocked` (exit 3), which matches the plan's `guest_blocked` state. Meet may also refuse automated Chrome, because Playwright leaves `navigator.webdriver` on. The spike makes no attempt to hide that, per the plan's "no evasion" rule. If Meet refuses, record it as a finding.
4. **Autoplay.** The page `AudioContext` needs `--autoplay-policy=no-user-gesture-required`, which `join.ts` passes. If Chrome still suspends it, the mic stays silent and `__gizziSpeak` throws. The local test proves the path works headless on a Mac. It has not been proven on Linux and Xvfb.
5. **Audio routing.** Chrome must send call audio to Pulse (`PULSE_SERVER` and `PULSE_SINK` come from `spike.env`). If `audio/seg-*.wav` comes out silent, check `pactl list sink-inputs` while the call runs.
6. **Clock alignment.** Segment times are `capture-start-ms` plus the offsets in ffmpeg's `segments.csv`. Startup latency (~100 ms) and drift over 30 minutes shift where speakers land. Compare a known utterance against `speakers.jsonl` to measure the drift.
7. **The repo's pnpm workspace.** `pnpm-workspace.yaml` includes `services/*/*/*`, so this `package.json` becomes a workspace package. A root `pnpm install --frozen-lockfile` will then fail on the missing lockfile importer. Before this folder is committed, add `!services/meeting-bot/spike` and `!services/meeting-bot/spike/**` to the exclusions, the same way `services/mailflare` is excluded. The spike uses npm and its own `package-lock.json` so that the root lockfile stays untouched.
8. **Root in the pod.** Chrome runs with `--no-sandbox` when run as root, the same as `allternit-headless.service`. Run PulseAudio and Chrome as a non-root user if the pod design allows it.
