---
name: motion-reel
description: >-
  Make a finished motion-graphics video (MP4 with an original synthesized soundtrack) from code:
  brand showreels, sizzle reels, logo reveals, product teasers, launch videos, animated title
  cards, kinetic typography, social promos. Deterministic canvas animation rendered frame-by-frame
  in headless Chrome, 120fps → motion-blurred 60fps via ffmpeg, beat-synced score from numpy.
  Use this whenever someone asks for a "motion graphics video", "showreel", "sizzle reel",
  "promo video", "animated intro", "logo animation", "hype video", "reel for my resume/portfolio",
  or any short designed video built from brand assets and text, even if they don't say
  "motion graphics". Not for editing existing footage or for AI text-to-video generation.
---

> Pair this with the `film-studio` skill (the production contract): brief.md (facts vs choices), a style guide from an inspected reference, a shot list of states and beats, sound on the same timeline, a scored contact-sheet review that fixes the three worst defects, separate compositions per format, and gates. This skill is the method; film-studio is the review and packaging layer.

# motion-reel

Produce a polished short motion-graphics video entirely from code. The approach: every frame is a
pure function `draw(t)` on an HTML canvas, so any moment can be rendered, inspected, and fixed in
isolation. Chrome renders the frames, ffmpeg blends them into motion blur, and a numpy synth writes
a score whose hits land exactly on the cuts. No timeline editor, no stock music, no licensing.

Starter example: `${SKILL_DIR}/templates/scenes.js` is an 8 s, 4-scene reel for a fictional
placeholder brand ("Northwind"): logo assembles from blocks, kinetic type slam, a product UI panel
that fills the frame, lockup. It shows the density and hand-offs to aim for. Every word, colour and
shape in it is a placeholder; replace all of it with the user's real brand.

## Why this works (keep these in mind; they're the whole trick)

1. **Content comes from real brand material, not invention.** A reel feels "correct" when every
   line, color, and shape is pulled from the brand's own docs and asset files. Generic motion
   design with the wrong words looks like a template. Research is the first half of the job.
2. **Time is the only input.** Never use `Math.random()`, `Date`, or accumulated state in draw code
   — use `rr(i,k)` (deterministic pseudo-random) and `P(t,a,b)` progress windows. That's what makes
   parallel rendering and still-frame review possible.
3. **Look before you render.** Rendering 15 seconds is cheap (~1 min), but *finding* a bug by watching
   is slow. Render stills at chosen timestamps, tile them into a contact sheet, and actually look at
   the image. This routinely catches a glitch that never switches off or a logo that renders
   black, both invisible in the code.
4. **Everything hangs on a beat grid.** Pick a BPM (120 → 0.5s beats). Scene cuts, slams, and
   flashes land on beats; the score is written against the same cue list. Sync is what makes it feel
   expensive.

## Workflow

### 1. Brand intake (don't skip — this is where quality comes from)
Find and read, in the user's workspace (or ask the user to point you at it):
- **Identity/positioning** — who the company is, what it sells, the standing one-line pitch.
- **Voice** — phrases they actually use, words they avoid. Pull real lines verbatim for on-screen copy.
  Watch for claims rules (e.g. no guarantees, no hype adjectives) and keep on-screen text compliant.
- **Design tokens** — exact hex colors, which color is the accent and where it's allowed, fonts.
- **Logo/mascot files** — SVGs especially. If the logo is built from geometric pieces (blocks, dots,
  bars), extract their coordinates so the logo can *assemble on screen* from its real parts:
  `python3 ${SKILL_DIR}/scripts/inline_assets.py <proj> blocks_from_svg=logo:<file.svg>` handles rect-grid logos.
  Images go in as data URIs: `inline_assets.py <proj> mascot=<file.svg>` → `IMG.mascot` in draw code.
- **Product facts** — surfaces, product names, divisions, feature names — for the "what we do" scenes.

If there's no brand material, **stop and ask** the user for: name, one-liner, 3–5 things they do,
colors, fonts, and a logo file. A reel with invented content isn't worth rendering. If they only
want a demo, use the Northwind placeholder and say plainly that it is a placeholder.

### 2. Storyboard on the beat grid
Write a short scene table first (time range → what's on screen → the hit that ends it). Default arc
for a 15s reel at 120 BPM — adapt, don't copy:

| Time | Beat role | Typical content |
|---|---|---|
| 0–2 | Build-up (riser) | Logo/mark assembles from particles; one label types in |
| 2.0 | **Drop** | Biggest hit: flash + shake + impact |
| 2–4 | Statement | Kinetic type of the positioning line, word slams on beats |
| 4–7 | Proof, fast | 3 capabilities × 1s each, each with its own mini-visual |
| 7–9.5 | Product | Floating UI windows with live-looking activity |
| 9.5–11.5 | Depth | A system/family/grid moment (tiles, network, map) |
| 11.5–13 | Breakdown | One quiet line of the core promise; music drops out, riser in |
| 13–15 | Lockup | Full logo re-assembles, tagline, URL, hold, fade |

`${SKILL_DIR}/references/design-playbook.md` has the scene recipes, transition catalog, typography rules, and
pacing numbers. Read it when designing scenes.

### 3. Scaffold
```bash
bash ${SKILL_DIR}/scripts/new_project.sh <project_dir>
```
Creates `index.html` (font setup), `scenes.js` (the Northwind starter to replace), and copies
`engine.js`, `render.mjs`, `find_chrome.mjs`, `score_lib.py`, `score.py` and an empty `assets.js`
into the project. It installs `playwright-core@1` into the project's `node_modules` on first run and
checks for ffmpeg, node, numpy+scipy and an installed Chrome/Chromium (macOS app path,
`google-chrome`, `chromium`, `chromium-browser`; set `CHROME=/path/to/chrome` to override). No
browser download happens: it drives the Chrome the user already has.
Point the `@font-face` rules in `index.html` at the brand's fonts (copy them into the project); the
defaults try the macOS system fonts, then common local fonts, then `system-ui` / `monospace`.
Check the sheet for fallback fonts on Linux.

### 4. Write scenes.js
`window.REEL = { w, h, duration, bpm, brand, hud, fonts, hits, flashes, glitches, wipes, scenes }`.
Each scene is `{start, end, name, draw(t, lt)}` where `t` is global time and `lt` is local time.
The engine handles shake (`hits`), flashes, slice glitches, block wipes, grain, vignette, HUD, and
fades from those arrays, so scenes only draw content.

Engine helpers (all in `engine.js`, read its header when needed):
- Timing: `P(t,a,b)` clamped progress · `win(t,a,b)` boolean window · easings `E.outExpo / ioExpo /
  outBack / outCubic / inExpo…` · `rr(i,k)` deterministic random.
- Type: `txt`, `tw` (measure), `rise` (masked rise-in), `riseWords` (word stagger), `typed`
  (typewriter + cursor), `slam` (scale-slam with motion-blur ghosts).
- Shapes/FX: `blk` (rounded block, glow), `ring` (shockwave), `grid` (drawn-on grid), `swarm`
  (converging streaks), `assembleBlocks` (logo from grid coords, with specular sweep),
  `camera(scale, fn)` (push-in), `panel`, `windowChrome`, `barWipe`.
- Colors: `mix(hexA, hexB, t)` returns `rgb()`, so **never nest `mix(mix(...))`**. Blend the amounts
  instead (`mix(a, '#fff', land + spec)`). Nesting silently produces black.
- Anything that must switch *off* after a moment (glitch, flash, overlay) needs `win()`, not just
  `P()`: `P` stays at 1 forever after its window. Glitches declared in `REEL.glitches` are already
  windowed.

Live preview: open `index.html` in Chrome; it loops in real time.

### 5. Review stills (the step that makes it good)
```bash
cd <project_dir>
node render.mjs stills 0.3,1.0,1.9,2.2,3.0,3.9,4.5,5.5,6.6,7.5,8.5,9.8   # pick moments mid-motion AND at rest
bash ${SKILL_DIR}/scripts/sheet.sh review sheet.png 3 4
```
Then **read `sheet.png`** and critique it like an art director: empty regions, text collisions,
unreadable contrast, things still visible that should be gone, scenes that look the same as each other,
anything off-brand. Fix, re-render stills, repeat until every tile looks intentional. Do at least two
passes; the first pass always finds something. `render.mjs` exits non-zero on page errors, so treat
that as a failure, not a warning.

**Art-direction checks agents miss** (runs often get the process right and still go flat after
the opening). Check every tile for these, not just for bugs:
- **Fill the frame.** A small window centered in black is a template, not a shot. Push the
  camera in (`camera()`), crop panels off the edge, or set big type next to the UI. Aim for
  the main subject covering at least half the frame.
- **Readable at speed.** On-screen text needs at least ~28 px at 1080p and enough hold time
  to read: about 0.25 s per word after it lands. A 1.5 s scene can't fit three typed
  commands. Type one, and show the rest already done.
- **Every scene needs its own move.** If scenes 3–6 are all "window fades in, text types",
  the middle sags. Vary it: slam, push-in, split, grid fill, morph from the previous scene's
  shape. The opening slam can't carry 10 seconds.
- **Ghosting on headers.** `slam()` motion-blur ghosts read as doubled letters on small or
  spaced-out mono text. Use `rise`/`typed` for small labels.

### 6. Score
Edit `score.py` (it imports the project's copy of `score_lib.py`: `impact, kick, clap, hat, blip, tick, riser,
whoosh, glitch, pad, saw_note, groove(), typing(), write_mix()`). Write cues from the same timestamps
as `REEL.hits` / cuts / typed reveals: riser into the drop, impact on every cut, groove between,
blips on block landings, drop the drums for the breakdown, chord + impact on the lockup.
`python3 score.py` → `score.wav`.

### 7. Render + encode
```bash
node render.mjs full --fps 120                                   # ~1 min for 15s on 6 workers
python3 score.py
bash ${SKILL_DIR}/scripts/encode.sh <name>.mp4   # 120→60fps motion blur, AAC, waveform.png
bash ${SKILL_DIR}/scripts/sheet.sh <name>.mp4 final_sheet.png 4 4
```
Read `final_sheet.png` and `waveform.png`. The waveform should show a clear build → drop → steady
groove → breakdown → final hit. If the middle groove is buried under the big hits (a common result),
lower impact sizes, raise kick/bass, rerun `score.py`, and swap audio without re-rendering:
`bash ${SKILL_DIR}/scripts/remux_audio.sh <name>.mp4 score.wav <name>-v2.mp4`.

### 8. Deliver
Save the MP4 plus a `source/` folder (scenes.js, index.html, engine.js, assets.js, score.py,
score_lib.py, score.wav) where the user's marketing/production files live, not only in a temp dir. In the reply:
- a scene-by-scene table of what's in the video,
- anything illustrative that could be mistaken for real (mock UI, invented labels/symbols),
- how to re-cut (edit scenes.js → stills → full render, about a minute),
- offers of variants: 9:16 vertical (set `w:1080, h:1920` and re-layout), 30s cut, licensed track.

## Bookends and series
If the user has an approved intro, outro, logo animation or HUD for their brand, reuse it as a
fixed constant and design only the middle. Don't redesign approved brand elements per video.
The engine's built-in HUD (corner brackets, label, timecode, scene index, thin progress line) is
optional: `hud: { show: false }` turns it off, `hud.progress: false` drops the progress line.

## Guardrails
- Nothing on screen should claim a result the brand can't back (metrics, guarantees, customer
  logos not approved for use). Mock UI is fine; label it as illustrative when you deliver.
- Never draw a convincing fake of a real product's UI or a real company's logo. Use the user's
  supplied assets or a clearly labelled placeholder.
- Don't put real people's likeness or voice in the video without documented permission.
- The score is synthesized from scratch. Don't pull in commercial tracks unless the user supplies
  a licensed file.
