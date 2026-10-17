# Animating in p5.brush

Read this whole file before you draw anything. It covers how to make a short, hand-painted cartoon, starring Blot (the starter rig in `src/blot.js`) or any character you design: the rules and animation principles that make it look good, the workflow that catches mistakes, and the full reference for the character and the engine.

The person prompting you decides **what** the video is about. This guide decides **how** it's made. If they ask for something the rules below forbid (a caption, a 3D spin), do what they ask.

**No design here is final.** Blot, the emotions, the props and the helpers are a starting point, not a limit. Change any of them, Blot's own design included, and add whatever new characters, props or emotions the idea needs. Paint new things with the same tools and rules, so they belong with the rest.

Look at the model sheets first (render them, then open the images):
- `node render.mjs --loop=emotions --sheet=1 --cols=1 --w=1600 --out=out/check/emotions.jpg`: every emotion, alive.
- `node render.mjs --loop=parts --sheet=1 --cols=1 --w=1600 --out=out/check/parts.jpg`: mouths, eyes, brows, motion helpers and emotes.
- `node render.mjs --loop=props --sheet=1 --cols=1 --w=1200 --out=out/check/props.jpg`: the example set and prop.

---

## The three goals

Every rule below serves one of three goals.

- **Handmade.** The video should look like someone painted it by hand, frame by frame. That means brush strokes, ink lines that boil, flat 2D and no lettering.
- **Alive.** Something is always moving and something is always happening. Faces act instead of snapping, and characters are big enough to feel.
- **One piece.** It's one short film, not a pile of clips. Plan it before you draw it, and link every scene to the next.

Underneath all three, the viewer has to be able to follow it. Timing (rule 4) is the rule models get wrong most often.

## The rules

### 1. The medium is solid: brush strokes, flat 2D, boil

- **Paint everything with p5.brush through `paint()` and `inkLine()`.** Characters get flat `wash` colour plus an ink outline. Backgrounds get soft watercolour `fill` shapes, usually with no outline or a thin one. Never use plain p5 shapes (`rect`, `ellipse`, `fill()`): they look like 2000s Flash.
- **The linework boils.** `jit()` and `random()` are reseeded 12 times a second (`BOIL`), so every drawing wobbles slightly, like hand-drawn animation. That's the look; don't fight it. For anything that must stay put from frame to frame (star positions, tuft heights), use `hash(i)`. Give each separate element its own seed with `boilSeed(key)` (see Engine), or one moving thing makes everything drawn after it jitter.
- **Everything is flat 2D. Never project 3D.** Don't rotate a box in perspective, don't use `rotateY` or WEBGL 3D and don't fake depth with math. A character turns through **drawn key views** (front → 3/4 → side → back), exactly like a cartoon model sheet: draw each view as its own set of shapes and step through them with a smear. Blot is a front-facing design that shows direction with `lookX` and `flip`; add views to it (or to your own rig) when a film needs real turns. Depth comes from overlap, scale and colour (farther = smaller, bluer, paler), never from a projection.
- **Light is the one exception.** p5.brush mixes colour like pigment, so a yellow glow painted over blue turns green, and a thin wash over it turns grey. Use `glow()` for anything that shines: it adds real light, under the paper grain.
- **Soft palette, no pure black or white.** Use `PAL.ink` for black and `PAL.cream` or `PAL.paper` for white. Keep colours soft and harmonious, and keep Blot clearly readable against the background.

### 2. No text

- **Show it, don't write it.** Models overuse text. No captions, no titles, no labels on objects, no signs, no speech bubbles with words, no words on screens, no "ZZZ" typed in a font.
- **Blot's reactions are painted marks, never letters**: `!`, `?`, zzz, sweat, hearts, a bulb, a rain cloud. Use the emotes (see the reference).
- **A sign that repeats the story is the classic failure.** If Blot holds a sign saying "I'm lost", the shot has failed. Show Blot being lost: looking left, then right, the map upside down, a sweat drop.
- If the prompt truly needs a word (a name, a shop sign that is the joke), use `letter()`. Paint it into the scene, keep it to one or two words and use it once.

### 3. Something happens in every scene

- **Every shot needs an event:** something changes between its first frame and its last. Blot wants something, finds something, tries, fails, reacts or gets it. "Blot stands in a meadow being cute" is not a shot.
- **One focal action at a time.** Stage it with a clear silhouette and nothing competing for attention, so it reads at a glance.
- **Cause, then reaction.** When something happens, Blot reacts to it: a take, an emotion change, a turn toward it. The reaction is often the funniest part, so give it time.
- **Pay it off.** Whatever you set up in a shot (a door, a sandwich, a strange noise) gets resolved on screen, in that shot or a later one.

### 4. Timing: model the viewer

Timing turns a set of drawings into a story. It's also where generated animation fails most often: everything moves at one brisk speed, events pile on top of each other, and moments are over before anyone understands them.

You know what happens because you wrote the code. The viewer doesn't: they see it once, at full speed, for the first time. **For every moment, ask what the viewer needs to understand and how long that will take them, and time it for that.**

- **Write the reads.** For each shot, list in order what the viewer has to understand. Each item is a *read*. Every read needs time for the eye to find it, time to understand it, and a moment to register before the next thing starts. Small, distant, fast or subtle things take longer to find and understand than big, central, obvious ones.
- **One read at a time.** Don't start a new read while the viewer is still taking in the last one. When two things happen at once, the viewer sees only one of them. Put a cause and its reaction in sequence, not on top of each other.
- **Fast actions, slow meanings.** A motion can be very quick if it's anticipated, but what it means needs held time. Anticipation tells the viewer where to look before the action, and the hold after it lets them understand it. Move quickly through what doesn't matter to the story, and spend time on what does. That contrast between quick and held is what gives a film rhythm; one constant speed, fast or slow, makes it flat and hard to follow.
- **Lead the eye.** The viewer looks at whatever moves, is bright, is big or is being looked at. Before an important read, get their eye to the right place (a character looks at it, the camera moves to it, it moves or lights up first), and give the eye time to get there.
- **Let the reads set the length.** A shot is as long as its reads need. A shot with many reads can't be short, and a shot whose reads have all landed shouldn't be padded. That includes the last shot: its final read needs time to land before the video ends.

For a worked example, see how the example film times its second shot, at the end of this guide.

### 5. Alive

- **Nothing is ever still.** Every emotion has its own idle motion (`feel()`), cameras drift or push, grass sways, stars twinkle and the linework boils. A frozen frame reads as a bug.
- **Faces act, they never snap.** Change moods with `emotions()`. It does anticipation, a blink, a take and overshoot around every change. Never swap `eyes`/`mouth` by hand between two frames.
- **Move like a cartoon, not a machine.** Every move follows the animation principles in the next section.
- **Blot is big.** In a medium shot, `u` is about 22–30 (Blot is 8u wide and about 9u tall), so the character fills 40–60% of the frame height. In a close-up it's 45–70. A tiny character (u < 12) is for wide establishing shots only, and never for the whole video.
- **Everything moves on a beat.** `PROJECT.bpm` drives every idle, bounce and dance, so the whole film shares one pulse. Put the hits on beats (`pulse()`, `beatN()`), even with no music.

### 6. Transitions always

- **Every seam gets a transition:** into the first shot, between every pair of shots and out of the last one. Never start on a hard frame, and never just stop.
- **Pick a transition that belongs to the story**, and don't default to the same one every time. Some options:
  - a brush wipe (`brushWipe`)
  - an iris or shaped iris (`iris`, `irisShape`)
  - a whip pan with a smear
  - a match cut (the same shape or motion across the cut)
  - a cut on action (cut mid-move, and finish the move in the next shot)
  - a camera move that carries through into the next shot
  - a fade or push from paper or black
- A plain cut is fine only when it's on action or a deliberate smash cut.
- **Changes inside a shot are transitions too:** emotions go through `emotions()`, and turns step through drawn views. Props arrive and leave on arcs, never popping in.

### 7. One piece: a vision before any code

- **Storyboard first**, in writing, before you write any scene code (the workflow below has the format). If you're working with a person, show them the storyboard and let them react before you build.
- **One world.** Pick a palette and a setting that carries through, with a colour arc across the video (e.g. cold night → warm dawn as Blot's mood lifts).
- **One thread.** The story has a beginning, a middle and an end, and Blot's emotional arc follows it. Plan the emotion keys across the whole video, not per shot.
- **Rhyme the ending with the opening:** the same place, pose or motif, changed. It makes the film feel whole.
- **Link scenes:** motion continues across cuts, and screen direction stays consistent (if Blot travels right, keep travelling right). Props and characters carry over.

---

## Animation principles

These are the classic principles of character animation, as they apply here. Most of them fix one problem: motion written as code comes out mechanical, because code moves every part at once, on the same curve, by the same amount.

- **Anticipation.** Before a big move, make a small move the opposite way: a crouch before a jump, a wind-up before a throw, a squint before a take. It tells the viewer something is coming and where to look. `jump()` and `emotions()` build it in.
- **Squash and stretch.** Bodies squash on impact and stretch when they move fast, keeping their volume (`sq`).
- **Slow in, slow out.** Almost nothing moves at a constant speed. Things ease out of one pose and into the next. A plain `lerp` over time looks mechanical, so run its progress through an easing (`ease`, `easeIn`, `easeOut`, `backOut`).
- **Weight.** How something starts and stops says what it weighs. Heavy things take longer to get going and to stop, and land with little bounce. Light things snap into motion, bounce and flutter to rest.
- **Arcs.** Living things move on arcs, not straight lines: thrown props, hops, arm swings, head turns (`arcPt`).
- **Overlapping action and follow-through.** Don't move every part at once. The eyes lead, the body follows, and arms, hats, props and tails drag behind, overshoot and settle last. Offset each part's timing a little from the one it hangs off (`spring`, `ring` and `backOut` for the settle).
- **Avoid twinning.** Code copies values, so both arms end up at the same angle, both eyes blink together and a crowd bounces in unison. Give one arm the action and the other something smaller, and offset timings, phases and `seed`s between characters.
- **Exaggeration.** Push poses, takes, squash and leans further than feels natural. In a short cartoon, subtle reads as nothing. If it looks like too much on the sheet, pull it back.
- **Strong key poses.** Each shot's storytelling poses should read as stills, with a clear silhouette and the body leaning into what it's doing, before any motion goes between them. If the key poses don't read, the motion won't fix it.
- **Show the thought.** A character notices, thinks, then acts, and the eyes move first. The viewer understands a choice when they see it being made.
- **Secondary action.** Small actions that support the main one (a hat bobbing, an emote popping, grass stirring) add life, but they never compete with it.

---

## Workflow

### 1. Storyboard

Write `STORYBOARD.md` before any scene code:

```
Logline: one sentence. Blot wants ___, but ___, so ___.
World: setting, a small palette, light, how the colour changes across the video.
Motif: the thing that recurs and pays off.
Blot's arc: the emotion keys across the whole video.
Shots:
  A  start–end  [transition in: ___]  what's seen · the EVENT · Blot's reaction · camera
     reads:  start–end  the first thing the viewer must understand
             start–end  the next one (where is the viewer's eye when it starts?)
             ...
  B  start–end  [transition: ___]  ...
  ...
  [transition out: ___]
```

The reads are the timing sheet. Give each one a start and an end, make sure each has time to be found and understood, and make sure no two important reads overlap. If a shot's reads don't fit its length, lengthen the shot or cut a read; don't squeeze them.

Check the storyboard against the rules:
- Is there an event in every shot?
- Does every read have time to land before the next one starts?
- Is there a transition at every seam?
- Is there any text anywhere?
- Does the ending rhyme with the opening?

### 2. Build

- Set `duration` (and `bpm`) in [src/config.js](src/config.js).
- Put your scene in a new file (e.g. `src/scenes/my_video.js`), wrapped in an IIFE, and end it with `shots([...])`. In [studio.html](studio.html), **replace** the `example.js` script tag with yours (`new_film.sh` does this and wires one file per chapter).
- Build and check one shot at a time, in order.
- Within a shot, block the key poses first and check them as stills (`--sheet` at the key times). Add the motion between them once they read.

```js
// src/scenes/my_video.js
(() => {
  function park(t, lt, dur) {                        // t = video time, lt = time in this shot, dur = shot length
    camBegin(960 + 20 * Math.sin(lt * .6), 540, 1 + .02 * lt);   // slow drift and push: the camera is never dead
    paint(rectPts(-200, -200, W + 400, H + 400), { wash: PAL.sky, ink: null });           // background
    paint(ellPts(960, 1150, 1400, 380, 40, 2), { wash: PAL.sap, ink: PAL.ink, sw: 1 });   // ground
    const mood = emotions(lt, [[0, 'sad'], [1.2, 'surprised'], [1.7, 'excited']]);      // acted changes
    const hop = jump(lt, 2.2, 2.7, 3);                                                    // add poses that share fields
    blot(960, 860, 26, { ...mood, dy: mood.dy + hop.dy, sq: mood.sq + hop.sq });
    const at = toScreen(960, 860 - 4 * 26);          // Blot's screen position, for the iris
    camEnd();
    if (lt < .45) iris(...at, lerp(0, 1500, easeIn(lt / .45)));            // transition in
    if (lt > dur - .3) brushWipe((lt - (dur - .3)) / .6);                 // transition out (next shot finishes it)
  }
  shots([[0, park] /*, [3.5, nextShot], ... */]);
})();
```

### 3. Look at it: the review loop

You can't see motion by reading code. Render and look at every shot, several times, at three zoom levels:

```bash
# contact sheet: the shape of the whole piece (every shot's first, middle and last frames)
node render.mjs --sheet=0.1,0.8,1.6,2.4,3.1,3.9 --cols=6 --w=320 --out=out/check/sheet.jpg
# strip: EVERY frame of a moment (turns, takes, jumps, throws, transitions)
node render.mjs --strip=2.1:2.6 --cols=6 --w=320 --out=out/check/strip.jpg
# crop: full-resolution detail (faces, hands, contacts, glows); crop=x,y,w,h in frame pixels
node render.mjs --sheet=2.3,2.4 --crop=760,420,500,400 --w=500 --out=out/check/face.jpg
# crop-at: the same, following a WORLD point through each frame's camera (a foot or a prop on a moving shot);
# x,y in world pixels (or an expression evaluated in the page), w,h in frame pixels
node render.mjs --strip=2.1:2.6 --crop-at=960,700,500,400 --out=out/check/feet.jpg
```

Open each image and actually look at it. Check:

- **Read:** is the event of each shot clear from its sheet alone? Is Blot big enough, and does Blot separate from the background?
- **Timing.** You can't judge timing from single frames, so read it like a viewer:
  - Render the shot as a sheet at a fixed step (every 0.1–0.15 s) and read it in order.
  - At each frame ask: where is the viewer looking right now, and do they understand it yet?
  - Count the frames each read gets (24 frames = 1 s). A read that flashes by in a few frames, or shares its frames with another read, will be missed.
  - After each important moment, is there time to take it in before the next thing starts?
- **Motion:** in strips, does every move have anticipation and follow-through? Are there any pops, jumps or snaps between frames? Do the parts move at different times, or all at once? Is anything moving at a constant speed, or mirrored left and right? Are the poses pushed far enough to read?
- **Boil:** in a strip, each pair of frames that share a boil drawing should match except where something moves. Anything still that changes every frame needs its own `boilSeed()`.
- **Contacts:** do feet touch the ground? Do held things touch the arm tips? Do thrown things leave from the hand?
- **Transitions:** check the first and last 0.5 s of every shot and every seam. Does it open and close with a transition?
- **Rules:** is there any text? Is there any 3D? Is there any dead stretch where nothing is happening?
- **Colour:** any muddy glows (use `glow()`), pure black or pure white?

Fix what you find, then look again. **Budget:** at least one sheet per shot, a strip for every key motion and transition, and a crop for every face that carries the story. Contact sheets run about 0.1–1 s per frame, so this is cheap: don't skip it.

### 4. Render

```bash
node render.mjs --clip --out=out/video.mp4                      # the whole video, straight to MP4
node render.mjs --frames --workers=4                            # or: parallel + resumable JPEG frames into out/frames …
node render.mjs --encode --out=out/video.mp4                    # … then encode them
```

---

## Engine

### Files

| file | what's in it |
|---|---|
| `src/config.js` | `PROJECT = { duration, bpm, offset, audio? }` |
| `src/core.js` | canvas, palette, timing and motion helpers, `paint()`, camera, full-frame effects, `glow()`, lettering, paper, render hooks |
| `src/blot.js` | Blot, the starter character: body, arms, feet, eyes, mouths, brows, emotes, emotions, `talk()` |
| `src/props.js` | the film's shared sets and props (the example meadow and balloon), `superText()` |
| `src/film.js` | the master timeline: VO cues, shot start times, subtitles |
| `src/shorts.js` | vertical 9:16 cuts of the film |
| `src/timeline.js` | `shots()`, `LOOPS`, `brushWipe()` |
| `src/sheets.js` | the model sheets as loops (`?loop=emotions`, `?loop=parts`, `?loop=props`) |
| `src/scenes/example.js` | a 7-second example. **Don't copy it** (see the end of this guide) |
| `studio.html` | open it in Chrome to scrub the video (`?t=2.5` jumps to a time, `?loop=emotions` shows a loop). Serve the folder (`npx http-server` or `python3 -m http.server`) if your browser blocks local files |
| `render.mjs` | headless renderer: sheets, strips, crops, stills, PNG loops, MP4 |

### Frames are pure functions of time

- **Frames render in parallel and out of order.** A shot is `fn(t, lt, dur)` and must draw the same frame for the same `t`, every time. No state carried between frames, no counters, no `Math.random()`, no physics that integrates frame by frame. Compute everything from `t`, in closed form (the helpers below do this for you).
- Randomness: `hash(i)` for stable per-object values, and `jit(a)`/`random()` for boil (they change 12 times a second).
- **Seed each element with `boilSeed(key)`.** Each boil drawing holds for two frames, so anything that isn't moving must draw the same in both. But a moving thing uses a different amount of randomness each frame, which shifts the stream for everything drawn after it, and all of that re-boils every frame and looks jittery.
  - `boilSeed(key)` restarts the stream from the boil frame and a key that stays the same every frame (any string or number, unique within the frame).
  - Call it before each separate element: each background layer, prop and effect.
  - `blot()` seeds itself and each of its parts, then reseeds when it's done, so nothing drawn after it depends on its pose. Its key is its call order; set `boilKey` if characters come and go mid-shot.
- **Each shot paints the whole frame,** background included. The paper texture is under everything and the grain is multiplied over the top, so leaving paper showing is a valid look.
- **Canvas:** 1920×1080, origin top-left, y down.
- `LOOPS.name = t => {...}; LOOPS.name.len = 4;` makes a standalone loop (tests, GIFs, sheets), rendered with `--loop=name`.

### Painting

`paint(pts, o)` paints one shape from a point list `[[x, y], ...]`:

| option | meaning |
|---|---|
| `wash, washOp` | flat colour (opacity 0–255, default 255). For characters, props, anything solid. |
| `fill, fillOp, bleed, tex, border` | watercolour fill with bleeding edges and pigment texture. For skies, hills, shading and shadows. `bleed` ~.05–.3, `tex` ~.3–.9. |
| `hatch: { d, a, o, b, c, w }` | hatching (distance, angle, `{rand, gradient}`, brush e.g. `'charcoal'`/`'HB'`, colour, weight). Texture, sparingly. |
| `ink, sw, br` | outline colour (default `PAL.ink`), weight (~.4–2), brush. **`ink: null` means no outline.** |
| `curv` | 0–1: smooth the outline through the points. |

- **Lines:** `inkLine(pts, sw, colour, brush = 'ink', curvature)`. The kit's brushes are `'ink'`, `'inkfine'` and `'dry'` (bristly). p5.brush's built-ins also work: `'2B'`, `'HB'`, `'charcoal'`, `'marker'`, `'pen'`, `'cpencil'`, `'rotring'`, `'spray'`.
- **Shapes:**
  - `rectPts(x, y, w, h, jitter)`
  - `ellPts(cx, cy, rx, ry, n, jitter, rot)`
  - `rrPts(x, y, w, h, r, jitter)`: a rounded rectangle
  - `starPts(cx, cy, r, inner, n, rot)`
  - `heartPts(cx, cy, r)`
  - `through(P)`: a smooth curve through the points
  - `ribbon(P, w0, w1)`: a tapered ribbon along a path, as one outline. Use it for tails, trails, vines and noodly arms.
- **One shape, one outline.** Build a creature or prop from as few outlines as you can, so it doesn't look like glued-on stickers: a tail is one `ribbon`, not five circles.
- **Light:** `glow(x, y, r, colour, a)`. It's additive, so it stays warm on dark grounds and barely shows on light ones (as real light would). Draw it before the things that sit in front of the light.
- **Palette** `PAL`: `paper, ink, clay, clayDk, clayLt, night, indigo, rose, ochre, sap, teal, violet, cream, sky`. `mixCol(a, b, k)` mixes two hex colours in RGB. Any hex colour works: pick a small palette per video.
- **p5.brush quirks:**
  - Colours mix like pigment: yellow over blue makes green. Layer light colours over dark ones with a full-opacity `wash`, or use `glow()`.
  - `wash` at 255 is exact colour; lower opacities mix.
  - Strokes drawn far from the origin under a zoomed camera collapse: from zoom ~2, an outline or a line at large
    world coordinates leaves only a dot at its first vertex. `paint()` and `inkLine()` draw each shape around its own
    centre, which avoids it. A shape much bigger than the canvas can still lose its outline: draw long edges as
    `inkLine`s no bigger than the canvas.
  - Outline weight is in world units, so it grows with the camera's zoom: a fine outline on a small shape becomes a
    dark blob in a close-up. Scale `sw` down with the zoom for small shapes.
  - A NaN in a point list throws `Failed to construct 'OffscreenCanvas': Value is not of type 'unsigned long'`, with
    a stack pointing at your scene rather than the NaN. Guard geometry that can degenerate (`Math.acos` of a ratio > 1).
  - p5 `push()/pop()/translate()/rotate()/scale()` work with all brush calls.
  - Cost is the number of `fill` shapes and strokes: hundreds are fine, thousands are not. Aim for ≤ 1.5 s per frame. The render log prints ms/frame.
  - Some scenes make p5.brush log five `WebGL: INVALID_OPERATION ... not from the associated program` warnings once per page. They're harmless (frames come out identical). Any other page error is real.

### Time and motion (all pure functions of t)

- **Progress and keys:**
  - `seg(t, a, b)`: 0..1 progress through [a, b]
  - `kf(t, [[t0, v0], [t1, v1], ...], ease)`: keyframes; values may be arrays
  - `lerp`, `clamp`, `frac`, `wob(t, freq, phase)`, `TAU`
- **Easing:** `ease`, `easeIn`, `easeOut`, `backOut` (overshoot) and `elasticOut`.
- **Rhythm:** `BEAT` (seconds per beat), `bpOf(t)` (beat position), `beatN(t)` (beat number), and `pulse(t, k)` / `pulse2(t, k)`, which are 1 on each beat (or eighth) and then decay.
- **Acting:**
  - `jump(t, t0, t1, h)`: crouch, stretch, arc and squash-land. Returns `{dy, sq}`.
  - `take(t, t0, amt)`: a surprise take, returns `{sq, dy}`.
  - `stroll(t, t0, t1, x0, x1, u)`: an eased walk. Returns `{x, walk, flip, dy}` (plus a `view` hint for rigs with drawn views).
  - `spring(t, t0, k, w)`: a damped wobble after an event, for settles and follow-through.
  - `ring(t, [t0, t1, ...])`: one `spring` kick per event time.
  - `arcPt(p0, p1, h, k)`: a point on a thrown arc.
  - `onTwos(t)`: holds each drawing for two frames. Wrap a shot's `t` in it for a snappier, hand-drawn feel.
- **Camera:**
  - `camBegin(cx, cy, zoom, rot)` … `camEnd()`: world point (cx, cy) lands at screen centre. One level only; always pair them.
  - `toScreen(x, y)`: world → screen while a camera is active. Use it to aim an iris at a character.
  - `shakeXY(t, amount)`: [dx, dy] to add to the camera on impacts.
- **Full-frame effects** (screen space, after `camEnd()`):
  - `brushWipe(p, [c1, c2])`: fat strokes cover the frame (p 0 → .5) and then drag off (.5 → 1). Cut under full cover. Its comment in timeline.js shows the two calls.
  - `iris(cx, cy, r, colour)` and `irisShape(pts, colour)`: shaped reveals.
  - `flash(k, colour)`: a full-frame flash.
- **Lettering** (only if you must, see "No text"):
  - `letter(txt, x, y, size, colour, {pop, rot, alpha, screen})`
  - `sfx(txt, x, y, size, colour, age)`
  - Both are composited at `flushLetters()`, after the shot. If a wipe or iris must cover them, call it yourself first.

---

## Blot, the starter character

Blot is a round ink-blob: one blob body with a pale belly, two noodle arms, two little feet, big eyes and an ink drip on
top. It's drawn in flat wash with an ink outline and watercolour shading, and boils like everything else. It is a
starting point, not a rule: recolour it, change its shape, add a hat, or build your own character in the same style.
Within one video, though, keep every character on model, with the same shape and features in every shot.

```js
const pts = blot(x, y, u, options)   // (x, y) = ground point between the feet; u = size unit
// pts = { head, top, handL, handR, mouth }: world points, for attaching props, aiming an iris or emotes
```

| shot | u | on screen |
|---|---|---|
| wide | 8–14 | small in the landscape |
| medium | 22–30 | the usual acting size |
| close-up | 45–70 | face acting |

### Options

| group | options |
|---|---|
| pose | `dx`, `dy` (in u; −dy = up), `sq` (squash; negative stretches), `rot` (pivots at the feet), `flip`, `aL`, `aR` (arm angle: 0 = straight out, + = up, − = down; ±1.5 is vertical), `walk` (foot phase), `lift` (rises off the ground, the shadow stays), `noShadow` |
| face | `eyes` (open, wide, happy, closed, half), `lookX`/`lookY` (−1..1), `blink` 0..1, `pupil` (scale), `brow` (−1 worried .. +1 angry), `mouth` (smile, grin, o, flat, frown, wavy, open), `talk` 0..1, `blush` 0..1 |
| colour | `col` (body), `belly`, `gray` 0..1 |
| extras | `emote` (`!`, `?`, sweat, heart, spark, zzz) + `emoteK` (0..1 pop), `holdL`/`holdR`: `(x, y, u, sw) => {...}` paints a prop at that hand, in world coordinates |
| boil | `boilKey`: a stable id for its boil seeds (default: call order; set it when characters come and go mid-shot) |

Options compose by spreading: `blot(x, y, u, { ...feel('happy', t), ...jump(t, 1, 1.4, 3), aR: 1.2 })`. Later spreads
win, so put the emotion first and the pose after it. If both an emotion and a pose move the same field (`dy`, `sq`), add
them together rather than letting one silently replace the other.

### Emotions

13 emotions, each a face, a colour and a way of moving, locked to the beat: neutral, happy, excited, sad, surprised,
angry, worried, sleepy, love, smug, scared, idea, curious. `feel(name, t, over)` returns all of it at time t:

```js
blot(x, y, u, feel('happy', t));                                   // one emotion, alive
blot(x, y, u, feel('sad', t, { lookX: -1 }));                      // with overrides
blot(x, y, u, emotions(t, [[0, 'sleepy'], [1.9, 'surprised', { lookX: .8 }], [2.5, 'idea']]));   // acted changes
```

Add your own by adding an entry to `EMO` in `blot.js` (a function of t returning options).

### Talking

`talk(t, [[start, dur], ...])` returns a 0..1 mouth opening while a line is spoken. Feed it the VO cues from `film.js`:
`blot(x, y, u, { ...feel('happy', t), talk: talk(t, VO.filter(v => v[1] === 'v03').map(([a, id]) => [a, DUR_VO[id]])) })`.

### Holding things

`holdR: (hx, hy) => balloon(hx + 70, hy - 300, 64, hx, hy)` paints the prop at the hand, so it stays attached through
every squash, hop and lift. Draw glows for a held prop inside the hook too.

## Write your own character

Copy the shape of `blot()`:
1. Seed each part with its own `boilSeed()` key (via a small `rs(part)` helper), then `boilSeed('after ' + id)` at the end.
2. `push(); translate(x, y); rotate(rot); scale(flip * (1 + sq * .5), 1 - sq);` so squash pivots at the feet, then paint
   in units of `u` with `paint()`, `ribbon()` and `inkLine()`. One shape, one outline: a limb is one `ribbon`.
3. Draw back to front: far limbs, body, belly/shading, face, near limbs.
4. Return the world points props attach to (hands, head, mouth).
5. Add an `EMO`-style table so the character acts with `feel()`/`emotions()`, and a model-sheet loop in `sheets.js`.
6. Render the sheet and look at it before anyone builds shots on the character.

## Music (optional)

The kit doesn't need music, but it's built for it:

1. Set `bpm` to the song's tempo in [src/config.js](src/config.js), and set `offset` to the time of its first downbeat in seconds (`python3 ../audio/beat.py <song>` estimates both). Every idle and `pulse()` then locks to the song.
2. `../audio/build_mix.py` writes `assets/mix.wav` (`PROJECT.audio`), which `--clip` and `--encode` mux in when it exists.
3. Land hits, cuts and takes on beats (`beatN`, `pulse`). Cut on bar lines for big changes, and give each musical phrase its own visual.
4. **Lyrics are not text.** Don't put words on screen beyond subtitles. Act the meaning of a line instead.

## Common failures

These are the things that make a generated cartoon look generated. Check your storyboard and sheets against them:

- signs, captions, labels or speech bubbles with words
- a character standing still and smiling while nothing happens
- everything moving at one brisk speed, with events stacked on top of each other and no holds
- moments that are over before the viewer understands them
- a tiny character in a big empty landscape for the whole video
- faces that snap from one expression to another
- mechanical motion: linear moves, every part moving at once, both arms or several characters in sync
- timid poses and takes that barely read
- jittery linework: still things re-boiling every frame because something moving before them shifted the random stream (`boilSeed`)
- hard cuts everywhere, or a video that just starts and stops
- 3D rotation, perspective boxes or projected turns
- plain p5 shapes, gradients or digital glows mixed into the paint (use `paint`/`inkLine`/`glow`)
- muddy green-grey glows from painting yellow over blue
- props floating near a hand instead of touching it
- every shot a different world with nothing linking them

## About the example

[src/scenes/example.js](src/scenes/example.js) ("The Balloon") exists to show the kit working: an acted emotion timeline, a hop, a prop held by a hand hook, `glow`, a brush wipe between shots and an iris in and out. **It's one idea, not a template.** Don't reuse its story, meadow, balloon or shot structure. Start from the prompt and your own storyboard, and replace its script tag in `studio.html` with yours.

### Worked example: how the example times its second shot

This shows rule 4 applied to one shot. Read it for the reasoning, not the numbers. Times are video seconds:

| time | read | why it's timed this way |
|---|---|---|
| 3.8–4.1 | the wipe drags off: Blot has the balloon | the cut happened under full cover, so the payoff of shot A lands at once |
| 4.1–4.5 | Blot is thrilled | one read: a big excited bounce, nothing else moves much |
| 4.5–5.25 | it settles into happiness | a held beat, so the next change reads as a change |
| 5.25–5.75 | love: blush and hearts pop | the emote pops after a blink, cause before effect |
| 5.75–6.3 | the balloon tugs; Blot rises off the ground, surprised | the camera eases up with it; the shadow stays put, which is what sells the lift |
| 6.3–6.55 | Blot laughs, floating | the reaction, after the cause |
| 6.55–7.2 | iris closes on Blot, holds, shuts | the last read gets time to land before the video ends |
