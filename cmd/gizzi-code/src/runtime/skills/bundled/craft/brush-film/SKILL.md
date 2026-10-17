---
name: brush-film
description: >-
  Make a finished, hand-painted animated short (a cartoon, satire ad, fake commercial, explainer, music-video segment or
  story film) entirely in code: every set, character, prop and transition painted with p5.brush (watercolour fills and
  boiling ink lines on paper), rendered deterministically frame by frame in headless Chrome, with a storyboard of timed
  "reads", audio locked first (VO from any TTS the user has, a music bed, synthesized SFX), chapters built and
  self-checked from rendered contact sheets, and a script that assembles the MP4. Ships a ready starter character
  (Blot, a round ink-blob with emotions, mouths, arms and emotes) and a 7-second example that renders. Use it for ANY
  request to make an animated video, cartoon, animated short, animated ad or explainer, or a "hand-drawn / painted /
  watercolour" animation, even if the user doesn't name the method. Not for motion-graphics title reels, editing real
  footage, or AI text-to-video clips.
---

# brush-film

Everything is painted in code with [p5.brush](https://github.com/acamposuribe/p5.brush). No image plates, no generated
stills, no stock clips: one painted medium, so nothing ever looks pasted on. Frames are pure functions of time, so they
render in parallel, out of order, and resume after a crash. Cost is $0 beyond whatever TTS the user chooses; renders
take ~0.1-0.5 s per frame on a machine with a GPU.

**The bar for every film:**
- **One painted medium.** Sets, characters, props and transitions are all `paint()` / `inkLine()` / `glow()`.
- **A real story:** an event in every shot, a turn near the end, an ending that rhymes with the opening.
- **The picture follows the words.** Every spoken or sung line gets its own visual that illustrates it, literally or as a
  sight gag. A line with no matching picture is a bug. Plan it as a column in the storyboard.
- **Entertaining at every second:** match cuts, whips, shaped irises, push-ins, set-ups that pay off. Something visibly
  changes on every beat.
- **Locked audio:** VO with on-screen subtitles, a music bed, SFX on the animated hits.
- **Characters on model** in every shot, big enough to act (40-60% of frame height in a medium shot).

**Cost rule: never build a whole film in one context.** Read `${SKILL_DIR}/references/LEAN-AGENTS.md` first. Use short
fresh stages (write + audio, one per chapter, a fix pass), keep state in `HANDOFF.md` in the film folder, and finish
mechanically with `${SKILL_DIR}/scripts/finish_film.sh`. A single agent that builds a whole film runs past 300k tokens.

## Read first
1. `${SKILL_DIR}/template/anim/ANIMATION_GUIDE.md`: the rules (handmade, alive, one piece, timing by reads, transitions
   always), the animation principles, the review loop, and the full engine + Blot API. These rules are what make the
   quality. Follow them.
2. `${SKILL_DIR}/references/STORYBOARD-example.md`: the shape of a finished storyboard.
3. `${SKILL_DIR}/template/anim/src/scenes/example.js`: a complete two-shot scene (emotion timeline, hop, held prop,
   glow, brush wipe, iris in/out).

Requirements: Node.js 18+, Google Chrome or Chromium, ffmpeg, Python 3 with numpy + scipy (audio tools). Check with
`node -v && ffmpeg -version | head -1 && python3 -c "import numpy, scipy"`; tell the user what's missing rather than
installing system packages yourself.

## Workflow

**0. Concept (the user approves).** Write `CONCEPT.md` in the film folder: logline and turn, the cast, a shot table
with the VO/dialogue lines, format and length. Ask only questions that change the build (length, format, who's in it,
any real names or logos), then **show the outline before building**.
Content rules: never draw or voice a real private person; public figures and real brands only when the user asks and
it's clearly commentary or parody; claims you can't verify stay off screen; never clone a real person's voice without
their documented permission.

**1. Scaffold.**
```bash
bash ${SKILL_DIR}/scripts/new_film.sh <film dir> <film-name> <chapter names...>
# e.g. bash ${SKILL_DIR}/scripts/new_film.sh ./lost-sock lost-sock cold-open dryer kingdom ending
```
It copies `${SKILL_DIR}/template` into the film dir (engine, renderer, Blot rig, example props, audio tools,
`assemble.sh`, `shorts.sh`), creates one stub per chapter (`anim/src/ch/chN_<name>.js`) wired into `studio.html`,
removes the example scene, runs `npm install` in `anim/` (p5, p5.brush, puppeteer-core; no browser download) and
starts a local git repo. Commit after every milestone. To just try the kit, copy the template and render the example:
`cp -R ${SKILL_DIR}/template <dir> && cd <dir>/anim && npm install && node render.mjs --clip --out=out/video.mp4`.

**2. Audio first. Timing comes from the voice, not the other way round.**
1. Write `audio/vo-lines.json` as `[[id, voice, spoken text, {options}?], ...]` (see `audio/vo-lines.example.json`).
   The spoken text may be spelled phonetically for the TTS; the subtitle in `film.js` keeps the real spelling.
2. `python3 audio/make_vo.py --engine=<say|piper|espeak-ng|elevenlabs|cmd|manual>` records each line to
   `audio/vo/<id>.wav` and prints `DUR_VO`; paste it into `anim/src/film.js`. Engines: `say` (macOS voices, free),
   `piper` / `espeak-ng` (free, offline), `elevenlabs` (the user's `ELEVENLABS_API_KEY`; **metered**, tell the user it
   spends their characters before running it), `cmd` (any TTS via `TTS_CMD='tool --voice {voice} --in {text_file}
   --out {out}'`), or `manual` (the user drops recorded/generated files into `audio/vo/`). Ask which TTS they have
   before recording; default to a free local engine. Never drive a logged-in browser account to get audio.
3. Music: the user's own track as `audio/bed.mp3` (or `.wav`), or a free synthesized bed:
   `python3 audio/synth_bed.py --style=bouncy|calm|tense` (tempo from `config.js`). `python3 audio/beat.py audio/bed.mp3`
   estimates `bpm`/`offset` for `anim/src/config.js` from a real track.
4. Lay out `VO` cue starts in `film.js`, then `SHOT_T` around them (~0.15 s lead-in, ~0.35 s tail per line), then set
   `MUSIC.bedEnd` (0 = runs to the end) and `PROJECT.duration` in `config.js`.

**3. Storyboard with reads** in `anim/STORYBOARD.md`: logline, world and palette arc, motif, running gags, then one
table per chapter: shot, time, what happens (the event, camera, transition in and out), the line it illustrates, and
**reads with times**. List exactly which on-screen words are allowed (subtitles, any genre supers). Everything else
follows the guide's no-text rule. Check every shot before building: an event in every shot, every read has time to
land, one read at a time, a transition at every seam, the ending rhymes.

**4. Shared look before chapters.** Extend `src/props.js` (sets, recurring props, the film palette), restyle Blot or
add new rigs (see "Write your own character" in the guide), and check them on model sheets:
`node render.mjs --loop=emotions --sheet=1 --cols=1 --w=1600 --out=out/check/emotions.jpg` (also `--loop=parts`,
`--loop=props`). Open the images and look. Fix pigment mixing (see Gotchas) before anyone builds on it.

**5. Chapters.** For a film over ~30 s, split it into 4-5 chapters. If you can spawn subagents, run one per chapter
(2-4 at once) with `${SKILL_DIR}/references/CHAPTER-BRIEF.md` filled in, and build the emotional-turn chapter yourself.
Otherwise build them one at a time, writing HANDOFF.md between them. Each chapter:
- edits only its own file; shared files are read-only (report bugs instead of patching them mid-build);
- checks every shot with `--sheet`, `--strip` and `--crop` and **looks at the images**;
- reports its hit times (for SFX) and ms/frame.

**6. Integrate.** Look at each chapter's sheet. Fix seams (wipe colours must match across files) and shared-file issues.
Time `SFX` in `audio/build_mix.py` to the reported hits (the library: click slap stamp whoosh sparkle boink crash chaos
ring clunk chalk pour creak, all synthesized, free and re-timeable).

**7. Assemble and QA.** `bash ${SKILL_DIR}/scripts/finish_film.sh <film dir>` (or `./assemble.sh v1` directly). It
rebuilds the mix, renders frames in parallel (resumable), encodes, adds the optional `anim/assets/intro.mp4` /
`outro.mp4` if the user supplies bumpers, loudness-normalises, writes `exports/<name>-v1-16x9.mp4` + a preview, renders
any vertical cuts, and writes contact sheets to `reviews/`. To redo shots, delete just their frames
(`anim/out/frames/fNNNNN.jpg`) and re-run with `FORCE=1`; `KEEP_FRAMES=1` keeps frames while you're still fixing.
Then QA the export itself: open every cell of `reviews/contact-*.jpg`; if a speech-to-text tool is available, check the
VO transcript against the script. Fix the three worst defects, re-run, look again.

**8. Deliver.** Give the user the master and preview paths, the contact sheet, and what's left (anything you couldn't
verify). Never post or upload anything on the user's behalf.

## Vertical cuts (9:16)
The engine paints vertical natively: `studio.html?ow=1080&oh=1920&short=<name>` paints the 16:9 design space into a
1080x1920 canvas through `window.FILM_VIEW(t)`. List cuts in `anim/src/shorts.js` as `{ name, start, end, hook, view:
[[t, cx, cy, z], ...] }`: standalone beats, a short hook line (top band), and a view keyframed so the key action stays
centred (z 0.55-0.7 for wide staging, up to ~0.85 for close-ups). Check with `node render.mjs --sheet=... --qs="ow=1080&oh=1920&short=<name>"`,
then `./shorts.sh [name]` writes `exports/shorts/<film>-<name>.mp4`. Frame vertical cuts as their own composition, not
a crop: re-check every cell.

## Quality checklist (before calling it done)
- [ ] Every shot has an event; every read lands before the next starts; no dead stretch.
- [ ] Every VO line has a picture that illustrates it; subtitles are in sync.
- [ ] A transition at every seam, including the first frame and the last.
- [ ] Characters are big, on model, acting through `emotions()` (no snapping faces), with anticipation and follow-through.
- [ ] No text beyond the allowed list; no 3D projection; no plain p5 shapes; no muddy glows.
- [ ] Still things don't jitter (each element has its own `boilSeed`); props touch the hands that hold them.
- [ ] The ending rhymes with the opening.
- [ ] You opened and looked at the model sheets, every chapter's sheets, and the final contact sheet.

## Gotchas
- **p5 global name clashes.** p5 defines globals like `OVERLAY` after your scripts load, silently overwriting
  `window.OVERLAY`. Engine hooks are `window.FILM_OVERLAY` / `FILM_GRADE` / `FILM_VIEW`; keep that prefix. Never name a
  local variable `push`: it shadows p5's `push()`.
- **Pigment mixing.** p5.brush mixes like paint: yellow over blue turns green, light over dark turns grey. Use an
  opaque `wash` with a pre-mixed `mixCol(base, colour, k)`, or `glow()` for light.
- **`letter()` ignores push/translate/scale.** It places text through the camera only: give it world coordinates.
- **Last-shot length.** While later chapters are stubs, the last registered shot's `dur` runs to the end of the film.
  Compute shot lengths from `SHOT_T` differences.
- **Wipe colours must match across files:** pass the same `[c1, c2]` to `brushWipe` at the end of one chapter and the
  start of the next.
- **Fonts must be loaded by the page.** Display text uses "Luckiest Guy" / "Permanent Marker" from Google Fonts in
  `studio.html` (offline renders fall back to a system face); add other faces there and to `window.LOAD`.
- **Big decorations painted off-frame can bleed into the picture.** Draw them only once their parent is on screen.
- **Long renders:** run them in the background with a long timeout; frames resume, so just re-run after an interruption.
  Without a GPU, watercolour fills are slow (seconds per frame): add `--soft-gl` on Linux and prefer flat washes.

## Credits
The engine is based on [claude-animation-base](https://github.com/JohnHeibel/ClaudeAnimationBase) by John Heibel (MIT);
see `${SKILL_DIR}/template/anim/README.md` and `LICENSE` for what was retained and what was added (Blot, props, film
timeline, vertical output, audio tools, assembly).
