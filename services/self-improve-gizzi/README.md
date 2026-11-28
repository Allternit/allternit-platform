# allternit-self-improve-gizzi

`harness-improve` is a repo tool. It tries small edits to Gizzi's prompts,
tool texts and turn limits, and keeps an edit only when it scores better on
your own saved test cases. It runs on your own brain, from a platform
checkout. It is built on `allternit-self-improve`.

It never merges. It never marks a PR train-ready. With `--open-pr` it opens
a draft PR, and that flag is off by default.

## What it may edit

Only these files are copied into the harness view. Nothing else from the
repo is in it, so the suite, the graders and the tests can never be edited.

- `cmd/gizzi-code/src/runtime/session/prompt/*.txt`
- `cmd/gizzi-code/src/runtime/tools/builtins/*.txt`
- `cmd/gizzi-code/src/runtime/session/limit.ts`
- `cmd/gizzi-code/src/runtime/session/guardrail-defaults.ts`

## When a patch is valid

A patch passes only if all of these hold:

1. It names no protected path (the suite, the graders, the tests).
2. Every file it touches is on the list above, and already exists.
3. The two `.ts` files change only in numbers, only on these lines:
   - `limit.ts`: `DEFAULT_WARN_AT` and `DEFAULT_LAND_AT` stay in 0.5 to
     0.99, and warn is not above land. `DEFAULT_WRAP_UP_STEPS` stays a whole
     number from 1 to 10.
   - `guardrail-defaults.ts`: each `TURN_LIMIT_DEFAULTS` value stays between
     half and double its current value.
4. Added prompt lines name no vendor, and each prompt stays under its byte
   cap.
5. The 5 guard tests pass in a copy of the repo with the patch applied.

Checks 1 to 4 run first, with no child process. The guard test command is
`GUARD_TEST_CMD` in `src/validity.rs`.

## The flow

1. Copy the editable files into a harness view (a temp folder).
2. Build a brief from the failed dev cases in the baseline report: title,
   label, note and grader details. The brief never has the hidden tests, the
   check files or the holdout cases.
3. The proposer runs the installed `gizzi run --format json --dir <copy>` on
   a copy of the view. Gizzi has no flag to load an agent prompt from a file,
   so the editor prompt (`prompts/harness-editor.txt`) is put at the top of
   the message.
4. The change becomes a repo-relative patch. The validity checks run (see
   above), then the guard tests in a candidate checkout. The guard tests stop
   after 10 minutes.
5. The candidate's Gizzi runs the protected suite:
   `eval run --mode live --split dev --fraction <f>` first, then the full
   gate, `--split all`. Cases already run in the staged pass are not run
   again (the full pass passes `--skip-case <id>` for each).
6. The accept rule runs.
7. The patch, `report.md` and a lineage record are written. With
   `--open-pr` a draft PR is opened.

## Scores and the accept rule

Per case, against the baseline report (`--baseline <report.json>`; the first
live baseline is made by hand). The baseline cases need a `split` field
(`dev` or `holdout`) for the brief.

- pass: +1
- fail that also failed in the baseline: +0.5
- fail that passed in the baseline (a regression): -1

The staged gate needs every score above 0, so one regression stops the run
before the full pass. A patch is accepted only when all of these hold:

- the gate is valid;
- the suite pass rate is no lower than the baseline minus 2 points, and no
  high-stakes case failed (the logic of `scripts/agency-eval-gate.mjs`);
- at least one more case passes than in the baseline.

## Price first

Before every run the tool calls `eval estimate`, prints the price, and asks
`run it? [y/N]`. `--yes` skips the question, not the price line. It adds up
the `cost` of each report. `--max-usd` is a hard stop: a run that would pass
it does not start, and a run that passed it ends the loop with an error.

## Outputs

- `<out>/patch.diff` and `<out>/report.md`. With no `--out`, they go in
  `.harness-improve/out/<id>/` under the repo. `report.md` has the gate
  scores per case, the price paid, the validity reasons and the decision.
- A lineage record in `.harness-improve/lineage/` under the repo.
- With `--open-pr` (off by default): a branch, a commit of the allowlisted
  files only, and `gh pr create --draft`. The tool never merges a PR and
  never runs `train ready`. A person does both.

## Dry run

`--dry-run`, or env `HARNESS_IMPROVE_FAKE=1`, uses a fake proposer and a fake
evaluator. `harness-improve run --dry-run` writes a patch, a report and a
lineage record with no network, no price and no guard tests.

## Commands

```
harness-improve propose                 # one candidate patch
harness-improve gate --patch <file>     # check one patch, then score it
harness-improve run                     # propose and gate, --max-generations rounds
```

Flags: `--repo` (default: the current folder), `--suite-dir`,
`--max-generations` (default 1), `--max-usd` (a hard stop), `--baseline`,
`--out`, `--fraction`
(default 0.3), `--seed`, `--dry-run`, `--yes`, `--open-pr`, `--brain`,
`--model`.

`gate --patch <file> --dry-run` runs checks 1 to 4 only.

## Tests

```
cargo test -p allternit-self-improve-gizzi
```
