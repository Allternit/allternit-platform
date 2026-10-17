# kimi-desktop adapter (route `desktop-app-ax`, lane `ui_bridge`, guarantee `best_effort`, mode `linked`)

Drives the user's own signed-in **Kimi desktop app** on a Mac through the macOS Accessibility tree, using the shared
`AxChatAppProvider` (`_shared/ax/chat-app.ts`) over the `ax-bridge` helper. Vendor id `kimi` (same as `kimi-subscription`).
No API key; Allternit never reads Kimi cookies, tokens or user data.

**STATUS: offline-complete, NOT live-verified.** Every selector in `selectors.ts` is `unverified`.

## What is installed on this Mac (read-only recon, 2026-10-17)

| | |
|---|---|
| Path | `/Applications/Kimi.app` |
| Bundle id | `com.moonshot.kimichat` |
| Version | 3.2.15 (Electron 43.7.3, productName `kimi-desktop`) |
| Kind | **Electron** (`Frameworks/Electron Framework.framework`, `app.asar`) |
| Running during recon | No (only a leftover `chrome_crashpad_handler`) — no AX tree could be read; the app was not launched to record one |
| Electron fuses (`@electron/fuses read`) | RunAsNode on, NodeCliInspectArguments **off**, EmbeddedAsarIntegrityValidation on, OnlyLoadAppFromAsar on |
| Debug-port deny-list in the bundle | None found (`grep` of `app.asar` for `remote-debugging-*`: no hits; the only "refusing to start" strings are its own daemon/runtime) |

## Contract (spec §3) — route `desktop-app-ax`

Same provider as `chatgpt-desktop`, so the same table applies:

| Item | Status | How / why |
|---|---|---|
| Start a chat | Implemented | `contextOpen` presses **New Chat** (`新建会话` also matched). |
| Continue a chat | Implemented | Composer write + read-back, then **Send** (`发送`). Idempotent per correlationId. |
| Pick the model/mode | Implemented | `models()` / `selectModel(name)` / `contextOpen({ model })` via the model picker (`Model` / `模型`). |
| Read the reply as it streams + know when finished | Implemented | Deltas while **Stop** (`停止`) is in the tree; completed when it leaves. |
| Stop mid-stream | Implemented | `contextCancel` presses Stop; confirmed only when streaming ended. |
| Send attachments (image, file) | **Blocked** | macOS open panel / pasteboard are outside the `ax-bridge` contract; `sendAttachment()` → `UNSUPPORTED`. |
| Get outputs back | **Partial** | Text verbatim from AX static text; generated files blocked (`outputFiles()` → `UNSUPPORTED`). |
| List and reopen past threads | Implemented | `threads()` from the History sidebar (`历史会话`); `contextOpen({ adoptContextId: "thread:<id>" })`. |
| Usage-limit / sign-out / verification wall as typed errors | Implemented | English and Chinese banners: `额度`/`上限`/`稍后再试` → `RATE_LIMITED`; `安全验证`/`人机验证` → `LANE_BLOCKED`; signed out → `AUTH_REQUIRED`/`AUTH_REVOKED`; drift → `ADAPTER_DRIFT`. |

Not eligible under §3 until attachments and file outputs land (`AX_ROUTE_ELIGIBLE === false`).

## `desktop-app-cdp`

Not registered yet. Kimi is the best CDP candidate of the three: Electron, and no vendor check against
`--remote-debugging-port` in its bundle (the disabled `EnableNodeCliInspectArguments` fuse only blocks Node's `--inspect`,
not Chromium's debug port). It still needs the app **started** with the flag, which this adapter never does to an app the
user has open, and nobody has probed a live launch. To add it: a consent-gated launcher modelled on
`claude-desktop/cdp-driver.ts` (loopback-only port, only when Kimi is not already running or the user quits it themselves),
a DOM selector pack from a live recon, and the router's launch-time probe recorded per app version.

## Windows (out of scope)

The Windows Kimi app is the same Electron build; the route there would be CDP (same caveats) or UIA through a Windows
helper speaking the ax-bridge JSON-lines protocol.

## Wiring

- Registration: `aai.ts`; `LANE_BLOCKED` until `SUBS_GATEWAY_KIMI_DESKTOP_AX_CONSENT=1`, `VENDOR_UNAVAILABLE` until
  `SUBS_GATEWAY_AX_BRIDGE_BIN` is set.
- Route ids: `manifest.ts` `ROUTE_IDS = ["desktop-app-ax"]`.
- Fixtures: `fixtures/*.json` (hand-built), `fixtures/offline.ts` (conformance). Tests: `test/desktop-app-routes.test.ts`.
