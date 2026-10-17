# Chapter agent brief: fill in the <...> and send one per chapter

You are animating one chapter of a short hand-painted cartoon ("<FILM TITLE>", <one-line genre/premise>) in a p5.js +
p5.brush project. Project root: <film dir>/anim

YOUR CHAPTER: <chN name>: shots <names> (video <start>-<end> s). Write ONLY src/ch/<chN_name>.js (it holds a stub
comment; replace it).

Read these first:
- ANIMATION_GUIDE.md (the rules and the API). Skip sections you already know.
- STORYBOARD.md: only the rows for your chapter, plus the "World", "Motif" and "On-screen words" sections.
- The rig and prop files the chapter uses: src/blot.js (option docs at the top), src/props.js, any other rig in src/.
- src/film.js (VO cue times, SHOT_T shot times).
Skim src/core.js and src/timeline.js for helpers (camBegin/camEnd, brushWipe, iris, irisShape, flash, glow, kf, seg,
ease, backOut, spring, jump, take, arcPt, pulse, boilSeed, letter, superText). Look at out/check/*.jpg model sheets.

Rules:
- **Files:** only edit your chapter file. Shared files (core, timeline, rigs, props, film, studio.html, render.mjs) are
  read-only. If you hit a real bug in one, work around it inside your IIFE and report it.
- **Structure:** wrap the file in an IIFE and register with `shots([[SHOT_T.<a>, fnA], ...])`. Each shot fn(t, lt, dur)
  paints the whole frame and must be a pure function of t: no state, no Math.random (use hash()). Compute shot lengths
  from SHOT_T differences, not `dur`: while later chapters are stubs, the last shot's dur runs to the end of the film.
- **Characters:** keep them big (a medium shot is 40-60% of frame height) and acting through emotions(), never snapping.
  If a character speaks, drive its mouth with talk(t, cues) from the VO cues.
- **Text:** <list exactly which words may appear in this chapter>. Nothing else. Keep key action above y = 960, because
  VO subtitles are drawn automatically in the bottom band.
- **Every shot:** an event, a camera move, and a transition at both seams. Into this chapter: <how the previous chapter
  ends, e.g. "brushWipe(.5 + lt / .6, [FP.hillDk, FP.hill])">. Out of it: <how the next chapter starts>. Time everything
  to the reads; put hits on beats where natural (BPM <bpm>, offset <offset>).
- **Pigment:** p5.brush mixes like paint (yellow over blue goes green). Use an opaque wash with a pre-mixed mixCol()
  colour, or glow() for light. letter() ignores push/translate: pass world coordinates. Never name a variable `push`.
- **Check your work**, open the images, actually look, and iterate until each shot reads clearly and looks good:
  - `node render.mjs --sheet=<6 times> --cols=3 --w=640 --out=out/check/<ch>_a.jpg` per shot
  - `--strip=a:b --cols=6 --w=320` for key moves and seams; aim for <= 1 s per frame
- **Budget:** about <30-60> tool calls. Stop and report if you hit it. Redirect render output to a log; read `tail -3`.

Final message (also append it as one line to HANDOFF.md):
- what each shot does now;
- **the time (s) of every sound-worthy hit** (impacts, pops, whooshes, clicks), for the SFX pass;
- the sheet image paths; ms per frame; any shared-file bug you found.

Do not render the full video or touch the audio.
