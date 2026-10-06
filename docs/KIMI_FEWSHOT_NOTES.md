---
status: done
date: 2026-09-18
branch: session/kimi-fewshot
files_changed:
  - domains/computer-use/core/core/kimi_fewshot.py
  - domains/computer-use/core/core/tier_a_traces.py
  - domains/computer-use/core/core/decision_head.py
  - domains/computer-use/core/core/shadow_eval.py
  - domains/computer-use/core/scripts/shadow_head_eval.py
  - domains/computer-use/core/scripts/tier_a_make_traces.py
  - domains/computer-use/core/tests/test_kimi_fewshot.py
  - domains/computer-use/core/evaluation/tier-a/traces.jsonl
  - domains/computer-use/core/evaluation/shadow-eval/shadow-eval-report-kimi-fewshot8.json
  - domains/computer-use/core/evaluation/shadow-eval/shadow-eval-report-kimi-fewshot8.md
  - domains/computer-use/core/evaluation/shadow-eval/shadow-eval-report-kimi-fewshot8-random-task-neutral.json
  - domains/computer-use/core/evaluation/shadow-eval/shadow-eval-report-kimi-fewshot8-random-task-neutral.md
  - domains/computer-use/core/evaluation/shadow-eval/shadow-eval-report-kimi-fewshot9-interleaved-default.json
  - domains/computer-use/core/evaluation/shadow-eval/shadow-eval-report-kimi-fewshot9-interleaved-default.md
  - docs/KIMI_FEWSHOT_NOTES.md
deviations:
  - "JEV_KIMI_HEAD_NOTES.md was expected at docs/ but lives at docs/learnings/JEV_KIMI_HEAD_NOTES.md; this notes file follows the done-criteria path docs/KIMI_FEWSHOT_NOTES.md."
  - "the Factory CLI is not on PATH in this environment, so no workspace DAG plan was created; progress is tracked in .steering/checkpoint.md instead (repo rule noted, pragmatic substitution)."
  - "4 ruff findings in touched files (unused imports in decision_head.py/shadow_eval.py, an f-string without placeholder in shadow_head_eval.py) pre-exist on origin/main and were left untouched per 'do not silently fix unrelated files'."
  - "evaluation/tier-a/traces.jsonl is covered by the repo-root *.jsonl gitignore; force-added because the make-traces docstring designates it as a committed experiment record and a unit test consumes it."
remaining:
  - "Hypothesis outcome: few-shot in-context distillation at N=8-9 did not break the per-task constant-policy collapse. What a live-trace version would need is specified in the honest-read section."
---

# KimiCliHead few-shot in-context distillation — shadow policy head (2026-09-18)

The zero-shot `KimiCliHead` scores 0.3182 agreement but collapses to a
per-task constant policy (22/22 click in search-flow *including all 8 fill
steps*; 21/22 fill in form-fill; 22/22 click in settings-toggle). This
session tests the hypothesis that **few-shot in-context distillation from
labelled Tier A traces breaks the collapse** — "training" is prompt-space
only; all inference is `kimi -p` subprocesses (subscription), zero local
model training.

## Setup

- Traces: `scripts/tier_a_make_traces.py` → `evaluation/tier-a/traces.jsonl`
  (579 steps: 419 train / 94 val / 66 heldout; held-out = the 3 canonical
  eval tasks, 22 steps each). The parked trace scripts were repaired first:
  `tier_a_traces.py` moved to `core/` (it uses package-relative imports),
  `ABSTAIN_OPTION` added to `decision_head.py`, and `SyntheticTask` gained an
  optional `step_trees` field honored by `_step_trees` (dynamic-observation
  variants need their scripted per-step trees, not derived ones).
- Few-shot plumbing: `core/kimi_fewshot.py` selects exemplars from the TRAIN
  split only; held-out task exclusion is a hard invariant **asserted in
  code** (filtered + raised-on-leak), covered by unit tests. Each exemplar =
  short state excerpt (task line + observed-element rows, ≤700 chars) →
  canonical gold operation (+ target index + element name when the recorded
  op's target menu was offered). `KimiCliHead(few_shot_block=...)` injects
  the rendered block before `[STATE]`; the strict JSON answer contract is
  unchanged (22 unit tests, subprocess stubbed; targeted suite 87 passed,
  1 skipped).
- Eval: `scripts/shadow_head_eval.py --head kimi --few-shot N
  --few-shot-order {random,interleaved} --few-shot-framing {default,task-neutral}`
  — 66 held-out steps per pass, batched questioning. Baseline 0.3182 was
  NOT re-run (known from JEV_KIMI_HEAD_NOTES.md).

## The numbers (3 tasks × 22 decide steps = 66; per-task collapse anatomy)

| Pass | Config | Agreement | vs 0.3182 baseline | search-flow (8 fill steps) | form-fill | settings-toggle | Mean latency/step | Vocab misses |
|---|---|---|---|---|---|---|---|---|
| Baseline | zero-shot | 0.3182 (21/66) | — | 0.3182 — policy click 22/22, fill 0/8 | 0.3182 — fill 21/22 | 0.3182 — click 22/22 | 27 903 ms | 1 |
| **1** | N=8, random order, default framing | **0.3333 (22/66)** | +1 step (noise) | 0.3182 — click 22/22, **fill 0/8** | 0.3636 — fill 22/22 | 0.3182 — click 22/22 | 20 504 ms | 5 (all `hover_target` key omissions) |
| **2** | N=8, random, **task-neutral framing** | **0.3030 (20/66)** | −1 step (**below baseline**) | 0.3182 — click 22/22, **fill 0/8** | 0.3636 — fill 20/22 + 2 clicks | 0.2273 — click 22/22 | 17 495 ms | 2 |
| **3** | N=9, **interleaved** by template, default framing | **0.2879 (19/66)** | −2 steps (below baseline) | 0.2727 — click 19/22 + fill 3/22, **fill steps still 0/8** | 0.2273 — mixed policy (10 fill/12 click) | 0.3636 — click 22/22 | 20 719 ms | 3 |

Per-task collapse check (the mission's key question): the per-task constant
policy was **unbroken in passes 1–2** (search-flow click 22/22, fill 0/8 in
both). Pass 3's interleaving did crack the constant policy — search-flow
produced 3 fill choices and form-fill flipped 10 steps to click — but the
departures were mostly the *wrong* operations, so aggregate agreement went
down, not up. In **no pass** did search-flow's 8 fill steps score a single
correct operation: the head never once chose `fill` correctly on a
search-flow step, exactly like zero-shot. The small aggregate wobble
(±1–2 steps of 66) elsewhere is target-selection noise on click steps, not
operation recovery.

## Pass-by-pass justification

- **Pass 1 (random, default):** the straight test of the hypothesis. Result:
  flat. The model reads the exemplars but still answers from task identity.
- **Pass 2 (task-neutral framing):** chosen because v1 showed the model
  anchors on the task line despite the soft "sites DIFFER" intro; the direct
  counter is an explicit anti-collapse preamble ("NO fixed per-site policy;
  decide from the CURRENT state only"). Result: worse than baseline — louder
  prose does not change the anchoring. (Variant (b), state-delta emphasis,
  was rejected as not implementable: state text is harness-produced and
  changing it would change the eval itself.)
- **Pass 3 (interleaved, N=9):** iterating on the best pass (v1). v1's random
  sample was 6/8 search exemplars — the exemplar window itself was
  click-anchored; interleaving round-robins templates so consecutive examples
  always come from different site types, with N=9 giving exactly 3 per
  template. This changes exemplar *composition in the window* rather than
  rewording instructions (which failed twice). Result: 0.2879 — the one pass
  that broke the per-task constant policy (mixed head policies on search-flow
  and form-fill) got **worse**, because the policy it left for was wrong more
  often than the constant one. Being less collapsed is not the same as being
  more correct.

## Latency and vocab misses

Few-shot prompting got *faster* per step (27.9s → 17.5–20.5s mean) — the
exemplars give the model a pattern to complete, shortening generation. All
misses across passes are `hover_target` speculative-menu key omissions (kimi
drops one optional key; JSON otherwise parses) — the strict contract held,
zero crashes, zero out-of-vocab operations in any pass.

## Honest read: does few-shot in-context distillation work?

**No — not at this scale, against this collapse.** Three passes spanning
ordering, framing, and exemplar composition all land within ±2 steps of the
0.3182 zero-shot baseline, and the diagnostic that matters — 0/8 → correct
on search-flow fill steps — never moved. The collapse is not a prompt-wording
problem: the model demonstrably understands the task and the format (targets
on click steps track at 0.50–0.62, well above chance), but its operation
choice is a task-identity lookup that exemplars from *other* sites do not
interrupt. In-context distillation needs the demonstration to come from the
**same task's own trajectory** to override the prior — which is precisely
what a live-trace head gets and a static prompt cannot.

What the live-trace version would need (beyond this session's budget):
1. **Same-task history**: inject the current task's *own* preceding
   state→decision pairs (from the harness's recorded/verified steps) instead
   of foreign-task exemplars. The first 1–2 cycles of each task establish
   fill→click→click locally; a head that sees "this task, step 1 was fill"
   has evidence against its click prior. This requires trace capture wired
   into the planning loop (the shadow.decision events already exist).
2. **State-delta conditioning**: the variant (b) that was out of scope here —
   prefix each state with an explicit "changed since last step" block
   (requires harness cooperation, not just prompt edits).
3. **More exemplars is not the answer**: N=8→9 changed nothing; the binding
   constraint is exemplar *relevance* (same task), not count.
4. If agreement must come from a static prompt alone, the remaining lever is
   few-shot **with per-step reasoning** (CoT rationales in the exemplars)
   rather than answer-only pairs — untested here, and at 28s+/step the
   latency budget argues for the live-trace design instead.

Bottom line for the shadow-head program: the kimi cloud tier remains an
iteration-speed instrument; the agreement path is unchanged from the mlx
session's conclusion — Tier A (a small classifier trained on labelled
shadow.decision traces) is the route, and these three passes add the
evidence that prompt-space distillation is a dead end for the
task-identity collapse.

## Reproduce

```bash
cd domains/computer-use/core
uv sync --frozen --extra dev && uv pip install -e '.[shadow-head]'
uv run --frozen --no-sync --extra dev python scripts/tier_a_make_traces.py
# each pass (~20-30 min; run in background, poll the log):
uv run --frozen --no-sync --extra dev python scripts/shadow_head_eval.py \
    --head kimi --steps 22 --few-shot 8 --few-shot-order random --few-shot-framing default
uv run --frozen --no-sync --extra dev python scripts/shadow_head_eval.py \
    --head kimi --steps 22 --few-shot 8 --few-shot-order random --few-shot-framing task-neutral
uv run --frozen --no-sync --extra dev python scripts/shadow_head_eval.py \
    --head kimi --steps 22 --few-shot 9 --few-shot-order interleaved --few-shot-framing default
# tests (subprocess stubbed; no real CLI call in the test path):
uv run --frozen --no-sync --extra dev pytest tests/test_kimi_fewshot.py \
    tests/test_decision_head.py tests/test_shadow_eval_smoke.py \
    tests/test_shadow_hook.py tests/test_element_table.py -q
```
