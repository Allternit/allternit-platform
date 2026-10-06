# phone-remote

View **and control** this Mac's desktop from an iPhone — a PWA over Tailscale,
no iOS app, no public exposure. Inspired by an open-source multi-harness agent
client's phone→Mac remote desktop, deliberately smaller: no auto-unlock, no
lock-on-exit, no Face ID, no credential vault.

Spec: `Allternit Brain/Research/specs/phone-remote.md` (queue rq-20260910-006).

## Security model (mandatory, not optional)

This surface performs **input injection on the host** (synthetic mouse/keyboard
at the macOS HID event tap). That is only acceptable behind both of these:

1. **Tailnet-only binding.** The server binds the machine's Tailscale IPv4
   (default: `tailscale ip -4`, currently `100.88.98.69`) plus `127.0.0.1` for
   local testing. It never binds `0.0.0.0` or a public interface. No
   cloudflared, no port forwarding, no off-tailnet access.
2. **Token auth on everything.** A random per-boot token gates every HTTP
   request and the WebSocket upgrade (`?t=…` → `prt` SameSite=Strict cookie).
   No token / wrong token → 403. The full URL is printed at startup; treat it
   like a password (it grants control of your Mac).

Single viewer at a time. A second WS connection is refused (1013).

## Lifecycle ownership

- **Normal operation: the Allternit Desktop app owns the server.** On launch
  it spawns the bundled `server/index.mjs` (Electron's bundled Node,
  `ELECTRON_RUN_AS_NODE=1`, `PORT=8477`) if `/healthz` is dark, supervises
  the child with backoff, and SIGTERMs it (SIGKILL after 3s) on quit. Server
  stdio lands in `phone-remote.log` next to `main.log`. Set
  `ALLTERNIT_DISABLE_PHONE_REMOTE=1` to opt out of the spawn.
- **Adopt, don't kill.** If something already answers `/healthz` on 8477
  (e.g. a manual `node server/index.mjs` dev start), the desktop logs the
  foreign owner's PID, uses it, and does NOT supervise or stop it.
- **Standalone manual start still works** for development:
  `node server/index.mjs` from this directory. `PORT` env pins the port
  without argv.
- **Capture runs on demand.** The server does not capture at boot. A WS
  viewer, a `/hello` or a `/frame` request (the Fabric desktop viewer proxies
  both) starts capture; it stops after 30 s with no viewer and no poll
  (`PHONE_REMOTE_CAPTURE_IDLE_MS` overrides), and the cached frame is dropped
  so a stale screen is never served. The first `/frame` after an idle stop
  waits up to 2.5 s for a fresh frame. Before this, capture ran for the whole
  app lifetime: on the `screencapture` fallback that is a process per frame
  and a busy `replayd` all day with nobody watching.
- **Watchdog.** In sckit mode, once capture has started, a frame must arrive
  at least every 5s (it runs at 10fps). If the helper freezes, the server
  kills it, tries ONE in-process sckit restart, and — if frames still don't
  flow — exits non-zero so the desktop supervisor restarts the whole server.
  An unexpected `sc_capture` exit (other than TCC code 3, which still falls
  back to the `screencapture` loop) gets the same one-restart treatment.
- **Honest `/hello`.** The response gains `stale: true` plus an `error`
  string when capture is dead/frozen, alongside the existing fields
  (`hasFrame` stays a literal "a frame was ever cached" — check `stale`
  before trusting it).

## Architecture

```
 iPhone (Safari PWA, Add to Home Screen)                this Mac
 ┌──────────────────────────────┐        ┌──────────────────────────────────────────┐
 │ client/ (static, no build)   │        │ server/index.mjs (node:http, zero deps)  │
 │  canvas viewer               │  JPEG  │  ├─ lib/ws.mjs  hand-rolled RFC 6455      │
 │  tap/long-press/drag/scroll  │───────▶│  │               (text+binary, no ext)    │
 │  pinch zoom/pan (client-side)│binary  │  ├─ lib/capture.mjs ──▶ sc_capture (Swift│
 │  OSK via hidden textarea     │  WS    │  │                    ScreenCaptureKit →  │
 └──────────────────────────────┘frames  │  │                    length-prefixed     │
        tap → {type:"input",ev}─────────▶│  │                    JPEG stdout)        │
        (stream-image pixels)            │  │                    fallback:           │
                                         │  │                    screencapture+sips  │
                                         │  ├─ lib/input.mjs ───▶ input_helper.py    │
                                         │  │   (maps image px →  (line-JSON stdio → │
                                         │  │    screen points,    Quartz CGEvent at  │
                                         │  │    SERVER-side)      kCGHIDEventTap)    │
                                         │  └─ token gate (HTTP + WS upgrade)        │
                                         └──────────────────────────────────────────┘
              Transport: Tailscale (100.88.98.69:8477), HTTP+WS, token URL
```

## Quickstart

```bash
cd surfaces/phone-remote

# 1. Build the capture helper (first run only; the server also auto-builds it)
npm run build-capture            # = swiftc -O -o server/capture/sc_capture server/capture/sc_capture.swift

# 2. TCC grants — see the runbook below. Verify first:
./server/capture/sc_capture --check-permission        # prints granted|denied
python3 input/input_helper.py --dry-run <<<'{"type":"ping"}'   # ready line shows accessibilityTrusted

# 3. Run (Node >= 22, zero npm deps)
node server/index.mjs

# 4. The server prints a token URL, e.g.
#    http://100.88.98.69:8477/?t=<random>
#    On the iPhone: Tailscale ON → Safari → open that URL → Share → Add to Home Screen.
```

Tests:

```bash
npm test          # 38 checks: WS framing (masking, fragmentation, ping/pong,
                  # close), HTTP/WS token gate, single-viewer refusal,
                  # watchdog freshness + /hello stale reporting
npm run smoke     # live loopback: real capture → frames over WS, measured fps
```

## TCC permissions runbook (macOS 14.6.1)

Both capture and input inherit their TCC grants from the **terminal app that
launches the server** (Terminal.app, iTerm2, VS Code, …). Grants attach to the
process tree. **After changing a grant you must fully restart the terminal
app** — macOS caches TCC decisions per process tree.

### Screen Recording (required for capture)

1. System Settings → Privacy & Security → **Screen Recording** → enable your
   terminal app. Restart the terminal completely (⌘Q, not just the window).
2. Verify: `./server/capture/sc_capture --check-permission` → `granted`
   (exit 0). `denied` (exit 3) means the grant is missing or the terminal was
   not restarted.
3. The server runs this preflight at startup and prints the result. If frames
   would be black/empty or ScreenCaptureKit errors, the server says so loudly —
   it never fakes success. Without the grant it falls back to the
   `screencapture -x` loop (which captures only the wallpaper without the
   grant — the runbook fix is the grant, not the fallback).

**Known gotcha — do not launch the server under Python.** If any ancestor of
`sc_capture` is `/usr/bin/python3` (Apple-signed), TCC re-attributes Screen
Recording to python3 (the "responsible process"), which has no grant — and
ScreenCaptureKit *hangs silently* instead of erroring. Node-spawned (the normal
`node server/index.mjs` path) attributes correctly to the terminal. Measured
2026-09-10 on this machine: hangs under a python3 parent, works under node.

### Accessibility (required for input injection)

1. System Settings → Privacy & Security → **Accessibility** → enable your
   terminal app. Restart the terminal completely.
2. Verify: `python3 input/input_helper.py --dry-run <<<'{"type":"ping"}'` —
   the `ready` line shows `"accessibilityTrusted": true`.
3. Read-only probe (no events posted):
   `printf '{"type":"mousepos"}\n' | python3 input/input_helper.py`
4. Without the grant, CGEventPost silently drops events: clicks/keys will not
   land even though the helper answers `ok`. Trust the `accessibilityTrusted`
   flag (also surfaced in the WS `hello` message), and re-check it after
   terminal updates (macOS sometimes revokes grants on app signature changes).

## Configuration

```
node server/index.mjs [options]
  --port N             listen port (default 8477; avoids 8013 = allternit-api)
  --bind IP            tailnet bind address (default: `tailscale ip -4`)
  --token T            auth token (default: random per boot — printed at startup)
  --capture MODE       sckit | screencapture | none (default sckit)
  --fps N              capture fps target (default 10)
  --scale F            sckit capture scale 0–1 (default 0.5 → 960x540 stream)
  --quality F          JPEG quality 0–1 (default 0.6)
  --no-input           view-only mode (input helper not started)
  --input-dry-run      input helper echoes commands, posts no events

env PHONE_REMOTE_CAPTURE_IDLE_MS  stop capture after this long with no viewer/poll (default 30000)
```

Optional HTTPS: `tailscale serve` can front the port with a valid cert
(e.g. `tailscale serve --bg 8477`), which also unlocks `wss:` automatically in
the client. Not required on a tailnet (traffic is already WireGuard-encrypted)
and intentionally **not** set up here — run it yourself if you want it.

## Measured numbers (this machine, 2026-09-10, M-series, macOS 14.6.1)

Capture sources, standalone:

| source | fps | first frame | frame size | notes |
|---|---|---|---|---|
| ScreenCaptureKit (`sc_capture`, 960x540 q0.6) | **9.8** | ~140–156 ms | ~124 KB avg JPEG | default; includes cursor |
| `screencapture -x` loop, full-res 1920x1080 | ~12.5 warm | ~680 ms cold | ~1.29 MB | too much bandwidth for tailnet |
| `screencapture -x` + `sips` 960w q60 | ~8.7 | ~680 ms cold | ~148 KB | the implemented fallback (uses 1280w) |

End-to-end through the server (loopback WS, viewer attached):

| path | fps | notes |
|---|---|---|
| sckit → WS binary frames | **9.3** | 170 ms to first frame incl. stream startup; ~92 KB avg (static-ish desktop) |
| screencapture+sips → WS | **6.9** | ~181 KB avg @1280w |

Default = ScreenCaptureKit: better first-frame latency, in-process streaming
(no filesystem churn), cursor included, and explicit TCC error detection
(exit 3 = denied). ffmpeg avfoundation is a documented second fallback only;
it was never needed and is not implemented.

Latency budget over the real tailnet to the iPhone is **not** measured yet —
that is part of the human acceptance pass (below).

## Validation boundary — what is proven vs not

Proven on 2026-09-10 (all in this session, on this machine):

- WS framing: masking, 7/16-bit lengths, fragmentation, ping/pong, close
  handshake — 20 automated checks pass (`npm test`).
- HTTP + WS token gate: 403 without/with wrong token, 200 with token, cookie
  session, path traversal blocked, single-viewer refusal (`npm test`).
- Tailnet bind: reachable on `100.88.98.69`, refused on the public LAN
  interface; loopback bound in parallel.
- Capture: real JPEG frames (FFD8-validated) at ~9.3 fps end-to-end over
  loopback WS; Screen Recording TCC = granted in the testing terminal tree.
- Input: helper starts, `accessibilityTrusted: true`, and ONE minimal
  real-input probe (a single mouse-move CGEvent, verified landed and restored).
  No clicks/keys were injected into any app during development.
- Fallback path (`--capture screencapture`) delivers frames end-to-end.

**Not proven** (explicitly out of the v0 claim, per spec):

- End-to-end from the physical iPhone: opening the token URL, seeing the
  stream, tapping and typing into a real app. **This is the human acceptance
  step** — commands below.
- Tailnet latency / smoothness budget, locked-screen operation, multi-display,
  audio, multi-viewer, off-tailnet access, and any credential/auto-unlock
  features. None are claimed.

### Human acceptance pass (from the physical iPhone)

```bash
# on the Mac, in a terminal that has the TCC grants:
cd ~/Desktop/allternit-workspace/allternit/surfaces/phone-remote   # (or the worktree path)
node server/index.mjs
# copy the printed http://100.88.98.69:8477/?t=... URL
```

On the iPhone: Tailscale ON → Safari → open the URL → you should see the live
desktop. Tap something harmless (e.g. click the desktop background, open
Spotlight with a tap), toggle ⌨ and type into a text field. Then Share → Add
to Home Screen. Watch the server log for `[input]`/`[capture]` errors.

## Long-term home

v0 is deliberately a small, dependency-free Node surface. It is designed to be
reimplemented as an **axum module in `cmd/allternit-api/`** (the long-term
home), following the token-as-credential patterns in
`cmd/allternit-api/src/computer_ws.rs` (purpose-bound tokens verified before
upgrade; pre-upgrade validation shared by all WS handlers).

The module boundaries exist to make that port mechanical:

| now (`server/`) | later (axum) |
|---|---|
| `lib/ws.mjs` upgrade + framing | `tungstenite`/`axum::extract::ws` (drop the hand-rolled codec) |
| `lib/capture.mjs` frames + info, `start()/stop()` | a `capture` module spawning the same `sc_capture` binary |
| `lib/input.mjs` coordinate mapping + line-JSON IPC | an `input` module spawning the same `input_helper.py` (or direct `core-graphics` crate calls) |
| `index.mjs` token gate + routing + single-viewer | router fns + `AppState`-held token, per `computer_ws.rs` |

Keep the wire protocol stable (JSON text control + binary JPEG frames,
`pr1` subprotocol) and the PWA needs no changes when the backend is swapped.

## Out of scope (by design, per spec)

Auto-unlock, lock-on-exit, Face ID, credential vault, native iOS/Android apps,
off-tailnet access, multi-viewer, audio, multi-display.
