# Motion design playbook

The taste layer. The engine makes things move; this is about what makes them read as designed.

## Contents
1. Pacing numbers
2. Composition & typography
3. Scene recipes
4. Transition catalog
5. Layered detail (what "go all out" means)
6. Common failures seen in contact sheets

## 1. Pacing numbers
- Entrances: 0.3–0.5s with `E.outExpo` (fast start, long settle). Almost never linear.
- Exits: 0.15–0.25s with `E.inExpo`. Exits are faster than entrances.
- Staggers: 0.04–0.1s between siblings (letters/words/tiles). Under 0.03 reads as simultaneous; over
  0.12 drags.
- A scene needs ~0.4s of *readable rest* before it leaves, or the viewer can't parse it.
- Something should move in every frame (a slow camera push of 1.00→1.06, a drifting grid, orbiting
  dots), but only one thing should *demand* attention at a time.
- Hit density: roughly one big hit per 2s and small accents on beats between. All big all the time =
  no big.

## 2. Composition & typography
- Pick 1920×1080 safe margins: ~150px left edge for text blocks, 44px HUD inset.
- Two families only: a heavy display face for statements (800–900 weight, negative tracking −2 to −6px
  at 100px+), a mono for labels/HUD/code (wide tracking 4–16px, uppercase). Mono labels above big
  headlines make it feel engineered.
- Headline sizes: statements 220–280px, section titles 90–130px, lines of copy 44–66px, labels 15–26px.
- Accent color is precious: use it on 1–3 elements per frame (the core block, a cursor, one bar).
  If the brand restricts where the accent may appear, obey that.
- Left-aligned text + a visual on the right is the default capability layout; center for lockups and
  single-line promises.
- Outline (stroked) giant numbers or words at 10–18% alpha behind content add depth cheaply.

## 3. Scene recipes
- **Logo assembly**: grid draws on → swarm streaks converge → blocks fly in from random far points
  ordered by distance from the logo's core (`assembleBlocks … order:'distance'`) with ghost trails →
  shockwave `ring` when the last lands → label types beneath → blocks explode outward into the cut
  (`E.inExpo` over the last 0.2s).
- **Kinetic statement**: small mono lead-in types (0.3s) → big words `slam` on consecutive beats →
  accent element lands as punctuation (e.g. the brand's core shape as the full stop) → rule line
  draws → supporting line types. Backdrop: huge stroked marquee text drifting in opposite directions.
- **Capability triplet**: one per second. Outline number behind, mono "0N / 03" label, 2-line title
  rising, caption typing, and a *specific* mini-visual on the right (audit map with scan line, agent
  graph with packets traveling edges, terminal typing a plan). Each visual must be different in kind.
- **Product windows**: 3 app windows rise with stagger and slight rotation/skew, idle float via
  `sin(lt)`. Inside each, one live behavior: chat streaming after a thinking indicator, code diff
  highlighting then an approve button click, a cursor path with click ripples. A mascot can pop from
  behind a window with `outBack`.
- **System/family grid**: tiles flip in (scaleX via `outBack`), then a highlight chases across them,
  then all get a finished state (accent underline). Orbits/ellipses behind.
- **Breakdown promise**: columns/stripes collapse to center → a shape draws itself (dash offset) → a
  single line rises word by word. Quiet, spacious, music thins.
- **Lockup**: core element travels from the previous scene's focal point into the logo position, rest
  of the logo bursts out of it, specular light sweep across, tagline words rise, URL + underline
  grows, 0.4s hold, fade.

## 4. Transition catalog (vary them; never the same twice in a row)
- **Hard cut + flash + shake**: the drop and big moments. `flashes` 0.5–0.95, `hits` 14–22.
- **Slice glitch**: 0.2–0.3s ramp into the cut, short decay after. For tech/edge moments.
- **Block wipe**: diagonal grid of brand-colored squares covers then uncovers (`wipes`). Great if the
  brand is geometric.
- **Bar wipe / push**: content slides out left while a thick bar sweeps; for sibling panels.
- **Match cut**: an element persists across the cut (the core block from box → logo). The most
  "designed" transition; plan at least one.
- **Implode/explode**: everything collapses to a point, flash, next scene bursts from it.

## 5. Layered detail
A frame that looks premium usually has 5–6 layers: background texture (grid, marquee, orbits) →
main content → secondary motion (particles, packets, rings) → transition/FX → grain + vignette →
HUD (corner brackets, label, running timecode, scene index, progress line). The HUD alone makes it
read as a "reel". Grain at ~0.07 overlay and a vignette hide the flatness of canvas fills.

## 6. Common failures seen in contact sheets
- An effect using `P()` that never turns off → ghosting/glitch in every later frame. Use `win()`.
- Nested `mix()` → black elements.
- Big empty lower half (content crammed at the top): shift content down or add a secondary element.
- Text overlapping the HUD corners or the bottom ticker.
- Two consecutive scenes with the same layout (both left-text/right-box) → vary the composition.
- Mock UI with lorem-ipsum-feeling copy; write short, plausible, on-brand strings.
- Fonts silently falling back (check the label glyphs look like the intended face in the sheet).
- Audio: drop and finale so loud the groove sounds weak after normalization. Check `waveform.png`.
