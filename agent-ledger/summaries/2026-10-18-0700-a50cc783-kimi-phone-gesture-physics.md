# Attestation — session/a50cc783 — phone gesture physics (kimi-code)

Date: 2026-10-18
Branch: `session/a50cc783` (worktrees: `allternit-ai-session-a50cc783`, `allternit-session-a50cc783`)
Code PR: Allternit/allternit-ai #635 (merged, merge commit on origin/main)
Docs companion PR: this PR (platform repo)

## What was done

Implemented iOS-style gesture physics for the Allternit phone UI (the shell
shared by the m.allternit.com PWA and the ai.allternit.com phone layout),
triggered by the shedsgns/Astra six-gesture exploration (x.com/shedsgns,
2026-10-18) plus four approved extras. Principle: drag tracks the finger 1:1
through a motion value; release velocity feeds a design-token spring
(`src/design/animation/timing.ts` presets) so motion settles instead of
stopping dead.

- Gesture core: `gestures/pan.ts` (pointer pan, dominant-axis lock, pure
  `projectDetent`/`rubberBand`/`sampleVelocity`), `gestures/springs.ts`
  (`springTo`, `SpringControls`), `useSwipeActions`, `useSheetGestures`,
  `useDragReorder`, `useLongPressPeek`, `usePressable`.
- Navigation: `phone-nav.ts` gained a capped (10) back stack of
  `PhoneNavSnapshot` with push-in-actions and `back()`; `ScreenStack.tsx`
  renders push/pop via two persistent slots + frozen snapshot layers through
  `PhoneNavScopeContext` (screens migrated to `usePhoneNavScoped`); left-edge
  bezel swipe = interactive back when depth > 0, drawer open at depth 0,
  drawer follows the finger both ways.
- Gestures: recents swipe-to-delete (two-step confirm) + long-press
  drag-to-reorder (persisted per-device in `phone-ui-store`); bottom-sheet
  detents + velocity fling + drag-down dismiss on six sheets; chat
  pull-to-refresh (chat canvas), long-press message peek (Copy/feedback/Retry),
  swipe-right quote-reply via the existing `lib/chat/reply.ts` pipeline;
  toast swipe-dismiss; setup-carousel fling paging; touch press scale.

## How it is safe

- Desktop behavior byte-identical: gating on `PhoneSurfaceContext`, an opt-in
  `onPullToRefresh` prop, or coarse pointer (toasts). Verified by reading
  every gate-closed branch.
- `prefers-reduced-motion` disables all tracking/transitions; tap/click paths
  unchanged; state changes preserved.
- Only transform/opacity animated; `will-change` scoped to active gestures.

## Verification evidence

- 170 phone vitest tests green (29 files), including synthetic-PointerEvent
  physics tests for every gesture and new `ScreenStack.test.tsx` (transition
  layers + snapshot freeze). `pnpm exec tsc --noEmit -p
  tsconfig.typecheck.json` clean. `vite build` and `vite build --config
  vite.fabric-session.config.ts` both green. Browser boot smoke on
  127.0.0.1:3013 (signed-out landing renders, no console crash).
- Dependency-map: `scripts/dependency-map.py --impact` before the change —
  "Phone layout and PWA" [live], journey "Start a Factory team from your
  phone", all three shipped surfaces, 294 components via shared code.
- CI on #635: check-sw-cache-bump pass.

## Honest deferrals / incidents

1. **Visual motion verification deferred.** The ui-motion strip.mjs frame
   capture needs the phone chrome, which local dev cannot show — the dev
   shell has no auth provider, so the signed-out landing renders without
   drawer/header. Verify the feel on staging or a device: edge-swipe-back,
   sheet detents, PTR, drawer drag.
2. **m.allternit.com PWA not deployed.** Merging deploys ai.allternit.com
   only; the PWA needs the manual wrangler step from the deploy runbook.
   Held for Eoj's go-ahead.
3. **Desktop preview not rebuilt.** Lifecycle step 8 (rebuild the desktop
   binary from merged main) was not run — held for Eoj's decision, since a
   desktop rebuild also triggers the one-current-build rules.
4. **Cowork canvas PTR not wired** — `CoworkRoot` owns its own scroller; the
   refresh reloads both session stores anyway. Documented in
   `allternit-ai/docs/phone-gestures.md`.
5. **Sheet X/scrim closes stay instant** — only drag-dismiss animates out.
6. **Shared checkout `allternit-ai` could not fast-forward** after merge
   (pre-existing local modifications abort the pull; not from this session).
   Canonical state is origin/main (74e0552e).
7. Workspace disk hygiene: `~/Desktop/allternit-workspace` is 206 GB
   (>150 GB threshold) though 171 GB remain free — cleanup is its own task.
8. `gizzi workspace node close` returned usage errors all session (claim/add
   worked); DAG dag_697144 node states were tracked in
   `.steering/checkpoint.md` instead.

## Merge SHA

Recorded at merge time by the orchestrating session (origin/main of
allternit-ai contains PR #635).
