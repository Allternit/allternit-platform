# chatgpt-desktop adapter (route `desktop-app-ax`, lane `ui_bridge`, guarantee `best_effort`, mode `linked`)

Drives the user's own signed-in **ChatGPT desktop app** on a Mac through the macOS Accessibility tree, using the shared
`AxChatAppProvider` (`_shared/ax/chat-app.ts`) over the `ax-bridge` helper. Vendor id `openai` (same as `chatgpt-subscription`
and `chatgpt-dots`, so the route router groups them). No API key; Allternit never reads ChatGPT cookies, tokens or user data.

**STATUS: offline-complete, NOT live-verified.** Every selector in `selectors.ts` is `unverified`.

## What is installed on this Mac (read-only recon, 2026-10-17)

| | |
|---|---|
| Path | `/Applications/ChatGPT.app` |
| Bundle id | `com.openai.codex` (NOT the native `com.openai.chat`) |
| Version | 26.908.70816 |
| Kind | **Electron** (`Contents/Resources/app.asar`, `electron.icns`, `Frameworks/Codex Framework.framework` = the renamed Electron framework) |
| Running during recon | No (only `cua_node` helper processes from its bundle, spawned by the Codex CLI's computer-use plugin) — no AX tree could be read; the app was not launched to record one |
| Debug-port deny-list in the bundle | None found (`grep` of `app.asar` for `remote-debugging-*` deny logic: no hits) |

The task brief assumed a native ChatGPT app; on this machine `/Applications/ChatGPT.app` is OpenAI's Electron build.
The adapter therefore tries `com.openai.chat` first (native build, where installed) and falls back to `com.openai.codex`.

## Contract (spec §3) — route `desktop-app-ax`

| Item | Status | How / why |
|---|---|---|
| Start a chat | Implemented | `contextOpen` presses **New chat**. |
| Continue a chat | Implemented | `contextMessage` writes the composer, reads the value back (an ignored `setValue` reads as drift, never "sent"), presses **Send**. Replays of a correlationId return the first result. |
| Pick the model/mode | Implemented | `models()` opens the picker, lists its menu items, closes it; `selectModel(name)` (exact, else unique prefix); `contextOpen({ model })` picks before the first message. Unknown name → `UNKNOWN` with the real options. |
| Read the reply as it streams + know when finished | Implemented | `agent.message.delta` while the app's **Stop** control is in the tree; `agent.message.completed` (`finishedBy: "stop_control_gone"`) when it leaves and the reply has text. Not "screen stopped changing". |
| Stop mid-stream | Implemented | `contextCancel` presses **Stop**; `confirmed: true` only when the next read shows streaming ended. |
| Send attachments (image, file) | **Blocked** | Attaching opens the macOS open panel (separate process) or needs pasteboard injection; both are outside the `ax-bridge` contract. `sendAttachment()` answers `UNSUPPORTED` with `details.contractItem = "attachments.send"`. |
| Get outputs back | **Partial** | Text: verbatim AX static-text values (no OCR), paragraph breaks kept (`exactText`). Generated images/files: blocked — needs a vendor save dialog; `outputFiles()` answers `UNSUPPORTED`. |
| List and reopen past threads | Implemented | `threads()` lists sidebar rows as `{ id, title }`; `contextOpen({ adoptContextId: "thread:<id>" })` presses the row and continues it (history is not re-emitted as new replies). |
| Usage-limit / sign-out / verification wall as typed errors | Implemented | Usage limit → `RATE_LIMITED` + `retryAfterMs` from the banner ("in 2 hours", "at 2:30 PM"); signed out → `AUTH_REQUIRED` (first run) / `AUTH_REVOKED`; verification wall → `LANE_BLOCKED` (latched); Accessibility not granted → `AUTH_REQUIRED` with how-to; app not running → `VENDOR_UNAVAILABLE`; selectors gone → `ADAPTER_DRIFT` (latched, with the missing key + pack version). |

Because two items are not fully implemented, **this route is not eligible under §3** (`AX_ROUTE_ELIGIBLE === false`);
the router must not offer it until attachments and file outputs land.

## `desktop-app-cdp`

Not registered. The Electron build shows no vendor deny-list for `--remote-debugging-port` (unlike Claude), so CDP is not
vendor-blocked as far as the bundle shows. But the flag only works at launch: using it means relaunching the app, which this
adapter never does to an app the user has open, and nobody has probed a live launch. A CDP adapter would need: a
consent-gated `launchWithDebugPort` (copy `claude-desktop/cdp-driver.ts`), a loopback-only port, and a launch-time probe
recorded per app version (the router's §2 debug-mode rule).

## Windows (out of scope)

The Windows ChatGPT app is also Electron-based; the route there would be CDP (or UIA via a Windows ax-bridge equivalent).
It needs a Windows helper speaking the same JSON-lines protocol, a UIA selector pack, and a live recon on Windows.

## Wiring

- Registration: `aai.ts` → `createAaiRegistration(env)`. Every call answers `LANE_BLOCKED` until
  `SUBS_GATEWAY_CHATGPT_DESKTOP_AX_CONSENT=1`, and `VENDOR_UNAVAILABLE` until `SUBS_GATEWAY_AX_BRIDGE_BIN` names the helper.
- Route ids: `manifest.ts` exports `ROUTE_IDS = ["desktop-app-ax"]` and the manifest carries `routes`.
- Fixtures: `fixtures/*.json` (hand-built AX trees for idle, streaming, complete, model-picker, rate-limit, verification,
  logged-out, drift; regenerate with `UPDATE_AX_FIXTURES=1 npx vitest run test/desktop-app-routes.test.ts`),
  `fixtures/offline.ts` (scripted app for `POST /aai/conformance/chatgpt-desktop`).
- Tests: `test/desktop-app-routes.test.ts`.

## First live session (needs Eoj's OK)

1. Open ChatGPT yourself and sign in. 2. Build/grant `ax-bridge` (see `_shared/ax/README.md`). 3. Snapshot the tree
read-only, save it under `fixtures/live-*.json`, correct `selectors.ts`, mark verified keys `verified`, bump `PACK_VERSION`.
4. The orchestrator runs the live conformance; no test here ever sends a real message.
