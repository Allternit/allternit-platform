---
name: visual-design
description: Make web and app UI look deliberate and finished instead of templated. Covers choosing an aesthetic direction before writing code, type scale and font pairing, a spacing system, colour as tokens with dark mode, layout and hierarchy, detail density, imagery, empty/loading/error states, contrast, and phone-width layout, then a screenshot review loop (desktop + phone, light + dark) where you critique the render and fix the three worst problems. Use when building or redesigning a page, landing page, marketing site, dashboard, settings screen or component styling, or when asked to "make it look better", "polish this", "make it premium/modern/cleaner", or "it looks generic". Respects an existing design system when the project has one.
---

# Visual design

The goal is a page that looks like someone made decisions. Generic output comes from skipping decisions: default font, default grey, everything centred, equal spacing everywhere, a gradient blob for a hero. This skill forces the decisions early, then checks them with your eyes on a real render.

Work in this order: **read what exists → pick a direction → set tokens → lay out → fill states → screenshot → critique → fix → repeat.**

## 0. Read what already exists

Before inventing anything, look for a design system and obey it.

- Search for tokens: `tailwind.config.*`, `theme.ts`, `tokens.json`, CSS files with `:root {` custom properties, `@theme` blocks, a `components/ui/` folder, Storybook.
- Note the existing font families, colour variables, radius, spacing scale and component primitives (Button, Card, Input).
- If they exist, **use them**. Your job shrinks to composition, hierarchy and states. Do not add a second palette, a new font, or one-off hex values beside the system. If a token you need is missing, add it to the system in the same style rather than hard-coding it.
- If the user asked for a redesign of the system itself, say which tokens you are changing and why.

Only when nothing exists do you pick everything below from scratch.

## 1. Pick a direction before code

Write down, in two or three lines, before touching markup:

- **Who it is for and what one thing they should do or feel.** ("Ops engineers scanning for failures. Find the red row in two seconds.")
- **A direction in a few concrete words**, drawn from a real-world reference rather than "modern and clean". Examples: "printed train timetable: dense, monospaced numerals, hairline rules"; "museum wall label: lots of air, one serif, small caps captions"; "field equipment: high contrast, chunky controls, safety-orange accent"; "newspaper weekend section: big serif headline, columns, pull quotes".
- **One signature element** the page will be remembered by: an unusual headline treatment, a strong grid, a single bold colour field, a distinctive illustration style, data shown as the hero. One, not five.

Then let the direction decide the details. Dense tool → small type, tight spacing, rules instead of cards. Editorial → large type, generous measure, few colours. If the direction does not change any default, it is not a direction.

Avoid the template tells unless the direction truly calls for them: purple-to-blue gradients, glassmorphism cards on a blurred blob, three identical feature cards with icons in circles, centred everything, emoji as icons, stock "people at laptops" photos, shadows on every box.

## 2. Type

Type does most of the work. Get it right and a plain layout looks designed.

- **Scale:** pick a ratio and derive sizes, don't eyeball each one. Dense UI: ratio ~1.125–1.2 from a 14–15px base. Content/marketing: ~1.25–1.333 from 16–18px. Display headlines can jump well beyond the scale; that jump is often the signature.
- **Fluid sizes** for headlines: `font-size: clamp(2.25rem, 1.2rem + 4vw, 4.5rem)`. Body text stays fixed (16px minimum on phones, so iOS does not zoom inputs).
- **Pairing:** at most two families. Either one family in several weights (safest), or a characterful display face + a quiet text face. Contrast the pair on one axis (serif vs sans, geometric vs humanist), not all of them. Monospace is a valid third only for code or numerals.
- **Line length:** 45–75 characters for reading text (`max-width: 65ch`). Line height ~1.5 for body, 1.0–1.15 for big headlines. Tighten letter-spacing slightly on large display sizes (−0.01 to −0.03em); loosen on small caps and tiny labels (+0.04 to +0.08em).
- **Numbers:** use `font-variant-numeric: tabular-nums` in tables, prices and counters so columns align.
- **Hierarchy with fewer levers:** usually size + weight + colour is enough. If you need underline, caps, italic and a colour to distinguish a heading, the scale is wrong.
- Load fonts with `font-display: swap` and give a fallback stack with similar metrics. Self-host or use the project's existing font pipeline.

## 3. Spacing and layout

- **One spacing scale**, e.g. 4, 8, 12, 16, 24, 32, 48, 64, 96, 128 (px or rem), exposed as tokens. Every margin, padding and gap comes from it.
- **Proximity carries meaning:** space inside a group is smaller than space between groups. A common failure is uniform 24px everywhere, which makes nothing belong to anything. Aim for at least a 2x jump between related and unrelated.
- **Grid:** decide a content width (e.g. 1120–1280px for apps, 65ch for reading) and a column structure. Align edges to it. Asymmetry is fine and often better than centring; misalignment by a few pixels is not.
- **Hierarchy:** squint at the screenshot. There should be one obvious first thing, a clear second, and then the rest. If three things shout, demote two.
- **Containers:** not everything needs a card. Group with space, a hairline rule, or a background tint first; use borders or shadows only when an element floats or is interactive.
- **Radius:** pick one or two values and use them consistently. Nested radii: inner = outer − padding.

## 4. Colour as tokens, with dark mode

Define semantic tokens, never sprinkle raw hex in components:

```css
:root {
  --bg: #f7f6f3;        --surface: #ffffff;   --surface-2: #efeee9;
  --text: #1b1b1f;      --text-muted: #5e5e66; --border: #dcdad3;
  --accent: #c2410c;    --accent-text: #ffffff;
  --ok: #15803d; --warn: #a16207; --danger: #b91c1c;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #111113; --surface: #1a1a1d; --surface-2: #232327;
    --text: #ececef; --text-muted: #a0a0a8; --border: #2e2e33;
    --accent: #fb923c; --accent-text: #1b1b1f;
  }
}
:root[data-theme="dark"] { /* same dark values, for a manual toggle */ }
body { background: var(--bg); color: var(--text); }
```

- **Mostly neutrals, one accent.** The accent marks what is actionable or important. If it is everywhere, it marks nothing. Status colours are for status only.
- **Tinted neutrals** (a hint of warm or cool) look more considered than pure grey; keep the tint consistent across the ramp.
- **Dark mode is its own palette**, not an inversion: lift surfaces by lightness instead of shadows, desaturate and lighten the accent a little, avoid pure black backgrounds and pure white text on large areas.
- **Contrast:** body text ≥ 4.5:1 against its background, large text (≥24px, or ≥19px bold) and UI component edges ≥ 3:1. Check muted text and placeholder text specifically, they fail most often. Check both themes.

## 5. Detail density and imagery

Finished pages have small, consistent details; templated ones have big generic ones.

- **Details that read as care:** consistent icon set and stroke width; focus rings that match the accent; hover and pressed states; hairline dividers aligned to the grid; real-looking sample data (names, dates, amounts) instead of "Lorem ipsum" and "John Doe"; aligned numerals; sentence-case labels.
- **Imagery:** choose one treatment and keep it: product screenshots in a consistent frame, a single illustration style, abstract shapes built from the palette, or photography with a consistent crop and grade. If you have no real imagery, prefer typography and layout over placeholder stock. Generated SVG patterns or simple geometric compositions in the token colours are a good fallback. Always set `width`/`height` or `aspect-ratio` so images do not shift layout, and write real `alt` text.
- **Icons** support labels; they rarely replace them. Icon-only buttons need `aria-label`.

## 6. States are part of the design

Design these with the same care as the happy path, and check each in a screenshot:

- **Empty:** says what this area is for and offers the first action. Not just "No data".
- **Loading:** skeletons shaped like the real content for layout-heavy areas; a small inline spinner or progress text for quick actions. Never a layout jump when content arrives.
- **Error:** says what happened in plain words, what the user can do, and keeps their input. Use the danger colour sparingly (icon + text), not a full red page.
- **Long and short content:** a 3-character name and a 60-character name, one row and 500 rows, a missing avatar.
- **Interactive states:** hover, focus-visible, active, disabled, selected. Disabled still needs readable text (≥ 3:1 is a reasonable floor for legibility even though WCAG exempts it).

To screenshot a state, add a temporary query flag or story that forces it, or point the script at a static fixture.

## 7. Phone width

- Design at 390px wide as a first-class layout, not a squeezed desktop. Minimum side gutter 16px.
- No horizontal page scroll. Tables either scroll inside their own container or collapse into stacked rows. Long words and URLs get `overflow-wrap: anywhere`.
- Touch targets ≥ 44×44px. Primary actions within thumb reach; sticky bottom bars are fine for one main action.
- Navigation collapses deliberately (a menu, a tab bar, or a short horizontal scroller), not by wrapping into three lines.
- Respect safe areas: `padding-bottom: env(safe-area-inset-bottom)` on fixed bottom bars.

## 8. The review loop (do not skip)

You cannot judge a design from the code. Render it and look.

**Capture.** With the page running (dev server URL or a local HTML file):

```bash
node ${SKILL_DIR}/scripts/shot.mjs --url http://localhost:3000 --sheet --dark --out /tmp/review/sheet.png
node ${SKILL_DIR}/scripts/shot.mjs --url http://localhost:3000 --out /tmp/review/desktop.png          # 1440x900
node ${SKILL_DIR}/scripts/shot.mjs --url http://localhost:3000 --phone --out /tmp/review/phone.png    # 390x844 @2x
node ${SKILL_DIR}/scripts/shot.mjs --url ./index.html --full --w 1280 --h 800 --out /tmp/review/full.png
```

`--sheet` puts desktop and phone side by side (light row, plus a dark row with `--dark`). The desktop cell is scaled down, so judge text size from the single desktop shot. The script uses the installed Chrome (or `$CHROME`), installs `playwright-core` into a cache on first run, and prints page errors, failed requests and **horizontal overflow** with the offending elements. Treat any of those as a bug to fix first.

**Look at the image** (open the PNG with your file/image reading tool). Then critique it like an art director, out loud, against this list:

1. Where does the eye land first? Is that the right thing?
2. Does it match the direction you wrote down, or has it drifted back to defaults?
3. Spacing: are groups obvious? Anything cramped, anything floating?
4. Type: too many sizes or weights? Line lengths too long? Headline wrapping badly (one orphan word)?
5. Alignment: edges that almost line up but do not.
6. Colour: accent overused? Muted text too faint? Dark mode muddy or glaring?
7. Phone: anything clipped, overflowing, too small to tap, or a desktop layout merely shrunk?
8. Does anything look like a placeholder (fake text, grey boxes, broken images)?

**Fix the three worst issues**, biggest visual impact first, then capture again and compare. Repeat until a fresh critique finds nothing worse than nitpicks, usually two to four rounds. Say what you changed each round in one line.

Report to the user with the final screenshots' paths, not just "done".

## Quick checklist before you finish

- [ ] Existing tokens/components used; no stray hex values or one-off fonts
- [ ] Direction written down and visible in the result
- [ ] One type scale, ≤ 2 families, readable line length
- [ ] One spacing scale; groups distinguishable
- [ ] Light and dark both checked; contrast ≥ 4.5:1 for text
- [ ] Empty, loading and error states designed and screenshotted
- [ ] Phone 390px: no overflow, 44px targets, 16px gutters
- [ ] Focus-visible styles present; reduced motion respected for any animation
- [ ] Final screenshots reviewed and paths reported
