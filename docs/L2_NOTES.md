# WP-L2 notes — S1 decision banks, Laya fine-tune, Q26

Date: 2026-10-02. Branch: `prog-l2-label-harvest` (worktree `allternit-wt-p-l2`).

- **PR:** https://github.com/Gizziio/allternit-platform/pull/1183 (DRAFT — do not merge)
- **Checkpoint:** `~/.allternit/system-one/ckpt-l2`, revision `ft-98d3cf16ed993a5a` (base `convaiinnovations/laya` rev `55cf4c4`, laya 0.3.22). **Not** swapped into the Desktop config — `setCheckpoint` untouched, nothing serves live, everything stays in shadow (Q26 canary never enabled; no canary state written).
- **Export:** `~/.allternit/system-one/export-l2` (43,059 labelled rows; audit slice 2%, the Q26-permitted 2–5%; `--max-options 18` for the 18-class bank; provider scores >16-option menus coarse-to-fine, export default stays 16).
- **Q26 report:** `~/.allternit/system-one/ckpt-l2/q26-report.json`.

## Per-bank counts (label provenance × Q26 split, from the export)

| bank | observed | backfill | teacher | train | tune | cert | audit |
|---|---|---|---|---|---|---|---|
| bank.route.v0 | 9 | 4,882 | 0 | 2,403 | 960 | 1,440 | 88 |
| bank.route_model.v0 | 4 | 1,652 | 0 | 820 | 327 | 491 | 18 |
| permission.commrails_judge | 36 | 12,216 | 0 | 6,010 | 2,404 | 3,606 | 232 |
| permission.cli_guard | 36 | 12,215 | 0 | 6,010 | 2,403 | 3,605 | 233 |
| judge.first_pass.tool | 37 | 9,932 | 0 | 4,899 | 1,959 | 2,938 | 173 |
| judge.first_pass.node | 1 | 271 | 0 | 135 | 53 | 80 | 4 |
| dec.classify_error | 1 | 1,734 | 0 | 851 | 340 | 509 | 34 |
| bank.vendor_consequential.v0 | 8 | 21 | 0 | 16 | 5 | 8 | 0 |
| lessons.triage.task_success / reusable_pattern / supported_by_events | 1 (wire smoke) | 0 | 0 | 1 each | 0 | 0 | 0 |
| memory.relation | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

(One extra `vendor.consequential` train row is the pre-fix wire-smoke row in the live shadow ledger; the 22 stray pre-fix harvest rows under that primitive were excised from the harvest ledger, 22 decisions + 22 outcomes.)

## Which banks met the Q26 sample minimums (≥300 tune + ≥500 cert, real labels only)

- **Met:** `bank.route.v0` (960/1,440), `judge.first_pass.tool` (1,959/2,938), `dec.classify_error` (340/509), `permission.commrails_judge` (2,404/3,606), `permission.cli_guard` (2,403/3,605) — the two permission banks meet the counts but are in the never-auto-act class by design.
- **Did not meet:** `bank.route_model.v0` (cert 491, 9 short — local history exhausted), `judge.first_pass.node` (53/80, true-only labels), `bank.vendor_consequential.v0` (5/8), `lessons.triage.*` (0 — no lessons-triage drafts exist on this Mac), `memory.relation` (0 — `memory_facts`/`memory_relationships` empty, observations untyped, recall logs carry no answer text).

## Fine-tune

- `run-finetune.sh export-l2 ckpt-l2`, MPS, head-only (26.5M trainable params), 21,148 train rows, batch 16, lr 2e-4, seed 1337, 380 steps, stopped at the 20-min wall after epoch 1/3 (train loss 0.473). Checkpoint reloaded from disk and scored on the held-out splits: scored-tune 8,451 rows, scored-cert 12,678 rows, both bound to revision `ft-98d3cf16ed993a5a`.

## Q26 results (fine-tuned checkpoint; δ=0.05, ε=5% default)

| bank | tune/cert | τ | outcome |
|---|---|---|---|
| bank.route.v0 | 960/1,440 | 0.50 | **FAIL** — split-B auto-act error CP upper bound 0.1515 > ε (coverage 0.69; secondary ECE upper 0.083 reported, non-gating); class `retrieval` auto-act error 1.0 > floor |
| judge.first_pass.tool | 1,959/2,937 | 0.50 | **FAIL** — error bound 0.0327 ≤ 0.05 at full coverage, but per-class floor: predicted-`false` auto-act error 1.0 > 0.1 (the 37 human-denied calls are not separable from 9.9k benign ones yet) |
| dec.classify_error | 340/509 | — | **FAIL** — no τ on split A keeps the CP upper bound ≤ 0.05 (18-class, one epoch of training) |
| bank.route_model.v0 | 327/491 | — | **FAIL** — cert n=491 < 500 |
| judge.first_pass.node | 53/79 | — | **FAIL** — sample minimums |
| permission.commrails_judge / permission.cli_guard | 2,404/3,606 · 2,403/3,605 | — | **ineligible by design** — permission/money/client family never auto-acts (S1 may only tighten) |
| bank.vendor_consequential.v0 | 5/8 | — | **ineligible by design** — never auto-act (tighten-only; consequential name class) |

Base-checkpoint rehearsal (export splits, pre-fine-tune) failed at the τ-selection step for every eligible bank. No binding passed → **no manifest written, nothing appended to `ALLTERNIT_S1_MANIFESTS`, no checkpoint swap.** Per Q26, a passing checkpoint may only be promoted through `SystemOneManager.setCheckpoint({path})` + canary; that path was not exercised because nothing passed.

## Teacher spend

**$0.** No teacher labels were produced or used. Every label is `observed` (a human action: permission denial, draft apply/reject — none exist locally, PR-merge verification is deterministic git) or `backfill_observed` (a deterministic rule ported verbatim from the live labeler: `routeLabel`, `routeModelLabel`, `s0_error_code`, the permission verdict join; or the harness's own no-ask flag for vendor consequential). All model readouts came from the local Laya on :7718 through the S1 router; no vendor model API was called for labelling.

## Test results

`cd tools/system-one-local && bun test`: **230 pass / 0 fail** across 9 files (22 harvest tests: redaction of secrets/emails in every new state shape, provenance, idempotent re-runs, per-source parsers on fixtures, bank-spec verbatim checks against the live callers, s0_error_code parity).

Harvest runs (all idempotent; decision_id = hash(bank, primitive, question, source, key)):

1. 2026-10-01 evening, resumed run: 9,389 replayed, 0 failed, 19,187 skipped-as-existing (a prior session had extended the ledger after the first-pass commit).
2. 2026-10-01 late: vendor-primitive fix re-run under the corrected primitive `bank.vendor_consequential.v0` (14 then 8 rows).
3. Mid-run incident: the Desktop's Laya (:7718) died and was restarted by the Desktop during the first resume run — 6,937 items failed with "laya unreachable", all recovered by the idempotent re-run. A second run was killed by a task timeout during source enumeration; also recovered by re-run.
4. Wire smoke: one request per new bank through the real `POST /v1/decision` on the Desktop's :7717 + `/v1/decision/outcome` (decision ids `7a4a02b1…`, `19bf7539…`, `c9b3b9ef…`, `cc0ccfd9…`, `33625a04…`, `7eb37ae3…`, `09dbe84d…` in the live shadow ledger).

## Open items

1. **Nothing auto-acts.** Next lever: full 3-epoch training (the 20-min wall stopped after epoch 1) and/or `--unfreeze-last N`, then re-run Q26 on the same untouched cert split. The label inventory is idempotently extensible — re-harvest only appends genuinely new history.
2. `judge.first_pass.tool` needs the rare class to separate (more human-denied examples, or down-weight bypass-mode rows further) before its per-class floor can pass; its overall error bound already fits.
3. `bank.route.v0` mixes gizzi turn-router and vendor-gateway instructions under one binding (same bank/option set; the instruction text differs per caller by design).
4. `bank.route_model.v0` cert is 9 rows under 500 — history exhausted locally (usage ledgers are near-empty; gizzi turn history fully harvested).
5. Zero local rows for `lessons.triage.*` (CommRails vault/triage pipeline never ran on this Mac — the brain-drafts source only accepts lessons-triage-format drafts with `x_commrails.candidate_id`; all 211 drafts in the Brain are from other producers), `memory.type`/`memory.relation` (memory write path produced no facts/edges here; MEMORY_TYPE's live op LABEL isn't servable by this runtime), `computer_use.*` (only synthetic test-run events locally), and agency `goal_satisfied` (no agency runs in the DB). The sources are in place and will produce rows when those subsystems run here.
6. `judge.first_pass.node` carries true-only labels on this Mac (242 PR-verified merges; 1 referenced PR still open; 0 closed-unmerged — no verifiable false class).
