# Lean agents: building a film without huge contexts

A film agent that does everything in one context (write, audio, every chapter, renders, fixes) grows past 300k tokens.
**The film lives on disk** (script, VO, chapters, frames, exports), so a context can always be thrown away. Every agent
works from files and leaves files. If you can't spawn subagents, do the same thing yourself across turns: finish a stage,
write HANDOFF.md, and keep only that in mind.

## Split the film into short, fresh stages (never one agent for a whole film)
| Stage | Who | Reads (only this) | Leaves on disk | Budget |
|---|---|---|---|---|
| A. Write + audio | 1 agent | SKILL.md, the approved CONCEPT.md, `references/STORYBOARD-example.md` | `anim/STORYBOARD.md`, `audio/vo-lines.json`, `audio/vo/*`, `src/film.js` (DUR_VO, VO, SHOT_T), `assets/mix.wav`, optional `src/shorts.js` entries, `HANDOFF.md` | ~80k tokens, ~40 tool calls |
| B. Chapters | 1 agent **per chapter** (2-4 at once) | `references/CHAPTER-BRIEF.md` (filled in), **its own** STORYBOARD rows, the shared rig/props files | `anim/src/ch/chN_*.js`, one small check sheet, a HANDOFF line with its hit times | ~80k each, ~30 tool calls |
| C. Finish | **no agent**: `${SKILL_DIR}/scripts/finish_film.sh <film dir>` | - | master, preview, vertical cuts, contact sheets | 0 tokens |
| D. Fix pass | 1 agent, only if the contact sheet shows defects | `reviews/contact-*.jpg`, the defect list | fixed chapters, a re-run of stage C | ~60k |

The orchestrator (the main session) reads only the HANDOFF files and the contact sheets, never the agents' transcripts.

## Rules that keep a context small
- **Never `cat` a whole chapter or the guide twice.** Use `grep -n` and `sed -n 'a,bp'` for the lines you need.
- **Redirect render, ffmpeg and TTS output to a log file.** Read logs with `tail -3`; one render prints thousands of progress lines.
- **Keep check images small.** At most 8 frames at <= 1200 px wide per sheet. Look once, then fix by frame time.
- **Write `HANDOFF.md` in the film folder after every stage:** what's done, what's next, the exact commands. A new agent
  resumes from it, so a dropped connection or a usage limit costs nothing.
- **Stop at the budget.** At ~150k tokens or the tool-call cap, write HANDOFF.md and end. A fresh agent continues cheaper.
- **Pick the model for the job.** Writing and storyboarding need the strongest model; chapter fixes and mechanical steps can use a cheaper one.
- **Don't re-render whole films.** Delete only the changed frames (renders resume), then re-run `finish_film.sh` with `FORCE=1`.
- **One render at a time.** `finish_film.sh` takes a machine-wide lock (`~/.cache/brush-film-render.lock`) and waits its turn.
