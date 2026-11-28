# Shared Gizzi suite

`shared.v1` is the starting set of eval cases for Gizzi. It has 13 cases.

- 10 workspace tasks, ported from the subagent evals in `../subagents/evals`. Each one gives Gizzi a prompt and a small project, then checks the files it leaves behind.
- 3 session cases with hand-written traces: one good run, one bad run, and one stopped by a guardrail. They give replay data without a live run.

## Dev and holdout

Tasks 1 to 7 are `dev`. Use them while you change prompts, tools or settings.
Tasks 8 to 10 are `holdout`. Run them to confirm a change helps, and do not tune against them.
The three session cases are `dev`.

## How a task is scored

Each workspace task runs 3 times and passes when 2 of the 3 runs pass.
A run passes when it finishes with status `completed` and every file check passes.
Hidden test files are carried inside the `tests` check. They are written after the run, so Gizzi never sees them.
Workspace tasks have no recorded run, so `recorded` is null.

## Regenerate

Run from the repo root:

```
python3 -I domains/agent-evals/gizzi/port_tasks.py
python3 -I domains/agent-evals/gizzi/port_tasks.py --check
```

The first command rewrites `suites/shared.v1/`. The second exits 1 if any committed file differs from a fresh run.
Output is byte-identical on every run. Do not edit the generated files by hand. Change `port_tasks.py` or the subagent tasks, then regenerate.
