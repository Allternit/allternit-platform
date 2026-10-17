---
name: ui-motion
description: >-
  Design and build product-UI motion that feels right: transitions, enter/exit, panels, sheets,
  menus, toasts, list reorder, shared-element moves, hover/press feedback, loading-to-content,
  gestures. Covers timing and easing numbers, springs, choreography, what moves and what doesn't,
  reduced motion, performance, and a frame-strip tool (scripts/strip.mjs) that captures a
  transition frame by frame so you can SEE it before calling it done. Use for ANY React/CSS/
  SwiftUI animation or "make this feel smoother / snappier / more polished" request, any
  framer-motion / motion / CSS transition / keyframes / View Transitions / withAnimation work.
  Not for videos (motion-reel, film-studio) or perf-only fixes.
---

# ui-motion

Motion in a product exists to explain change: where something came from, where it went, what
caused it. If an animation doesn't answer one of those, cut it. The rest of this file is the
numbers and the review loop that make the remaining motion feel deliberate.

## The loop (do all four, every time)

1. **Write the beat sheet first**, in a comment or the PR: trigger → what moves → from where →
   to where → how long → easing → what happens on interrupt. One line per element.
2. **Build it** with the numbers below, in the project's existing motion tokens/library.
   Read the design system first (`grep -r "cubic-bezier\|transition\|spring\|duration" src`);
   reuse its tokens instead of inventing new ones. If it has none, add a small token set.
3. **Look at it frame by frame** with `scripts/strip.mjs` (below) and read `sheet.png`.
   You cannot judge motion from code. Check: does frame 1 already show movement (no dead
   lag)? Is the settle visible but short? Do things that should move together actually move
   together? Any flash, jump, layout shift, or element popping in at the end?
4. **Check the edges**: interrupt it mid-flight (click twice), reduced motion (`--reduced`),
   dark mode (`--dark`), phone width (`--w 390 --h 844`), slow data (content arrives late).

## Numbers

| What | Duration | Easing |
|---|---|---|
| Press / hover feedback | 80–120 ms | ease-out |
| Small element enter (tooltip, menu, toast) | 150–220 ms | ease-out `cubic-bezier(.2,.8,.2,1)` |
| Exit (anything) | 60–75% of its enter | ease-in `cubic-bezier(.4,0,1,1)` or plain fade |
| Panel / sheet / dialog | 240–340 ms | strong ease-out `cubic-bezier(.16,1,.3,1)` or spring |
| Page / route / large shared-element | 300–450 ms | spring or `cubic-bezier(.65,0,.35,1)` for on-screen A→B |
| Stagger between list items | 20–40 ms each, cap total at ~200 ms | same as the item |

- **Ease-out for things arriving, ease-in for things leaving, ease-in-out only for things that
  move from one on-screen spot to another.** Linear only for spinners, progress and loops.
- **Springs** (motion/framer-motion, SwiftUI): UI default `stiffness 400–550, damping 32–40,
  mass 1` (≈ no visible overshoot). Playful/bouncy: damping 18–24. SwiftUI: `.snappy`,
  `.smooth`, or `.spring(response: 0.35, dampingFraction: 0.85)`. Use springs whenever the
  motion can be interrupted or follows a gesture — they keep velocity; tweens restart.
- **Distance is small.** Enter offsets 4–16 px, scale from 0.96–0.98, never from 0. Big
  slides read as slow even when they're fast.
- Bigger/farther = slightly longer; never longer than ~450 ms for anything the user waits on.

## Choreography

- **One hero per transition.** The thing the user acted on leads; everything else follows
  20–60 ms later or just fades.
- **Animate from the cause.** A menu grows from its button (`transform-origin` at the
  trigger), a toast enters from where toasts live, a deleted row collapses in place.
- **Continuity beats decoration.** If an element exists before and after, morph it (layout
  animation / `layoutId` / View Transitions `view-transition-name` / SwiftUI
  `matchedGeometryEffect`) instead of fading one out and another in.
- **Exits are quiet.** Shorter, less travel, usually opacity + 2–4 px.
- **Don't animate what the user is reading or typing into.** No motion on text that's
  updating while focused, no layout shift under the cursor.
- **Loading → content**: hold skeletons/orbs ≥ 300 ms or don't show them at all (avoid a
  50 ms flash); crossfade content in 150–200 ms; reserve the final size to avoid shift.

## Implementation rules

- Animate **only `transform` and `opacity`** (and `filter`/`clip-path` sparingly). Never
  width/height/top/left/margin — use layout animations (FLIP) for size and position changes.
- Exit animations need the element to stay mounted: `AnimatePresence`, a `data-state`
  attribute + `animationend`, or View Transitions. A conditional `&&` alone pops.
- Every animation must be **interruptible**: re-triggering mid-flight continues from the
  current value, never snaps to the start. Test it with two quick actions in strip.mjs.
- **Reduced motion**: keep opacity/color changes, drop travel and scale, cut springs to
  instant or 100 ms fades. `@media (prefers-reduced-motion: reduce)`, `useReducedMotion()`,
  SwiftUI `@Environment(\.accessibilityReduceMotion)`. Never remove the state change itself.
- `will-change` only during the animation; no infinite animations on things off-screen.
- Keep the numbers in tokens (`--ease-out`, `--dur-panel`, a `springs.ts`) so the app
  has one motion language.

## scripts/strip.mjs: see the motion

```bash
node ${SKILL_DIR}/scripts/strip.mjs \
  --url http://localhost:5173/settings --act "click:[data-testid=open-panel]" \
  --ms 500 --step 33 --clip "[role=dialog]" --out strip-panel
# then open strip-panel/sheet.png (read the image) — every frame is stamped with its ms
```

- Freezes time (Playwright clock fakes rAF/Date/performance, so JS springs step
  deterministically) and seeks every CSS/WAAPI animation to the same elapsed time, so the
  strip is identical on every run. Uses the installed Chrome/Chromium (macOS app path,
  `google-chrome`, `chromium`, `chromium-browser`; `CHROME=/path` to override). No browser is
  downloaded. On first run it installs `playwright-core@1` next to the script (needs npm); if
  that's not possible it prints the one-line install command. `--url` also takes a local file
  path (`--url ./demo.html`).
- `--act` is `;`-separated: `click:`, `hover:`, `focus:`, `press:Key`, `type:sel=text`,
  `wait:ms`, `eval:js`. These run before capture starts. Prefix one with `@<ms>:` to fire it
  during capture instead. That is the interrupt test:
  `--act "click:#open;@100:press:Escape;@170:click:#open"` shows whether the close
  reverses from mid-flight and the reopen continues without a snap.
- Flags: `--reduced`, `--dark`, `--w/--h` (phone: 390×844), `--clip <selector>`.
- Known limit: animations run entirely on the compositor by a native app (Electron native
  views, iOS) aren't captured. For SwiftUI use Xcode previews / simulator screen recording.
- Compare against a reference: capture the reference the same way (same `--step`) and put
  both sheets side by side. Matching a known-good strip beats arguing about feel.

## References for iOS / SwiftUI

If an iOS motion-reference MCP or library is connected (real interaction breakdowns with SwiftUI
code), use it to compare against known-good examples; it isn't required. The principles above
apply the same way in SwiftUI.
