# grok-bot adapter (Agent Gateway, lane `ui_bridge`, guarantee `best_effort`, mode `linked`)

Drives the user's own, already signed-in **Grok Bot** desktop app (`/Applications/Grok Bot.app`, Electron 42, v0.61.0,
bundle id `com.anysphere.sand`) over local CDP. Not an official API. Events are `best_effort`/`inferred`, never `exact`.

## Recon findings (read-only, from the app bundle; no live app touched, no user-data read, no network)

- **UI is bundled locally.** `main-app.cjs` calls `loadFile(dist/renderer/index.html)`; `loadURL(devServerUrl)` exists only
  for dev builds. The renderer CSP allows network only to `https://grok.com` (`connect-src`). So CDP attaches to a
  `file://.../renderer/index.html` page, not a remote site. The app is a Cursor-derived codebase (`@anysphere/*`, "sand").
- **No debugging port is baked in.** Nothing in the main bundle enables `--remote-debugging-port`; it must be passed at
  launch. A running app cannot be attached to, so the wizard asks the user to quit it, then `launchWithDebugPort()` starts it
  (`open -a "Grok Bot" --args --remote-debugging-port=N`) only with `userConsented: true`, and never kills a running instance
  (returns `LANE_BLOCKED` asking the user to quit it).
- **IPC is not usable.** The preload exposes `desktop`, `coordinatorPort`, `desktopMount` via `contextBridge` and an internal
  MessagePort to a node-agent coordinator. Private, versioned, sandboxed: we do not touch it. DOM only.
- **Stable DOM hooks (verified in bundle):** `.sand-message[role=article][data-role=user|assistant]`,
  `.sand-message-content`, `.sand-transcript-row`, composer `.ui-prompt-input-editor__input` (ProseMirror,
  `contenteditable`, `role=textbox`). `data-testid` is almost unused (only publish/credential UI), so strategy is
  **role + accessible name + class hooks**, with ordered fallbacks.
- **Accessible names come from the Lingui English catalog** (locale `en`): `Send message`, `Stop`, `New chat`, `Sign in`,
  `Allow once`, `Always allow`, `Deny`, `Skip`, `Approve`, `Needs your approval`, `Routines`, `Routine`, `Generating…`,
  `Working`, `Usage limit reached`, `This request has been rate limited, try again shortly`. Non-English locales will
  read as drift by design.
- **Concepts:** the app hosts "Bots" (agents) with chats, a "Routines" (automations) section, per-action approval cards
  (Allow once / Always allow / Deny or Skip), usage-limit and rate-limit messages. Bots keep memory across chats, so context
  isolation is declared `shared`; one main window means `maxParallel: 1`.

## Selectors strategy (`selectors.ts`, version `v1`)
Named keys, each with ordered CSS fallbacks; `composer` and `appRoot` are critical. Missing composer with no Sign-in gate, or an
assistant turn without `.sand-message-content`, is **drift**: provider latches `ADAPTER_DRIFT` and stops. Buttons and approval
cards are located by accessible name (`NAMES`), approval cards by the ancestor whose text says it needs approval.

## Files
`manifest.ts` (AdapterManifest + `agent` section, auth descriptor `desktop_session`, terms warning, pacing) ·
`cdp-driver.ts` (loopback-only CDP client, `launchWithDebugPort`) · `driver.ts` (transport seam) · `observe.ts` (HTML → page state) ·
`minidom.ts` (dependency-free parser/selector engine so live and fixture pages share one code path) · `provider.ts` (`AaiProvider`) ·
`replay-driver.ts` + `fixtures/` (offline) · `look-profile.json` + `assets/grok-bot.png` (look pack; icon from `icon.icns`) · `index.ts` (factory).
Regenerate fixtures: `node node_modules/tsx/dist/cli.mjs adapters/grok-bot/fixtures/generate.mjs`.

## Behaviour
- Declared: context open (new chat) / message (paced, idempotent by correlation id, serialized on one composer) / events (polling of the
  transcript, cursor replay) / cancel (Stop button, `confirmed` only when the page shows it stopped) / close (never deletes the chat) /
  approvals list + human-only respond / health.
- `UNSUPPORTED`: steer, resume/adopt, memory read/write/snapshot (opaque), tasks (routines are surfaced as `agent.tool.called` cues only),
  computer, artifacts, sync, snapshot.
- Errors: app not running or no debug port → `VENDOR_UNAVAILABLE`; signed out → `AUTH_REQUIRED`/`AUTH_REVOKED`; rate/usage limit banner →
  `RATE_LIMITED` (+cooldown, `retryAfterMs`); verification/unusual-activity banner → `LANE_BLOCKED` (latched); drift → `ADAPTER_DRIFT` (latched);
  self pacing limits → `RATE_LIMITED`. `clearHalt()` releases a latch after the user resolves it or the selectors are updated.
- Approvals are only ever answered by a `human` actor; anything else is `APPROVAL_REQUIRED`. An approval answered inside the app becomes `cancelled` (outcome not observable).

## UNVERIFIED LIVE (needs one user-consented session)
Written from the bundle, never run against the live app. To confirm: (1) app honours `--remote-debugging-port` and the renderer target URL match;
(2) composer `Input.insertText` + `Send message` click actually submits; (3) streaming is signalled by the `Stop` button; (4) the real approval-card,
banner and routine markup (fixtures use inferred containers); (5) signed-in detection (composer visible, no Sign-in gate); (6) whether `New chat` is a
button with that name. Fixtures are hand-built, so selector drift shows up first at that gate.

## Registration (src/aai/registry.ts, owned elsewhere)
`import { grokBot } from "../../adapters/grok-bot/index.js"; registry.register({ adapterId: grokBot.adapterId, manifest: grokBot.manifest, create: grokBot.create });`
