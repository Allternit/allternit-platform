---
status: done
files_changed:
  - surfaces/ai.allternit.com/src/components/bot-chat/chat-stream-adapter.ts
  - surfaces/ai.allternit.com/src/components/bot-chat/chat-stream-adapter.test.ts
  - surfaces/ai.allternit.com/src/components/bot-chat/WaitingOnYouPill.tsx
  - surfaces/ai.allternit.com/src/components/bot-chat/ApprovalPill.tsx
  - surfaces/ai.allternit.com/src/lib/bots/bot-routine.service.ts
  - surfaces/ai.allternit.com/src/lib/bots/bot-chat-composer.ts
  - surfaces/ai.allternit.com/src/lib/agents/mode-session-store.ts
  - surfaces/ai.allternit.com/src/fabric-session/App.tsx
  - surfaces/ai.allternit.com/src/fabric-session/pages/DashboardPage.tsx
  - surfaces/ai.allternit.com/src/fabric-session/pages/BotsRosterSection.tsx
  - surfaces/ai.allternit.com/src/fabric-session/pages/BotsRosterSection.test.tsx
  - surfaces/ai.allternit.com/src/fabric-session/pages/BotsChatPage.tsx
  - surfaces/ai.allternit.com/src/views/bots/BotChatSessionView.tsx
  - surfaces/ai.allternit.com/src/components/dispatch/FabricSessionPanel.tsx
  - surfaces/ai.allternit.com/src/components/dispatch/FabricSessionDriveViews.tsx
  - docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md
  - bot-e2e-pwa-fabric.cjs
  - docs/programs/gizzi/FABRIC_PWA_BOT_MODE_PHASE_1C_NOTES.md
  - .steering/checkpoint.md
  - .allternit/shared-context.md
deviations:
  - "PWA roster uses an initial circle, not BotAvatar — BotAvatar imports
    src/views/** (GizziMascot, AgentMascotPreview). Allowed by the task."
  - "Exported existing `routinePrompt` from bot-routine.service.ts so chips/HUD
    can use the canonical `[bot:<name>] <title>` prompt. Format unchanged."
  - "`sendMessageStream` wrapped onToolResult/onToolError without forwarding
    consumer callbacks. One-line forwards added so the fold adapter actually
    receives tool events. onChunk/onThinking/onToolCall already forwarded."
  - "WaitingOnYouPill renders a <span> when onClick is omitted so a roster
    row button can host it without nested <button>s. Still a <button> when
    clicked. React import added on WaitingOnYouPill + ApprovalPill for
    vitest's classic JSX transform (same 1B fix)."
  - "PWA 390×844 smoke SKIP against this worktree's Vite: auth is unconfigured
    ('Authentication is unavailable in this build'). :3013 is a sibling
    worktree (allternit-session-botmode-clerk-0910) without this phase's Bots
    section. See skipped section below."
remaining:
  - "Live 390×844 smoke with Clerk keys on this worktree: `cd surfaces/ai.allternit.com && pnpm dev` then `PLATFORM_URL=http://localhost:3013 node bot-e2e-pwa-fabric.cjs` (needs /tmp/botmode-e2e-state.json from bot-e2e-live.cjs)."
  - "Approval answers update the local fold only. No cowork/permission-store POST yet — matches the spec stub (server has no option sets / grantKey today)."
  - "PWA chat 'Watch computer' sets App-level watching; `streamAci` only runs while FabricSessionPanel is mounted (machine drive). Toggling watch from bot chat does not pull frames until a machine session is open."
  - "Bundle review carried from 1A: Streamdown markdown in SettledBubble/StreamingBubble."
  - "Nothing past 1C was started."
---

# Phase 1C notes — PWA integration + ACI pull + web adoption

## What was built

Shared fold adapter plus both consumers (Fabric Session PWA and web
`BotChatSessionView`). 1A/1B presentational files preserved except the two
pill files above. Zero new runtime deps. No react-router. No policy UI. No
git. No deploy.

### Fold adapter

`src/components/bot-chat/chat-stream-adapter.ts` — pure. Maps
`sendMessageStream` callbacks onto `TranscriptEvent`s (`message.user` via
`userSendEvent`, thinking/text/tool/error/done). Cowork
`approval_request` / `approval_result` mappers: binary approve/deny when
options are omitted, **no invented grantKey**. `messagesToTranscript`
rebuilds history and skips empty assistant placeholders.

### PWA

`App.tsx` view-state switch `dashboard | chat` keyed by `selectedBotId`.
Dashboard **Bots** section sits between Needs you and Machines. Signed-in
`fetchAgents()` hydrates the roster; fetch failure does not block machines.
Empty copy: "No bots on this account yet." Chat page owns the fold, pins
`ApprovalPill` under `env(safe-area-inset-top)`, composer
`env(safe-area-inset-bottom)`, 44px targets. Composer: routines as chips +
`/` HUD, actions attach / new thread (clears local fold only) / watch
computer / share / Interrupt-red-while-busy.

### Web adoption

`BotChatSessionView` swaps the spinner message list for `BotTranscript` and
`ChatComposer` for `BotComposer` (same routines/actions). Keeps back,
computer sidecar, model picker, create/send/stop. `data-bot-composer`
stays on the wrapper.

### ACI pull

`FabricSessionPanel` no longer starts `streamAci` whenever `driveKind ===
'aci'`. Default watching = off. Stream runs only while watching **and**
`document.visibilityState === 'visible'`; abort on toggle-off or hidden;
last screenshot kept. `FabricAciDrive` shows "Watch computer" / "Stop
watching".

## Independent test summary

```
npx vitest run src/components/bot-chat src/fabric-session/pages/BotsRosterSection.test.tsx

 ✓ chat-stream-adapter.test.ts  (7)
 ✓ BotsRosterSection.test.tsx   (2)
 ✓ 1A/1B suites unchanged       (44)

 Test Files  8 passed (8)
      Tests  53 passed (53)
```

Adapter tests: stream mapping, tool error + stream error, cowork approval
with binary defaults and no grantKey, grantKey passthrough only, result
timeout/deny/approve, history rebuild skipping placeholders. Roster tests:
empty state, tap-select, Waiting-on-you when pending.

```
npx tsc --noEmit   (from surfaces/ai.allternit.com)
0 errors
```

`node scripts/release-preflight.mjs` — **35 passed, 0 failed**.

## What was skipped and why

PWA smoke (`node bot-e2e-pwa-fabric.cjs`, viewport 390×844, entry
`/fabric-session.html` — **not** `/fabric-session`, which the main Vite
plugin rewrites to the platform SPA):

- `http://localhost:3013` is already a Vite from sibling worktree
  `allternit-session-botmode-clerk-0910`. Dashboard there has no Bots
  section (this phase's code is not on that process).
- A short-lived Vite on `:3020` from **this** worktree loaded
  `/fabric-session.html` at 390×844 but hit the unconfigured auth wall
  ("Authentication is unavailable in this build"), so the Bots section
  never mounts. Screenshot:
  `~/.agent-orchestrator/evidence/fabric-pwa-bot-mode-ui/pwa-fabric-390x844.png`.

Human command once this worktree has Clerk keys:

```
cd surfaces/ai.allternit.com && pnpm dev
PLATFORM_URL=http://localhost:3013 node bot-e2e-pwa-fabric.cjs
```

Requires `/tmp/botmode-e2e-state.json` from a prior `scripts/e2e/bot/bot-e2e-live.cjs` run.

Evidence: `~/.agent-orchestrator/evidence/fabric-pwa-bot-mode-ui/`
(`vitest-bot-chat-1c.txt`, `tsc-noemit-1c.txt`, `pwa-fabric-390x844.png`).

## Open questions

- None blocking 1C. Live smoke is a human Clerk-keys run, not a code gap.
- Streamdown bundle weight from 1A still stands if a later desktop audit
  flags it.
