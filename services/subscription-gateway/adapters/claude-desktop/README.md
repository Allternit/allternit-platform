# claude-desktop adapter (Agent Gateway, lane `ui_bridge`, guarantee `best_effort`, mode `linked`)

Drives the user's own signed-in **Claude desktop app** (`/Applications/Claude.app`, Electron, v2.16120.0, bundle id
`com.anthropic.claudefordesktop`) including its **Cowork** agent mode, over local CDP. Account = the user's own Claude
subscription; no API key, and Allternit never reads claude.ai cookies/tokens. Same vendor id (`claude`) as the official-lane adapter
`claude-managed-agents`, so both lanes share the Claude look pack. Events are `best_effort`/`inferred`, never `exact`.

**STATUS: offline-complete, NOT live-verified, and the CDP route is probably vendor-blocked (see first finding).**

## Recon (read-only, bundle extracted with `@electron/asar`; the running app was not touched, no user-data read, no network)

1. **Claude Desktop refuses a debugging port.** `.vite/build/index.pre.js` keeps a deny-list (`remote-debugging-port`,
   `remote-debugging-pipe`, `ignore-certificate-errors`, `host-resolver-rules`, `disable-web-security`, ...). If any is on the command
   line the app prints "Claude: refusing to start - a debugging or network-override switch is present" and `process.exit(1)`, unless
   `CLAUDE_CDP_AUTH` is a valid token: three dot-separated parts, timestamp under 5 minutes old, payload equal to the base64 of
   `CLAUDE_USER_DATA_DIR`, and an Ed25519 signature verified against an Anthropic public key embedded in the bundle. Allternit cannot
   mint that token and does not try to bypass the check. So `launchWithDebugPort()` returns `LANE_BLOCKED` unless the user supplies a
   vendor-issued `authToken` + `userDataDir`. For ordinary users this lane is unavailable; the supported path is the Managed Agents lane.
   (Also: the flag cannot be added to an already running app, and this adapter never quits or relaunches one.)
2. **UI is remote, not bundled.** The main window's webContents does `loadURL(claude.ai)` (redirect/recovery paths all call
   `loadURL(TO())` with the claude.ai origin). `renderer/*.html` (main_window, quick_window, about, find_in_page, buddy, ...) are thin shell
   pages. So the chat DOM is claude.ai's and is not in the bundle; the CDP target to attach to is the page whose URL starts with
   `https://claude.ai/`. The preload exposes private IPC (`LocalAgentModeSessions_*` etc.); not used, DOM only.
3. **Cowork** is "local agent mode": the bundle carries UI strings `Cowork`, `New Task`, `New Chat`, `Allow Claude to use {toolName}?`,
   `Allow Claude to cowork in {path}?`, `Trust {directory} and start a Cowork task?`, `Your usage limits`, `Sign in again`. Those are native-shell
   dialogs; the in-page Cowork task view markup is not visible from the bundle.
4. Cowork tasks also run in a VM/sandbox managed by the app (Cowork VM bundle/sessions); irrelevant to the DOM lane but means task state is not
   fully mirrored in the page, hence tasks/computer stay `UNSUPPORTED`.

## Selector confidence (`selectors.ts`, version `v1`) - all UNVERIFIED against the running app

| Key | Hook | Confidence |
|---|---|---|
| composer | `[data-testid=chat-input]`, `.ProseMirror[contenteditable=true]`, `role=textbox` (label "Write your prompt to Claude") | Medium (recalled from public claude.ai) |
| turn / role | `[data-testid=user-message]` (user), `.font-claude-response` (assistant) | Medium |
| turnContent | `.standard-markdown, .progressive-markdown`, fallbacks | Medium/Low |
| streaming | `[data-is-streaming=true]` or button "Stop response" | Medium |
| send / stop / new chat | accessible names "Send message", "Stop response", "New chat" | Medium (English only) |
| Cowork mode / new task | tab or button named "Cowork", then "New task" (`aria-selected` = active) | Low (name from app catalog, markup guessed) |
| approval card | ancestor text `Allow Claude to ...` / `needs your permission`; buttons "Allow once", "Deny"/"Don't allow" | Low |
| tool / artifact cues | `[data-testid=tool-use-block]`, `[data-testid=artifact-block-cell]` | Low (designed) |
| usage limit | alert/status text `usage limit`, `limit reached`; retry hint parsed from `resets in N hours` / `resets at H:MM PM`, else 1 h | Low |
| logged out | buttons "Log in", "Sign in", "Continue with Google/Email" and no composer | Medium |
| bot check | Turnstile / "verify you are human" text -> `LANE_BLOCKED` latched | Low |

Non-English locales read as drift by design. "Always allow" is deliberately never clicked: only one-shot approval is offered, and only for a `human` actor.

## Behaviour
- Entry points: `claude-desktop` = new Chat; `claude-desktop:cowork` = switch to Cowork then New task (per-binding via `externalAgentId`,
  same convention as grok-bot). Anything else is `CONTEXT_NOT_FOUND`.
- Declared: context open / message (paced, idempotent by correlation id, serialized) / events (polling, cursor replay) / cancel (Stop,
  `confirmed` only when the page shows it stopped) / close (never deletes the chat) / approvals list + human-only respond / health.
- `UNSUPPORTED`: steer, resume/adopt, memory, tasks, computer, artifacts read/write (visible artifacts and tool blocks surface only as inferred
  `agent.tool.called` cues with `kind: tool_use | artifact`), sync, snapshot.
- Errors: app not running or no debug port -> `VENDOR_UNAVAILABLE`; signed out -> `AUTH_REQUIRED`/`AUTH_REVOKED`; usage/rate limit -> `RATE_LIMITED`
  (+cooldown, `retryAfterMs`); verification banner -> `LANE_BLOCKED` (latched); drift -> `ADAPTER_DRIFT` (latched, `clearHalt()` releases);
  own pacing limits -> `RATE_LIMITED`; approval answered in-app -> `cancelled`.
- Registration: `aai.ts` exports `createAaiRegistration(env)` (port from `SUBS_GATEWAY_CLAUDE_DESKTOP_CDP_PORT`, default 9222); the CDP driver connects lazily.

## Files
`manifest.ts` · `selectors.ts` · `driver.ts` (seam) · `cdp-driver.ts` (loopback-only, consent-gated `launchWithDebugPort`) · `observe.ts` (HTML -> page state) ·
`minidom.ts` · `provider.ts` · `replay-driver.ts` + `fixtures/` (hand-built; regenerate with
`node node_modules/tsx/dist/cli.mjs adapters/claude-desktop/fixtures/generate.mjs`) · `look-profile.json` + `assets/claude.png` (icon from `electron.icns`, the app's own icon).
Tests: `test/claude-desktop-adapter.test.ts` (conformance passes every declared area offline).

## Needs a consented live session (nothing below has been done)
1. A vendor-issued `CLAUDE_CDP_AUTH` token, or an official debug build, otherwise steps 2-4 are impossible.
2. Quit Claude yourself, relaunch through `launchWithDebugPort({ userConsented: true, authToken, userDataDir, port })`.
3. Confirm the CDP target is `https://claude.ai/...` and every selector above; record the real markup, fix `selectors.ts`, regenerate fixtures, bump the version.
4. Confirm the Cowork tab/New task names, the permission-card markup, the usage-limit banner and the logged-out gate; then update this table.

## Alternative transport: macOS Accessibility (no debug port)

`createClaudeDesktopProvider({ transport: "ax", axBinPath, axConsented: true })` (or `SUBS_GATEWAY_CLAUDE_DESKTOP_TRANSPORT=ax`)
drives the app through its accessibility tree instead of CDP. Selector pack `claude-ax-v1` is unverified. See `../_shared/ax/README.md`.
