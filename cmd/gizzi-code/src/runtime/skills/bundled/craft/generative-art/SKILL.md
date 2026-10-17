---
name: generative-art
description: Make original generative or algorithmic art in code (plain canvas or p5.js) with seeded determinism, an interactive viewer (sliders, seed field, seed grid, hi-res PNG export), composition principles, recipes for common systems (flow fields, particles, recursive subdivision, noise landscapes, circle packing, L-systems), hi-res still rendering, and a look-and-fix loop over seed grids. Use when asked for generative art, algorithmic art, creative coding, procedural patterns, a flow field, particle art, a p5.js sketch, a print or wallpaper made from code, or "art from a seed". Produces original work, never imitations of a living artist's signature style.
---

# Generative art

A generative piece is a **system plus a seed**. The system is your code and its parameters; the seed picks one output from the space the system can make. Your job is to build a system whose space is mostly good, then find the best seeds in it by looking.

Order of work: **concept → system in the viewer → seed grid → look → adjust → pick seeds → hi-res stills.**

## 1. Start from the viewer

Copy the template next to the user's work and edit only its `SYSTEM` block:

```bash
mkdir -p art && cp ${SKILL_DIR}/assets/viewer.html art/viewer.html
```

`viewer.html` is a single self-contained file (no CDN, opens from `file://`). It gives you:

- `PARAMS`: a list of `{ key, label, min, max, step, value }`; a slider is generated for each.
- `PALETTES`: named lists of 3–5 colours, the first one is the background.
- `draw(g)`: paints one image. `g` has `ctx, W, H, u, rand, noise, P, colors, seed`.
  - `rand()` is a seeded uniform 0..1 generator, `noise(x, y)` seeded 2D gradient noise (about −1..1).
  - `u` is 1/1000 of the short side. Express every length, step and line width in `u` so the same seed composes identically at 900px and 6000px.
- A seed field with prev/next/random, a **seed grid** (3x3 neighbours; click one to open it), palette picker, PNG export at 2000–6000px, and a shareable URL hash (`#seed=42&palette=1&p.density=0.9`).
- `window.renderStill(seed, size)` returns a PNG data URL, used by the render script.

The shipped example is a flow field so you can see every part working; replace it with your own system.

If the user wants p5.js, keep the viewer and add `<script src="https://cdnjs.cloudflare.com/ajax/libs/p5.js/1.9.4/p5.min.js"></script>` (exact version, never a floating tag), then draw into an offscreen `p5.Graphics` in instance mode and blit it to `g.ctx`. Call `randomSeed`/`noiseSeed` from the seed and never use p5's unseeded `random()` before seeding. Plain canvas is usually enough and has no dependency.

## 2. Determinism rules

One seed must always give the same image. Breaking this makes seeds worthless.

- No `Math.random()`, `Date.now()`, `performance.now()` or frame counts inside `draw`. Use `g.rand` and `g.noise` only.
- Consume randomness in a fixed order. If an optional feature draws extra random numbers, give it its own generator (`makeRand(seed + ':feature')`) so toggling it doesn't reshuffle everything else.
- No resolution-dependent logic: loop counts and sizes come from parameters and `u`, not from `W` in pixels.
- Avoid iteration over object keys or `Set`s whose order depends on insertion timing.
- Verify with the check command below whenever you change the system.

## 3. Composition

Random is not the same as composed. Build these in deliberately:

- **A focal point.** One area of highest density, contrast or scale, usually off-centre (near a third). Sample start positions with a bias toward it (rejection sampling against a distance falloff works well).
- **Density variation.** Dense, medium and sparse regions. Uniform texture edge to edge reads as wallpaper.
- **Negative space.** Leave real emptiness: a margin, a quiet band, or a void the forms flow around. It is what makes the dense parts look intentional.
- **Palette from 3–5 colours** with clear value (lightness) contrast: one background, one dominant ink, one or two supports, one rare accent used in under 5% of marks. Check that it still reads in greyscale.
- **Scale hierarchy.** A few large marks, more medium ones, many small ones.
- **Edges.** Decide whether forms are cropped by the frame, held inside a margin, or fade out. Hard clips at a margin can look like an accident; fade or stop strokes before the boundary if that happens.

## 4. Systems, briefly

Each recipe is a starting point; combine and distort them.

- **Flow field:** angle = `noise(x*s, y*s) * TAU` (add a second octave for turbulence). Drop particles, step them along the angle, draw the path with thin, translucent strokes. Variants: curl of noise for swirl, field lines that avoid each other (keep a spatial hash of occupied cells), angle quantised to a few directions for a woven look.
- **Particles with forces:** each particle has position and velocity; apply attraction to points, repulsion from each other, damping. Draw trails rather than dots. Seed the attractor layout for composition.
- **Recursive subdivision:** start with the canvas rectangle; split it at a random ratio (0.3–0.7) along the longer side; recurse with a probability that falls with depth and with distance from the focal point. Fill leaves with flat colour, hatching, or a nested system.
- **Noise landscapes:** for rows `y` from back to front, draw a polyline `y + amp * noise(x*s, row*t)`, fill below it with the background to occlude rows behind it. Vary amplitude by row for depth, use fewer colours toward the back.
- **Circle packing:** propose random centres; accept a circle at the largest radius that doesn't overlap existing ones (cap radius, cap attempts). Bias the radius cap by a noise field or distance to the focal point for density variation. A grid or quadtree keeps it fast.
- **L-systems:** an axiom plus rewrite rules (e.g. `F -> F[+F]F[-F]F`), iterate a few generations, then interpret with a turtle (F forward, + / − turn, [ ] push and pop). Seed the turn angle jitter and branch length decay so each seed is a different plant.
- **Others worth knowing:** Voronoi/Delaunay from seeded points, reaction-diffusion (slow; render once, then export), differential growth, truchet tiles, wave interference, dithering a noise field.

Keep `draw` fast enough that a 3x3 grid renders in seconds at preview size. Raise counts only for the final still.

## 5. Look and fix

You can't judge generative work from code or a single seed. Render a grid of seeds and look at it.

```bash
node ${SKILL_DIR}/scripts/render.mjs --viewer art/viewer.html --grid 3 --seed 1 --out art/review/grid-1.png
node ${SKILL_DIR}/scripts/render.mjs --viewer art/viewer.html --grid 4 --seed 100 --size 1600 --out art/review/grid-100.png
node ${SKILL_DIR}/scripts/render.mjs --viewer art/viewer.html --check --seed 42      # determinism
node ${SKILL_DIR}/scripts/render.mjs --viewer art/viewer.html --grid 3 --p density=1.2 --palette 2 --out art/review/dense.png
```

The script uses the installed Chrome (or `$CHROME`), installs `playwright-core` into a cache on first use, and prints any page errors. Open the PNG with your image reading tool and critique it:

1. **Hit rate:** how many of the 9 seeds are good? Under half means the system needs work, not more seed hunting.
2. **Sameness:** do all seeds look nearly identical? Push the seed into composition (focal position, palette weights, structure), not just texture.
3. **Composition:** is there a focal point, density variation and negative space in most cells?
4. **Failure modes:** empty cells, mud (too many overlapping translucent marks), hard clipping, a single colour dominating, visible grid artefacts from the noise.
5. **Detail at print size:** render one still at full size and look at a crop; thin strokes that read at 900px can vanish or alias at 6000px.

Fix the worst two or three problems in the system or parameter ranges, re-render the same grid (same starting seed, so you are comparing like with like), and repeat. Then scan a few more grids to collect keepers. Write the keepers down as seed + palette + any non-default params, or keep their URL hashes.

## 6. Hi-res stills

```bash
node ${SKILL_DIR}/scripts/render.mjs --viewer art/viewer.html --still --seed 42 --size 4000 --out art/out/seed-42.png
```

Or use the viewer's PNG button. For print: 300 dpi means 3000px per 10 inches; square pieces at 4000–6000px are typical. Large renders of heavy systems can take tens of seconds. If a render takes far longer, lower stroke counts with size rather than raising them, since `u` already scales stroke width.

## 7. Originality

- Build from systems and your own composition choices, not from a particular artist's look. Do not recreate a living artist's recognisable signature work or series (their specific motifs, palettes and layouts together) even when asked to make something "in the style of" them. Offer an original piece that shares the general technique instead (e.g. "a flow field with sparse heavy strokes" rather than a copy of someone's named series).
- Techniques themselves (flow fields, packing, subdivision) are common ground and fine to use.
- Name palettes and pieces yourself; don't reuse another artist's titles.

## Checklist before you hand it over

- [ ] Same seed renders identically (`--check` passes)
- [ ] Sizes in `u`: preview and 4000px still compose the same
- [ ] Seed grid viewed; most seeds are good and they differ meaningfully
- [ ] Focal point, density variation, negative space, 3–5 colour palette
- [ ] Keeper seeds listed with palette and params
- [ ] Hi-res stills rendered and one checked at full size
- [ ] Work is original, not a copy of a living artist's signature style
