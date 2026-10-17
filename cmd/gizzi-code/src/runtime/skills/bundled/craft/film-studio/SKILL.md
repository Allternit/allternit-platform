---
name: film-studio
description: >-
  The production contract for any code-rendered or assembled video ("motion engineering": a studio,
  not a prompt). Director's brief with facts vs choices, a style guide written from an inspected
  reference, a state-based shot list, a seekable renderer, motion rules per object class, sound on
  the same timeline, a contact-sheet review with a scored critique that fixes the three worst
  defects, separate compositions per format, and the decisions packaged in files with gates where a
  missing real asset means stop and ask. Load it at the start of ANY video job (motion-graphics
  reels, product promos, explainers, animated shorts, social cuts) and follow its gates alongside
  the method skill (e.g. motion-reel). It does not replace the method skill; it is the review and
  packaging layer over it.
---

# Film studio: the production contract

The core idea: **BRIEF → SYSTEM → FILM → REVIEW → BETTER SYSTEM**, not PROMPT → CLIP. The model
writes the program, the program draws the frames, the renderer makes the film, and the review loop
decides whether anyone sees it. Generic output comes from missing decisions that the model fills
with safe defaults. Remove those decisions before rendering.

Use it with a method skill: **motion-reel** (canvas motion graphics), or whatever renderer the
project already uses (Remotion, p5, Lottie, an ffmpeg edit of real footage, generated clips). Two
standing rules apply to all of them:
- **The picture follows the words.** Whatever the narration or on-screen copy says at a moment is
  what the frame shows at that moment.
- **Facts come from the user, choices come from you.** You may invent a camera move. You may never
  invent a UI, a logo, a quote, a metric or a fact.

## The studio folder (every film gets one, so the next run starts from files, not from chat)
```
<film>/
  brief.md          one-sentence goal, product facts, what's allowed, what's unacceptable
  style-guide.md    palette, type, motion rules, banned defaults (written from an inspected reference)
  shotlist.md       beats with timestamps, entry state, exit state, purpose
  assets/           approved logos, real screens, photos (with licence), voices, music
  src/              seekable composition code + render command
  reviews/          contact sheets + defect notes + scores, one file per pass
  exports/          one export per format, plus poster frames
```

## Gates (stop at each one, then show evidence before moving on)
1. **Brief.** Write `brief.md` before anything else. It answers five questions:
   - What is it, for whom, how long?
   - What is the ONE thing the viewer remembers?
   - Which reference may influence the look?
   - Which elements must come from the real product or real material (logo, screens, quotes, people)?
   - What would make it unacceptable?

   Keep **facts** (the product, its screens, the logo, real quotes, numbers) separate from
   **choices** (camera, pacing, jokes, transitions). If the user hasn't supplied the facts, ask.
2. **Reference → style guide.** Inspect a real reference: a frame, a film, the user's previous
   work, their site or design tokens. Write down what you learned as rules, not a collage. If a
   required asset is missing (the real wordmark, a real screen, a licensed photo or track),
   **stop and ask**, or use a clearly labelled placeholder. Never draw a convincing fake.
3. **States and beats.** Every beat has an entry state, an exit state and a reason to exist
   ("what does the viewer know now?"). If you can't say why a shot exists, cut it. For UI, list
   the states before designing the transitions.
4. **Renderer on purpose.** Use the smallest system that supports a film you'll make again.
   Seekability is non-negotiable:
   - every frame is a pure function of t;
   - no hidden clocks (`Date`, accumulated rAF state);
   - seeded randomness only;
   - nothing that only works after the page has run for a while.
5. **Motion with weight.** Write one motion rule per object class:
   - small controls snap;
   - big panels settle;
   - the camera moves slowly enough to keep the viewer oriented;
   - headlines arrive fast and hold long enough to read (about 0.25 s per word after landing).

   Use springs on purpose, not everywhere: if everything overshoots, nothing reads as precise.
   Check after each move that the viewer knows where to look.
6. **Sound on the same timeline.** VO, music hits and SFX cues come from the same timestamps
   that drive the picture (one cue list, read by both the renderer and the mix script).
   - Cut against measured beats.
   - Never loop a bed so you can hear the seam. Generate or supply one long enough, and end on
     its sting.
   - The eyes-closed test: listen to the film. If it has no shape without the picture, the sound
     isn't finished. Render a waveform and look at it.
7. **Contact sheet before the full render.** Render one frame at every beat plus a strip around
   each fast transition, tile them, and actually look at the image. Look for:
   - empty or blank frames, or a big empty half of the frame;
   - graphics bigger than their cards, text overflow, text under the HUD or safe margins;
   - covered faces, wrong z-order, crowded characters;
   - effects that never switch off (a glitch or flash still visible later);
   - fallback fonts, wrong colours, anything off-brand;
   - consecutive scenes that look the same.

   Then **score** hook, readability, continuity, motion and sound sync (1–5 each), fix the
   **three worst defects**, and re-render only those sections. Save the sheet and scores in
   `reviews/`. The job is done when the evidence says the film works, not when the encoder exits.
8. **Variants from the source, never a crop.** 16:9 and 9:16 (and 1:1 if needed) are separate
   compositions: same assets and states, different layout, type size and camera path. Make a
   contact sheet for every variant, and check each export's duration and size with ffprobe
   against the target platform's limits.
9. **Deliver.** Hand over:
   - finals and poster frames;
   - contact sheets and the review scores;
   - source and the render command;
   - the asset list with licences;
   - a short note on what still needs human review (mock UI, any illustrative numbers, claims).

   The user approves before anything is posted or sent anywhere.

## Long runs: gates, not freedom
An agent can work for hours between gates. It must not make irreversible brand decisions on its
own: the wordmark, real people's likenesses, voices, money spent on paid generation, posting.

A missing real asset is a blocker, not an invitation. An unclear reference gets a question or one
test frame, not 900 polished wrong frames. Each stage leaves evidence the next stage can check, so
a run can resume from a finished gate instead of restarting.

## The contract prompt (adapt it for subagents)
```
You are directing and engineering a code-rendered film.
Goal: a [duration]-second film for [audience]. The viewer should remember: [one sentence].
Inputs: only the supplied assets and approved copy. Inspect the reference and write style-guide.md.
        Write shotlist.md with timestamps, entry states, exit states and purpose.
Build: say which renderer and why. Every frame is seekable and deterministic. Motion, audio cues and
       transitions share one timeline. Keep the editable source and a repeatable render command.
Gates: 1) style guide + shot list before building  2) contact sheet before the full render
       3) score hook, readability, continuity, motion, sound sync  4) fix the three worst defects,
       re-render those sections  5) export each format as its own composition.
Deliver: finals, poster frames, contact sheets, source, the asset list, and a note on what still
         needs human review.
```

## Defects worth checking on every contact sheet
- A screen or panel left empty until a cue word: show the first graphic from the start of the shot.
- A photo or page drawn bigger than its card: fit inside it (contain), never cover.
- A caption or name strap over a face.
- Typed text where the brand has a real logo: use the supplied logo file, never a typed stand-in.
- A music bed looped until you hear the seam: make it full length with an open and a closing sting.
- A translucent wash or overlay on a busy background turning see-through: draw a solid card first.
- Motion-blur ghosts on small or wide-tracked text reading as doubled letters.

## Guardrails
- No on-screen claim the user can't back (results, metrics, guarantees, customer logos without
  permission). Label mock UI and illustrative numbers as such when delivering.
- No real person's likeness or voice without their documented permission. Use faceless or
  fictional stand-ins otherwise.
- No commercial music or stock assets unless the user supplies a licensed file; record every
  asset's licence in the asset list.
