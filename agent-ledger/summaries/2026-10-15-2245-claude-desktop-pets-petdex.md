# Desktop pets (Petdex standard): audit, art pilot, and the remaining work

- **Date:** 2026-10-15 22:45 CDT
- **Agent:** claude (Opus 5.5), session 981ea92d
- **Commit:** none. No product code changed. Art, tools and previews are in `allternit-workspace/scratch/desktop-pets/`.

## Why

Eoj saw Hermes Agent's desktop pet: a Petdex sprite that roams the desktop and reacts to agent activity. He wants the same in Allternit, and wants it "really good": real drawn frames, not one picture moved around in code.

## What was found

- **The pet window already exists.** `allternit/surfaces/allternit-desktop/src/main/desktop-companion.ts` makes a transparent, always-on-top window and handles drag and click-through.
- **The pet never reacts to the agent.** `allternit-ai/src/pages/DesktopCompanionPage.tsx` renders `<BotAvatar>` with no state.
- **The state-to-row mapping is wrong.** `allternit-ai/src/lib/agents/avatar-activity.ts`: "done" plays the walk-left row and "error" plays the review row. It also loops all 8 columns on every row, so short rows show blank frames.
- **Every shipped avatar sheet is one portrait moved around in code** (`scripts/avatar-packs/sheetgen.py`), so nothing can really walk or turn.
- **The Gizzi Brawl game sheets are broken.** In the live game (`~/Projects/basement-temp/public/games/kombat/` → brawl.allternit.com; source `~/Desktop/gizzi-brawl/_pack-src/sprites/`):
  - ghost half-copies are baked between the frames
  - frames drift across the game's equal 8-way slices, so arms and weapons get clipped
  - Codex Super and Grok Super sit on solid magenta that was never keyed out
  - Husk Super is in fragments in the file itself

  Eoj says these problems were never fixed in the game.

## The standard (Petdex, MIT)

- **Grid:** 8 columns, 192×208 cells. The v2 layout has 11 rows.
- **Rows, with frame counts:**
  - idle (6)
  - running-right (8)
  - running-left (8)
  - waving (4)
  - jumping (5)
  - failed (8)
  - waiting (6)
  - running (6)
  - review (6)
  - Allternit's two extra rows: sleep (8) and special/celebrate (8)
- **Frame times:** `crafter-station/petdex` `packages/petdex-desktop-native/src/sprite.zig`.
- **Agent activity → state (from Hermes):**
  - tool running → running
  - model thinking → review
  - approval or question open → waiting
  - turn done → waving
  - plan done → jumping
  - error → failed
  - idle → idle, roaming while idle

## Done in this session (art only)

- **Gizzi YouTube cast, 12 characters, real frames at 0 image cost.** They are drawn from code rigs: `tools/rigsheet.cjs` for the show rigs and `tools/gizzisheet.cjs` for the brush-film Gizzi rig with a true side view. Sheets are in `rig-sheets/`. **Eoj approved these ("look good").** Gizzi, Dropsy and Crescent are clay in the avatar picker but flat 2D as rig pets.
- **Gemini test fighter: 10 ChatGPT images cut into a full v2 sheet** (`gemini/gemini-sheet-full.webp`). It took about 10 minutes, about 1 minute per image. Preview: `previews/gemini-test.html` (artifact 9SvRexjhNvdEbmBAydBhiV). Eoj asked about time and moved to hand this to another agent.
- **Muse is now Jolly.** Eoj's call: Meta Muse fighters must be Jolly, the tan fuzzy Muse mascot (reference `jolly-muse/jolly-reference-fortune.jpg`), NOT the blue M logo. Done: both portraits plus 8 of Muse's 10 strips. Missing: Muse's `waiting` and `special`, and all 10 of Muse Super's strips.

## Tools (`scratch/desktop-pets/tools/`)

- **`rows.py <char> <base.png>`:** runs the 10 state prompts through ChatGPT, one at a time.
- **`turn.py`:** the queue. It holds `/tmp/allternit-chatgpt-safari.lock` and waits while any other session's `safari_capture.py` is running.
- **`slice.py <dir> <char>`:** keys out white, splits each grid image, matches row sizes to idle, and writes `<char>-sheet-full.webp` (v2, 192×208).
- **`recut.py`:** a partial attempt to clean the old game sheets automatically. It works on some fighters and fails where ghost copies touch the real frames. Don't rely on it.

## Rules Eoj set during this session

- **Safari ChatGPT lane:** one capture at a time across all sessions. Wait your turn and never kill a capture mid-run, because the window only closes in its `finally` block. Eoj called parallel captures "reckless". Memory: `chatgpt-image-safari-lane`.
- **No surprise image batches.** Do one character, show it, then continue.

## Outstanding work (the next agent picks this up)

1. **Brawl fighters as pets.**
   - **Roster:** 31 fighters remain, the 16 vendor fighters and their 15 Supers: clawd, codex, deepseek, glm, grok (no Super), hermes, husk, kimi, nemotron, ollama, omp, openclaw, opencode, qwen and gizzi-brawl Gizzi, plus Muse Super.
   - **Inputs:** run `rows.py` on each clean picker portrait (`allternit-ai/public/avatar-packs/gizzi-brawl/<id>.webp` on white), then `slice.py`.
   - **Size:** about 10 images and 10 minutes per fighter, about 310 images and 5–6 hours in total.
   - **Process:** show Eoj each fighter, or small groups, before continuing.
2. **Game fight poses (Eoj wants the new art to improve the game too).**
   - The game needs attack, block, hit reaction and KO-on-floor, which the pet set doesn't include. That's about 3 more strips per fighter.
   - The game currently reads one 8-frame row: idle, walk×3, attack, hit, block, KO (`configs/characters/base.js` `BASE_FRAMES`). The animation controller has to be updated for longer walks, jump, run and victory.
   - Deploying the game needs Eoj's go-ahead.
3. **Product build. Eoj approved steps 1–5, none started.**
   - Move the Petdex v2 layout and timings into `avatar-activity.ts` and `AnimatedBotSprite.tsx`.
   - Relay agent activity from the main window to the pet window over a new IPC next to `companion:state`. Sources: `useThreadGatewayEvents`, approvals.
   - Hermes behaviours: roam when idle, a "finished while away" badge, speech bubbles (reuse gizzi-code `pet/observer.ts` quips), double-click to open the app.
   - A local hook port. Eoj: "use another port but don't have it competing with something we already do". Pick one no Allternit service uses and feed the same pet state, so there's still one pet.
   - Same states in-app on ai.allternit.com and the m.allternit.com PWA.
   - Docs in the same PR.
   - Character builder: make the `walk` and `run` rows a true side view. `scene.ts` uses yaw 0.5 today.
4. **Everyone else (about 178 avatars).** Eoj's direction: "then we can do all of the rest". Options are on artifact XoRkheRgWMejCNxnfrVtvV: A hop, B fewer images, C puppet, D generate on demand, E rig. Confirm the approach with Eoj before generating at scale.
5. **Old sheets.** The 113 moving-portrait sheets and the game sheets should be replaced by the new art, not patched.

## Previews (claude.ai artifacts, owned by Eoj)

- Full cast in the Petdex layout: XCugR7TLfzucWxXVWH7DyD
- Options A–E: XoRkheRgWMejCNxnfrVtvV
- Gizzi YouTube pets: 8fbtzn76bV1QEgPUcnPRJf
- Brawl from the old game frames, showing the problems: Y1deVBT4yUGZsShwWjvWqT
- Gemini test: 9SvRexjhNvdEbmBAydBhiV
