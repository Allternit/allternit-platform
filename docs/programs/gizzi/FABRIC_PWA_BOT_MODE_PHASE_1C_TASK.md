# PHASE 1C TASK — PWA integration + ACI pull + web adoption + chrome

Read `docs/programs/gizzi/FABRIC_PWA_BOT_MODE_MAP.md` first — it is binding. Spec: `fabric-pwa-bot-mode-ui`
(rq-20260910-007, approved). This is sub-phase **1C of 3**. 1A is commit `63e9cb96e`. 1B is
commit `875d8ed1f` (reviewed: 44 vitest, tsc clean). **Preserve 1A/1B.** Do not rewrite
`src/components/bot-chat/` except adapter files you add there (or a sibling
`src/components/bot-chat/adapters/`). Zero new runtime deps. No OpenMausBot code.

## Binding decisions you must not violate

- SSE/stream events are the only transcript truth. No optimistic UI.
- `src/components/bot-chat/*` presentational files stay free of `src/fabric-session/` and
  `src/views/**` imports. Adapters may import stores/APIs; components may not.
- Named exports only. Tailwind + `cn` + `var(--…)` tokens. `@phosphor-icons/react`.
- Approvals never invent `grantKey`. Client never invents permission keys.
- Do **not** add react-router routes. `fabric-session/main.tsx` already wraps
  `BrowserRouter` but `App.tsx` does not use `<Routes>`. Navigation is a view-state switch
  (`dashboard | chat`) keyed by selected bot id.
- Do **not** implement policy/verdict UI (`openbot-policy-gateway`).
- Do **not** start a production deploy. No `confirm:true`. No git operations.

## Verified repo facts (re-check line numbers if files moved)

- PWA entry: `src/fabric-session/App.tsx` currently renders only `DashboardPage`.
- Dashboard: `src/fabric-session/pages/DashboardPage.tsx` — after sign-in, sections
  **"Needs you"** then **"Machines"**. Insert a **Bots** section between them (or immediately
  after Needs you).
- Auth: `PlatformAuthProvider` is already in `fabric-session/main.tsx`. Clerk sign-in is
  **not** a blocker. When `auth.isSignedIn`, hydrate bots.
- Roster: `useUnifiedRoster()` (`src/lib/bots/use-unified-roster.ts`) reads
  `useAgentsWithSwarms()` / agent store. **PWA never calls `fetchAgents` today** — ShellApp
  does (`src/shell/ShellApp.tsx`). You must call `useAgentStore.getState().fetchAgents()`
  once the PWA user is signed in, or the roster is empty.
- Web chat: `BotChatSessionView.tsx` uses `useChatSessionStore().sendMessageStream` /
  `abortGeneration`. Streaming UI today is a spinner at ~L407 (`"{bot} is thinking…"`).
  Transport: `mode-session-store.ts` ~L826 `chatApi.streamChat` callbacks
  (`onChunk`, `onThinkingChunk`, `onToolCall`, `onToolResult`, `onToolError`, `onDone`).
- Approvals wire: `cowork.approval_request` has **no option sets / no grantKey**. Fold
  already defaults a binary approve/deny pair. "Always allow" simply does not render.
- Routines: `src/lib/bots/bot-routine.service.ts` — `getRoutinesForBot(botId)`,
  `routinePrompt()`.
- ACI: `FabricSessionPanel.tsx` ~L199-222 **always** loops `fabricClient.streamAci(runId)`
  whenever `driveKind === 'aci'`. `FabricAciDrive` is presentational (screenshot prop).
- PWA smoke: `scripts/e2e/bot/bot-e2e-pwa.cjs` (Playwright, 1280×900 today). MODULE_FALLBACK
  currently points at `~/altw/allternit/node_modules` — if you touch the file,
  also accept the worktree's `node_modules` / `surfaces/ai.allternit.com/node_modules`.
- Desktop lock: changes under `src/components/` and `src/fabric-session/` reach the
  desktop static bundle. Before claiming done, run
  `node scripts/release-preflight.mjs` from the worktree root if that script exists;
  if it fails for an unrelated pre-existing reason, record it honestly in NOTES (do not
  "fix" unrelated desktop lock issues in this phase).

## Exact scope

### 1. Fold adapter (web + PWA share this)

Add `src/components/bot-chat/chat-stream-adapter.ts` (name is flexible, keep it next to
the fold):

- `streamCallbacksToEvents(callbacks from chatApi.streamChat) → applyEvent(...)`
- Map: user send → `message.user`; thinking chunks → `thinking.delta`; text chunks →
  `message.delta`; tool call/result/error → `tool.call` / `tool.result`; abort/error →
  `error`; done → `turn.completed`.
- Approval events (`cowork.approval_request` / `approval_result`) → `approval.requested`
  / `approval.resolved` when they appear on the session; if the web path has no approval
  events yet, still export the mapper and a unit test with a synthetic payload (binary
  options, no grantKey).
- Pure-ish: the adapter produces fold events; `BotTranscript` still only receives
  `BotChatTranscript`. Colocated `chat-stream-adapter.test.ts`.

### 2. PWA view switch + Bots section + chat page

- `App.tsx`: `view: 'dashboard' | 'chat'` + `selectedBotId`. Dashboard stays default.
  Chat view is a full-screen column: header (back, bot name, WaitingOnYouPill if pending),
  `BotTranscript`, `BotComposer`, top-pinned `ApprovalPill` with
  `style={{ paddingTop: 'env(safe-area-inset-top)' }}`. Composer bar uses
  `padding-bottom: env(safe-area-inset-bottom)`.
- New page/module under `src/fabric-session/pages/` (e.g. `BotsChatPage.tsx`) **or**
  inline in App — do not add a router. The chat page is the consumer of bot-chat
  components; it owns the fold state + adapter.
- Dashboard **Bots** section: list from `useUnifiedRoster()` after `fetchAgents()`.
  Each row: `BotAvatar` (or a simple initial circle if avatar import pulls too much
  web chrome), displayName, tagline, `WaitingOnYouPill` when that bot has a pending
  approval. Tap → switch to chat. Empty state: one plain sentence, Register 1, no hype.
- Hydrate: when `auth.isSignedIn`, `fetchAgents()`. Do not block the machines list on
  bot fetch failure — bots empty-state, machines still work.
- Composer wiring on the chat page:
  - `onSend` → create/resume canonical bot session via the same ChatSessionStore path
    `BotChatSessionView` uses (`sendMessageStream`), feeding chunks through the adapter
    into the fold. Spec recommendation: reuse canonical chat SSE, not the fabric relay.
  - `suggestions` from `getRoutinesForBot` (label = title, prompt = `routinePrompt()`).
  - `commands` for `/` HUD from the same routines (`action: 'prompt'`).
  - `actions`: attach (no-op or file input if already easy), new thread (clear local
    fold only — do not fork the canonical session), watch computer (toggles ACI pull),
    share transcript (navigator.share or clipboard), Interrupt (`danger`, only while
    `busy`, calls `abortGeneration`).
  - `status` for send errors.

### 3. Web adoption — `BotChatSessionView.tsx`

Replace the spinner-only streaming block and the message list with `BotTranscript`
driven by the same adapter (fold rebuilt from `session.messages` on load, then live
callbacks). Replace `ChatComposer` in this view with `BotComposer` using the same
routines/actions pattern. Keep: back button, computer sidecar (`BotComputerViewport`),
model selection, existing session create/send/stop behavior. Do not restyle the whole
page; swap the transcript+composer guts. `data-bot-composer` may move onto BotComposer
or a wrapper so any existing selectors still work.

### 4. ACI pull mode

In `FabricSessionPanel.tsx`, **do not** start `streamAci` on mount whenever
`driveKind === 'aci'`. Default watching = off. Start the iterator only when the user
turns watch on (composer "watch computer" and/or a control on `FabricAciDrive`). Abort
on toggle-off and on `document.visibilitychange` hidden. Keep last screenshot. Drive
chrome should make the off state obvious ("Watch computer" / "Stop watching") — Register 1.

If `FabricAciDrive` needs a `watching` / `onToggleWatch` prop, add it. Do not push
frames while the PWA is backgrounded.

### 5. Touch / safe-area chrome pass (PWA)

- 44px min targets on new interactive chrome (existing Machines rows: do not restyle
  the whole dashboard; new Bots rows + chat chrome must hit 44px).
- Header / composer / approval pill: `env(safe-area-inset-*)`.
- Transcript: `touch-action: pan-y` (already on BotTranscript).
- `prefers-reduced-motion` already on 1A/1B primitives — do not reintroduce undamped
  per-token animation.

### 6. PWA smoke

Extend `scripts/e2e/bot/bot-e2e-pwa.cjs` (or add a sibling `scripts/e2e/bot/bot-e2e-pwa-fabric.cjs` if the existing
file is the web-app PWA, not fabrictransport):

- Viewport **390×844**.
- Assert the Fabric Session PWA (fabric-session entry, not the full web shell) shows a
  Bots section after sign-in **when a saved session exists**. If fabric-session is a
  different origin/port than `:3013`, document the command and skip honestly when the
  server isn't running — do **not** start a long-lived Vite server from this task
  unless one is already up. A 390×844 screenshot of the dashboard+bots empty/list
  state via Playwright against a running server is enough. If no server and no
  storageState, record SKIP in NOTES with the exact command a human would run.

### 7. Docs

Add a **PWA** section to `docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md`: Fabric Transport is the phone
surface; bot roster → chat; shared `src/components/bot-chat/`; ACI is watch-to-pull;
SSE/stream is the only truth. Register 1. Short.

Update `.steering/checkpoint.md`. Append `### fabric-pwa-bot-mode-ui <ts>` to
`.allternit/shared-context.md` when present. Evidence under
`~/.agent-orchestrator/evidence/fabric-pwa-bot-mode-ui/`.

## Verification

From `surfaces/ai.allternit.com`:

```
npx vitest run src/components/bot-chat
npx tsc --noEmit
```

Also run any new tests you add next to the adapter / PWA pages.

If `node scripts/release-preflight.mjs` exists at repo root, run it and paste the
summary into NOTES (pass or honest fail).

NO git. NO production deploy.

## Deliverable sentinel

Write `docs/programs/gizzi/FABRIC_PWA_BOT_MODE_PHASE_1C_NOTES.md` with frontmatter `status`,
`files_changed`, `deviations`, `remaining` + prose (what was built, independent test
summary, what was skipped and why). `status: done` = done. Existing with `status: done`
means you are finished — do not keep going.
