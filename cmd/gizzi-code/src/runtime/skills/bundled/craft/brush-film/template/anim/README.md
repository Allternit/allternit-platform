# Brush film: the animation project

A hand-painted cartoon, made entirely in code with [p5.js](https://p5js.org) and
[p5.brush](https://github.com/acamposuribe/p5.brush): watercolour fills, boiling ink lines and paper grain, rendered
frame by frame in headless Chrome. Every frame is a pure function of time, so renders run in parallel and resume after
a crash.

Read [ANIMATION_GUIDE.md](ANIMATION_GUIDE.md) first: the rules (handmade, alive, one piece, timing by reads,
transitions always), the animation principles, the review loop and the full API.

## Run it

You need Node.js 18+, Google Chrome or Chromium, and ffmpeg (Python 3 with numpy + scipy for the audio tools).

```bash
npm install                                                     # p5, p5.brush, puppeteer-core (no browser download)
node render.mjs --sheet=0.3,1.6,2.3,2.8,4.3,6.2 --out=out/check/sheet.jpg   # look at it
node render.mjs --clip --out=out/video.mp4                      # the 7-second example, straight to MP4
```

Chrome is found automatically (macOS app, `google-chrome`, `chromium`, a Playwright Chromium). Otherwise pass
`--chrome=<path>` or set `CHROME`. On Linux without a GPU add `--soft-gl` (slow on watercolour fills); on a headless
NVIDIA box try `--gpu-angle=gl-egl` (`node gpu_probe.mjs <chrome>` shows which renderer each flag set gets). Open
`studio.html` in Chrome to scrub (serve the folder with `python3 -m http.server` if local files are blocked).

## What's here

| path | what it is |
|---|---|
| [ANIMATION_GUIDE.md](ANIMATION_GUIDE.md) | The rules, the workflow and the full API. Read it first. |
| [src/blot.js](src/blot.js) | Blot, the starter character: emotions, eyes, mouths, brows, emotes, talking, hand hooks |
| [src/core.js](src/core.js) | Painting, timing, motion helpers, camera, light, paper, vertical output, render hooks |
| [src/timeline.js](src/timeline.js) | Shots, loops and the brush-wipe transition |
| [src/props.js](src/props.js) | Shared sets and props (the example meadow and balloon) |
| [src/film.js](src/film.js) | The master timeline: VO cues, shot starts, subtitles |
| [src/shorts.js](src/shorts.js) | Vertical 9:16 cuts, painted natively at 1080x1920 |
| [src/sheets.js](src/sheets.js) | Model sheets as loops (`--loop=emotions`, `parts`, `props`) |
| [src/scenes/example.js](src/scenes/example.js) | "The Balloon", a 7-second example |
| [render.mjs](render.mjs) | Headless renderer: contact sheets, frame strips, crops, stills, frames, MP4 |

## Credits and license

The engine is based on [claude-animation-base](https://github.com/JohnHeibel/ClaudeAnimationBase) by John Heibel
(MIT), a kit distilled from his p5.brush music video. Retained and adapted from it: `src/core.js`, `src/timeline.js`,
`render.mjs`, `gpu_probe.mjs`, `studio.html`, `src/config.js` and most of `ANIMATION_GUIDE.md`. Its character rig,
model-sheet images and demo scene are not included.

Original additions in this skill: `src/blot.js` (the Blot character), `src/props.js`, `src/sheets.js`, `src/film.js`,
`src/shorts.js`, `src/scenes/example.js`, the vertical-output (`ow`/`oh`, `FILM_VIEW`), grade (`FILM_GRADE`) and
overlay (`FILM_OVERLAY`) hooks in `src/core.js`, the Blot, music and example sections of `ANIMATION_GUIDE.md`, and
everything outside `anim/` (audio tools, `assemble.sh`, `shorts.sh`).

Everything here is under the MIT license in [LICENSE](LICENSE). p5.js (LGPL-2.1) and p5.brush (MIT) are installed from
npm, not bundled.
