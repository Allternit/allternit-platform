# craft-host — host-page bridge protocol (craft:1)

One embedding protocol for all three craft editors (image / pdf / video). The WASM apps
run sandboxed in a cross-origin iframe served from `office.allternit.com/craft/<app>/`;
the Allternit workspace (or any host page) drives them over `postMessage`. FilmCraft's
existing `window.filmcraft` object is the upstream reference shape; this protocol
generalizes it and adds the security model embedding requires.

## Goals
- Open a document (bytes), let the user edit, stream saves back to the host.
- Let an agent (or host UI) drive the app's existing engine command registry live —
  the same visible editor, not a headless clone.
- Zero trust in the parent: the app validates origin + a per-session token before
  accepting any command.

## Roles
- **Host**: the Allternit workspace page holding the iframe (ArtifactWindow's editor
  pane). Generates the session token, owns persistence (`/api/v2/artifacts`).
- **App**: the craft WASM editor inside the iframe. Stateless about where bytes go;
  asks the host to save.
- **Agent**: optional; speaks to the app *through the host* (host forwards
  `command` messages; the app never has a second channel).

## Handshake
1. Host loads iframe: `https://office.allternit.com/craft/<app>/?embed=1&origin=<urlencoded parent origin>`.
2. App posts `craft:ready {app, version, protocol: "craft:1"}` to `event.source` (it does
   not know the parent origin yet; it replies to whoever loaded it — safe because it
   was loaded with `sandbox` and no credentials, and it will validate the token).
3. Host posts `craft:hello {protocol: "craft:1", token, theme, chrome, capabilities}`.
4. App validates token (random ≥128-bit, host-generated per editor session) and origin
   (the `origin` query param must equal `event.origin` of the `hello` message), then
   enters embedded mode: chrome hidden (no menu bar/window chrome per `chrome` value),
   theme applied, and acknowledges `craft:hello-ack {ok: true}`. Until a valid hello,
   commands are rejected and the app shows its normal standalone UI.

## Messages (host → app)
| type | payload | meaning |
|---|---|---|
| `craft:open` | `{name, bytes: ArrayBuffer (transferred), format?}` | replace current document |
| `craft:command` | `{id, cmd, params}` | run an engine command (the agent lane); see Command channel |
| `craft:theme` | `{dark, accent?, scale?}` | live theme update |
| `craft:ping` | `{} | keepalive / liveness |

## Messages (app → host)
| type | payload | meaning |
|---|---|---|
| `craft:ready` | `{app, version, protocol}` | step 2 of handshake |
| `craft:hello-ack` | `{ok, error?}` | step 4; `ok:false` = host must show an error surface |
| `craft:open-ack` | `{ok, error?, warnings?}` | document loaded (or parse errors, warnings) |
| `craft:document-changed` | `{dirty, autosaveable?}` | dirty flag for the host header |
| `craft:save-request` | `{name, format, bytes: ArrayBuffer (transferred), meta?}` | user (or command) initiated save; host persists, then MUST reply `craft:save-ack` |
| `craft:command-result` | `{id, ok, result?, error?}` | response to `craft:command` |
| `craft:command-event` | `{event, data}` | async engine events (progress, selection changed, etc.) |

## Command channel
Each app already has a JSON command registry (PhotoCraft 500+, PdfCraft, FilmCraft 650+)
used by its CLI/MCP/control channel. `craft:command` reuses the *same* registry — the
bridge maps `cmd`/`params` onto it. The host allow-lists which command ids each kind
may run (image editor ≠ pdf editor surface); unknown ids return
`{ok:false, error:"unknown/disallowed command"}`. The app MUST rate-limit command
execution to one in flight per session unless the registry documents re-entrancy.

## Security model
- App iframe: `sandbox="allow-scripts allow-downloads"` (no `allow-same-origin` in v1 —
  opaque origin, storage partitioned; OPFS use is verified in the build phase, and if a
  hard blocker appears we revisit with `allow-same-origin` + same-site serving).
- Token: host-generated per session, passed only via `craft:hello`, validated by the app
  before accepting `craft:open`/`craft:command`. Wrong/missing token → ignore + log.
- Origin: app was loaded with `?origin=`; a `craft:hello` whose `event.origin` differs
  is ignored. The served `index.html` carries no credentials, cookies, or tokens of its
  own (static hosting only).
- CSP on `/craft/*`: `default-src 'none'; script-src 'self' 'wasm-unsafe-eval';
  style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; media-src 'self'
  blob:; connect-src 'none'; worker-src 'self' blob:`. COOP/COEP headers for
  wasm threads (verify Pages support; else single-threaded builds — upstream documents
  this fallback).
- Bytes travel as `ArrayBuffer` via `postMessage` transferables (no base64 copies of
  multi-MB documents).

## Failure modes (host UI)
- iframe load error / no `craft:ready` in N seconds → error surface with retry.
- `craft:hello-ack {ok:false}` → "editor refused this embed" (version/protocol mismatch).
- No ack to `craft:save-request` in 30s → app keeps document dirty and shows its own
  "host unreachable — keep editing" state; nothing is lost.

## Per-app adapter mapping (from the 2026-10-09 audits)
`audit/{pdf,video}.md` §D/E established the concrete wiring; image follows the same shape
once its audit lands.

**video (has `window.filmcraft` already)** — smallest adapter. A wasm-side gate shim is
loaded with the embed page: it performs the `craft:1` handshake (token + parent-origin
validation) and only then exposes the existing object to the parent:
- `craft:open` → bytes → `File` → `filmcraft.importFiles(...)` / `openProject`
- `craft:command` → `filmcraft.execute(cmd, params)` / `filmcraft.request(...)`
- save → after `file.save` completes, `filmcraft.files()` + `filmcraft.readFile(path)` →
  `craft:save-request` to parent (bytes as transferables)
- ready → `window.filmcraftLoad.readyMs`; fatal → `craft:hello-ack {ok:false}` semantics
  via the `fatal` flag surfaced in `craft:ready`

**pdf (no JS object; in-process APIs)** — adapter inside `apps/pdfcraft-web` (~150 lines,
`embed` feature): bridge init on canvas start; `craft:open` → existing public
`open_bytes()`; `craft:command` → in-process `execute(command_id)`; **new save-bytes
write-back callback** hooked where saves today become Blob downloads
(`editing.rs:659`) — in embed mode bytes post to parent instead of (or in addition to)
the download.

**image (PhotoCraft)** — audit pending; expected same class as pdf (control protocol +
`apps/photocraft-web`). Adapter written after its `audit/image.md` §D lands.

**Host side (one shared implementation)** — `allternit-ai/src/components/craft/bridge.ts`:
session token, iframe lifecycle, protocol client, ArrayBuffer transfers, timeout/retry,
error surfaces. Kind editors (PDF/Image/Video) are thin wrappers over it.

## Crate layout (to implement)
`vendor/craft/craft-host/` — one Rust crate, `embed` feature:
- `protocol.rs`: serde types for every message above (shared with hosts in TS later).
- `bridge.rs`: wasm-bindgen `HostBridge` — init(origin, token), `on_message` handler
  dispatching to app-provided callbacks: `open_cb(bytes, name) -> Result<warnings>`,
  `save_cb() -> SaveRequest`, `command_cb(cmd, params) -> Result<json>`,
  `theme_cb(theme)`, plus `post_*` helpers for the app→host messages.
- Each app's `apps/<app>-web` gets a thin adapter (≤150 lines) wiring its engine session
  + document IO onto those callbacks, behind `#[cfg(feature = "embed")]`.

Versioning: protocol is versioned (`craft:1`); bump on breaking shape changes; apps
advertise the versions they speak in `craft:ready`.
