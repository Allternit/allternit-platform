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

## Commands

```
harness-improve propose                 # one candidate patch
harness-improve gate --patch <file>     # check one patch, then score it
harness-improve run                     # propose and gate, --max-generations rounds
```

Flags: `--repo` (default: the current folder), `--suite-dir`,
`--max-generations` (default 1), `--max-usd` (a hard stop), `--fraction`
(default 0.3), `--seed`, `--dry-run`, `--yes`, `--open-pr`, `--brain`,
`--model`.

`gate --patch <file> --dry-run` runs checks 1 to 4 only.

## Status

The view, the allowlist and the validity check are built. The evaluator
(step 3.3) and the proposer (step 3.4) are not built yet. Those commands
stop with a clear error until they are.

## Tests

```
cargo test -p allternit-self-improve-gizzi
```
