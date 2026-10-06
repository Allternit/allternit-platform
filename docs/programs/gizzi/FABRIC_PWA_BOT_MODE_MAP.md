# FABRIC PWA BOT MODE — MAP (orchestrator analysis, 2026-09-10)

Spec: `fabric-pwa-bot-mode-ui` (rq-20260910-007, approved 2026-09-10). Source reference:
[milind-soni/OpenMausBot](https://github.com/milind-soni/OpenMausBot) (Apache-2.0, `enterprise/`
excluded). This doc is the full gap map + architecture decisions the phases execute against.

## Goal (Phase 1)

`src/components/bot-chat/` = one surface-agnostic bot-chat component set (streaming transcript,
folded tool runs, approval cards, one-handed composer), consumed by BOTH the Fabric Session PWA
(new Bots section) and the web bot session view (`src/views/bots/BotChatSessionView.tsx`), so the
two cannot drift. Plus: SSE discipline, on-demand ACI screenshots, touch/safe-area chrome.

## Verified repo facts (executors: trust these, re-verify line numbers if files moved)

- **Styling**: Tailwind inline + `cn()` from `@/lib/utils` + theme tokens `var(--bg-elevated)`,
  `var(--text-tertiary)`, `var(--accent-primary)`. NO css-modules/styled-components.
- **Icons**: `@phosphor-icons/react` (e.g. `PaperPlaneRight`, `CircleNotch`, `Robot`).
- **Exports**: named exports only; consumers import file paths directly (no dispatch barrel).
- **Tests**: vitest ^1.6.1, jsdom, colocated `*.test.ts(x)`; `@` → `./src`.
- **Bot chat transport**: NOT raw EventSource. `src/views/chat/ChatSessionStore.ts` →
  `src/lib/agents/mode-session-store.ts:826` calls `chatApi.streamChat(session.id, text, modelId,
  { onChunk, onThinkingChunk, onToolCall, onToolResult, … })`; streaming state in
  `streamingBySession` map; abort via `abortGeneration`. `BotChatSessionView.tsx:407-412` today
  renders only a "{bot} is thinking…" spinner — no per-token display.
- **Approvals**: `cowork.approval_request` / `cowork.approval_result` events exist
  (`src/views/cowork/cowork.types.ts:175-198`) with `riskLevel`, `consequence`, `timeout`,
  `responder` — **no option sets, no grant keys**. Harness permissions:
  `src/views/chat/components/ChatApprovalCard.tsx` + `src/lib/agents/permission-store`.
  No `grantKey` field exists anywhere in `src/`.
- **Roster**: `src/lib/bots/use-unified-roster.ts` → `UnifiedRosterBot` (`id`, `displayName`,
  `handle`, `tagline`, `accentColor?` native-only, `status`, `source`, `agent`). Avatar component:
  `src/views/bots/BotAvatar.tsx`.
- **Routines**: client-side zustand persist store `src/lib/bots/bot-routine.service.ts`;
  `BotRoutine` (`id`, `title`, `instruction`, `frequency`, `enabled`, …);
  `getRoutinesForBot(botId)` (:432); `routinePrompt()` (:119) builds `[bot:<name>] <title>\n\n<instruction>`.
- **ACI frames**: `fabricClient.startAci` → `POST /api/aci/run`; `streamAci(runId)` → SSE
  `GET /api/aci/stream/:runId` (fetch-based iterator in SDK `proxySse`).
  `FabricSessionPanel.tsx:200-222` loops the stream into `setAciScreenshot(...)` whenever
  `driveKind === 'aci'` (always-on push). `FabricAciDrive` (FabricSessionDriveViews.tsx:140-160)
  is presentational (`screenshot` prop + `events`).
- **PWA structure**: NO router. `src/fabric-session/App.tsx` renders `DashboardPage` directly.
  Adding pages = lightweight view-state switch, do NOT add react-router.
- **Desktop lock**: repo AGENTS.md desktop-v1.1.1 rules — `surfaces/allternit-desktop` bundles
  `surfaces/ai.allternit.com` (`prepare-platform-static.cjs`) including the fabric-session entry,
  so changes under `src/components/` and `src/fabric-session/` reach the locked bundle.
  Before merge: `node scripts/release-preflight.mjs` must pass.

## Architecture decisions (binding)

1. **State discipline**: transcript state changes ONLY inside an event fold. Web chat today
   mutates via stream callbacks — the shared components receive a normalized
   `BotChatTranscript` (pure data + fold reducer in `src/components/bot-chat/transcript.ts`);
   adapters map `chatApi.streamChat` callbacks (web) and fabric relay events (PWA) into fold
   actions. Components never call fetch/SSE directly.
2. **Rungs**: working chamber (last ~2000 chars of thinking, collapses when answer tokens start)
   → typing dots (only before first content) → live bubble with caret at token frontier, atomic
   swap to settled. Scroll-follow keyed on char count, unanimated. `prefers-reduced-motion`
   disables dot/caret animation.
3. **Tool runs**: pure fold logic — 2+ consecutive tool calls fold to a run capsule
   ("Running N steps ➜" / "Ran N steps ✓", inline expand); failures NEVER fold.
   Extension point on the row model for policy verdict chips (openbot-policy-gateway Phase 1) —
   this map does NOT implement policy UI.
4. **Approvals**: normalized `ApprovalRequest` model. UI: per-option capsules when the event
   carries `options[]`; approve/deny binary otherwise (current server shape). Refusal = neutral
   tint, approval = bot accent tint. "Always allow this tool" renders ONLY when the event
   carries a server-issued `grantKey` — the client never invents keys (none exist server-side
   today → link simply doesn't render; forward-compatible).
5. **Composer**: sibling-of-scroll, 1–5 line growth, Return sends. Predictive chips from
   `getRoutinesForBot` (tap → `routinePrompt()` submit). `/` opens routines HUD. `+` rotates to
   ×, opens bottom-anchored action sheet (attach / new thread / watch computer / share /
   Interrupt-red-while-busy). Dictation via Web Speech API with replace-against-frozen-draft
   partials; graceful no-mic fallback (iOS Safari partial support is acceptable).
6. **SSE discipline**: reconnect cursor `<streamId>:<seq>` where a raw SSE source is used
   (fabric relay); for `chatApi.streamChat` (fetch-chunk transport) abort + resumable retry is
   the equivalent. `visibilitychange` hidden → abort streams; `shared-context` liveness via
   existing push subscription. No transcript mutation outside the fold.
7. **ACI pull**: `FabricAciDrive` never starts the frame stream at mount. The "watch computer"
   action (composer sheet + drive chrome) starts `streamAci` on toggle-on and aborts on
   toggle-off/hidden. Default state = off (no pushed pixels).
8. **Touch/chrome**: 44px min targets, `env(safe-area-inset-*)` on header/composer/pill,
   `touch-action: pan-y` on transcript scroll, `navigator.vibrate` only on send/approval.
9. **PWA navigation**: `App.tsx` gains a minimal two-view switch (`dashboard | chat`) keyed by
   selected bot id — no router, matches existing idiom.
10. **Sync**: `BotChatSessionView.tsx` adopts the shared transcript/composer in the same PR
    (Phase 1C), replacing its spinner-only streaming display.

## Sub-phases (each own task file + review)

- **1A — Foundation + transcript primitives**: `src/components/bot-chat/` types, transcript fold
  reducer (runs folding, gap timestamps, rungs), SSE cursor client, streaming bubble, typing
  dots, working chamber, receipt chip + run capsule. Tests: fold + run-folding + cursor reconnect.
- **1B — Approvals + composer**: approval card + approval pill + roster pill, composer
  (growth/chips/HUD/action sheet/dictation), haptics.
- **1C — PWA integration + ACI pull + web adoption + chrome**: App view switch, Bots section +
  chat page (roster → chat), ACI watch-toggle pull mode, touch/safe-area pass,
  `BotChatSessionView.tsx` adoption, 390×844 standalone smoke (extend `scripts/e2e/bot/bot-e2e-pwa.cjs`),
  docs, release-preflight.

## Risks / watch-items

- `mode-session-store` streaming callbacks are the de-facto event grammar — the adapter must map
  them losslessly (thinking chunks, tool calls/results, abort). Read the actual hook source.
- Two transports (chatApi chunked-fetch vs fabric relay SSE) — keep the cursor client thin and
  test the fold, not the wire.
- Desktop bundle: any new heavy dep in shared components lands in the desktop static build —
  prefer zero new runtime deps in Phase 1 (Web Speech API + phosphor icons already in tree).
- `openbot-policy-gateway` may merge verdict chips concurrently — row model has the extension
  slot; do not implement policy.
