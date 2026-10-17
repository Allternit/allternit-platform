# macOS Accessibility (AX) transport for desktop `ui_bridge` adapters

Drives desktop apps by enumerating their accessibility tree instead of a debug port. Claude Desktop refuses
`--remote-debugging-port` without a vendor-signed token (see `../../claude-desktop/README.md`) and we do not bypass that.
The macOS Accessibility API is the OS-sanctioned automation interface: the user grants the permission, once.

## Pieces

| Piece | Where |
|---|---|
| `ax-bridge` Swift CLI (new; nothing shipped in the repo did tree + actions + observers, `cua-driver` and the Python `accessibility_adapter` were checked and are not JSON-lines/observer capable) | `native/ax-bridge/` (`swift build -c release`) |
| `AxBridgeDriver` (spawn, request/response, observer stream, timeouts), `AxDriver` interface | `_shared/ax/bridge.ts` |
| Selector format v1 (`role` + `subrole` + `label` regex + ancestor `path` + `nth`), pack versioning, drift evidence | `_shared/ax/selector.ts` |
| `AxReplayDriver` (static snapshots or a scripted app) | `_shared/ax/replay.ts` |
| Claude Desktop transport | `claude-desktop/ax/` (pack `claude-ax-v1`, bundle `com.anthropic.claudefordesktop`) |
| ChatGPT.app transport for `chatgpt-dots` | `chatgpt-dots/ax/` (pack `chatgpt-app-ax-v1`, bundle `com.openai.chat`) |
| ChatGPT desktop-app transport | `chatgpt-desktop/` (pack `chatgpt-desktop-ax-v1`, bundles `com.openai.chat` + `com.openai.codex`) |
| Kimi desktop-app transport | `kimi-desktop/` (pack `kimi-desktop-ax-v1`, bundle `com.moonshot.kimichat`) |
| Generic chat-app provider for route `desktop-app-ax` (contract table `AX_CHAT_CONTRACT`) | `_shared/ax/chat-app.ts` + offline `chat-app-fixtures.ts` |

## How the lane works

`AxClaudeDesktopDriver` / `AxChatGptAppDriver` implement the adapters' existing driver interfaces. A read is: `snapshot` ->
resolve the selector pack -> the same `Scenario` the DOM fixtures use -> `renderPage` -> the adapter's existing `classify`. So
the provider, drift latch, rate-limit cooldown, approval-id derivation and conformance suite are shared with the CDP path;
only the transport differs. Actions are AX `press` / `setValue` / `focus` by tree path found from the latest snapshot. `typeText`
reads the value back: an app that ignores an AX `setValue` reads as `false` (drift), never as "sent".
Guarantees are unchanged: lane `ui_bridge`, `best_effort`, events `inferred`. Approvals are still only answered by a human actor,
and the AX pack deliberately has no "always allow" button.

- Deterministic: no model in the loop. A selector that resolves nothing is `ADAPTER_DRIFT` (latched), with the missing critical keys
  and pack version in the driver's `lastDrift`. A pack in an unknown selector format is drift too, never a guess. Jev (policy head)
  is only for a batch decision over accumulated drift reports, not per click.
- Observers: `AxDriver.observe` streams `AXValueChanged`, `AXUIElementCreated`, `AXFocusedUIElementChanged`; `normalizeAxEvent` /
  `AxHintBuffer` turn them into transport-neutral hints (`text_changed`, `element_added`, `focus_moved`) so a poller can skip
  re-snapshotting an idle app. The snapshot remains the source of truth for events.
- Not trusted: `AXIsProcessTrusted()` false -> `DriverError("not_trusted")` -> `AUTH_REQUIRED` with `AX_NOT_TRUSTED_MESSAGE`
  (System Settings > Privacy & Security > Accessibility > enable the Allternit helper). The helper never calls the prompting
  variant and never opens System Settings.

## Config (per binding / lane)

| Env | Effect |
|---|---|
| `SUBS_GATEWAY_AX_BRIDGE_BIN` | absolute path of the built `ax-bridge` |
| `SUBS_GATEWAY_CLAUDE_DESKTOP_TRANSPORT=ax` (+ `SUBS_GATEWAY_CLAUDE_AX_CONSENT=1`) | claude-desktop over AX (default stays CDP) |
| `SUBS_GATEWAY_DOTS_TRANSPORT=chatgpt-app` (+ `SUBS_GATEWAY_CHATGPT_APP_AX_CONSENT=1`) | chatgpt-dots over native ChatGPT.app (default stays the Playwright browser) |

Without the per-app consent flag every call answers `LANE_BLOCKED` and the bridge is never spawned or attached. Grok Bot stays on CDP.
Programmatic: `createClaudeDesktopProvider({ transport: "ax", axBinPath, axConsented })`, `createChatGPTDotsProvider({ transport: "chatgpt-app", axBinPath, axConsented })`.

## Status: OFFLINE ONLY

Every selector is **unverified**. Trees under `claude-desktop/ax/fixtures/*.json` are hand-built structure, not recordings. Nothing
here has attached to a running app. Electron apps expose web content to AX only after `AXManualAccessibility=true` (set by `attach`);
whether Claude Desktop's remotely loaded claude.ai view exposes the composer/turns with the labels guessed here is the main open question.

**2026-10-17 (this Mac, read-only recon):** the Accessibility permission for the `ax-bridge` helper **is granted**
(`{"cmd":"trust"}` → `trusted:true`). None of the three vendor apps were running (ChatGPT.app `com.openai.codex` v26.908.70816,
Claude.app v2.42304.0, Kimi.app `com.moonshot.kimichat` v3.2.15 — all Electron (this Mac's ChatGPT.app is OpenAI's Electron build, not the native `com.openai.chat`); Claude re-verified to refuse a debug port,
the other two carry no such vendor check), and none were launched to force a recording, so every pack below stays
hand-built/unverified until a consented live session. Bundle analysis only — no app data was read.

## Needs a consented live session

1. Build: `cd native/ax-bridge && swift build -c release`; grant Accessibility to the helper (the user does this in System Settings).
2. Per app, with Eoj's OK: run `attach` + `snapshot`, save the real tree as a fixture, correct the pack, flip `confidence` to `verified` per key, bump `PACK_VERSION`.
3. Verify `setValue` on the composer actually reaches the web app (ProseMirror may need a focus + paste/keystroke fallback), and that Cowork tab, New task, permission prompt and usage-limit banner resolve. Confirm ChatGPT.app even exposes dots.
