# Drive: the opt-in DAG runner

Status: **implemented 2026-09-29** (`commrails/src/drive/`). Source requirements:
Raven reverse audit S10 (spawn caps, capacity admission, harmful-action
idempotency) and S9 (lease heartbeats / verifier close, integrated via hooks).

## What it is

```
allternit-commrails drive <dag_id> [--max-concurrent N] [--max-spawns-per-hour N]
                                   [--once] [--dry-run] [--workdir <dir>] [--timeout-seconds N]
```

A **foreground operator command**. `DAG_AS_DEFAULT_TASK_SYSTEM.md` keeps "no
background executor" as a non-goal: nothing starts `drive` on a schedule, from a
hook, or from the service. An operator runs it; it exits when nothing is READY
or running, on Ctrl-C, or after one pass with `--once`.

Drive adds scheduling, caps and an attempt record. Everything else is the
existing path, unchanged:

| Step | Existing mechanism |
| --- | --- |
| readiness | `ready_nodes` (blocked_by DONE, wait-gates satisfied; elapsed timers resolved lazily) |
| pickup | Gate 1 `wih_pickup_detailed` (agent `drive-<harness>`, node `owner_role` passed as role) |
| open | `wih sign-open` (signature `drive:<pid>:<executor>`) |
| prompt | the WIH's resolved description (`{{ node.output }}` placeholders already resolved by Gate 1) + a footer naming the dag/node/WIH and the lease command |
| spawn | `Orchestrator::spawn` with `wih` set — the #965 spawn gate: `admit()`, the Claude PreToolUse hook settings, `gate_argv` (no bypass flags) |
| completion | `Orchestrator::poll` (Done / Dead) + drive's own deadline (Timeout) |
| close | Gate 4 `wih_close_with(..., output)` — the #954 node output (blob + receipt + `nodes/<node>.out.md`) |

## One pass

1. **Poll own sessions.** Exit-code file present → Done; pane dead or session
   gone without it → Dead; past `timeout_seconds` → Timeout (session killed).
2. **Reconcile open attempts** (a `DriveAttemptStarted` with no finish) this
   process does not hold: exit-code file present → collect it; session alive →
   adopt it (Ctrl-C and crash recovery); otherwise → `interrupted`.
3. **Decide** for every non-terminal node (pure function, shared with
   `--dry-run`):
   - not READY with upstream DONE → print its blocking gates ("needs you" for manual);
   - READY, no executor → "pick up manually" (the plan root: "verify and close it");
   - `bot:<slug>` → mail once, never spawn;
   - `ao:<harness>` with no WIH → admission pre-check, then spawn;
   - `ao:<harness>` with a drive WIH whose last attempt ended without closing
     it → restart or needs-you (below);
   - a WIH not created by drive → "held by <assignee>".
4. **Act**: spawn within caps, add needs-you gates, send bot mail.

### Executors

- `ao:<harness>`: argv from `.allternit/drive/config.json` `harnesses.<name>.argv`
  (defaults: `claude -p {prompt}`, `codex exec {prompt}`), then the spawn gate
  rewrites it. The harness runs headless: stdin `/dev/null`, stdout → the node
  output, stderr kept for failure receipts, exit code written atomically to
  `exit_code` (the completion sentinel).
- `bot:<slug>`: drive does not run bots. It sends one typed message on thread
  `dag:<dag_id>` to `bot:<slug>` (`ack_required`) with the pickup command and
  records `DriveBotNotified`; later passes skip it. No gate is added, so the bot
  can pick the node up.

### Admission and refusal

Pickup always creates WIHs with `requires_lease_for_write: true`, so drive runs
the same `admit()` the spawn path runs **before** pickup: an ungated harness
(or one with no argv configured) gets a needs-you gate and no WIH. The spawn
path's own check still runs after pickup (a refusal there is recorded
`spawn_refused` + `HarnessSpawnRefused`). With #965's classes, only hooked
harnesses (claude) are admitted on a WIH today.

### Needs you

A node drive cannot or must not advance gets a **manual node wait-gate** with
`params.source = "drive"` plus `reason`, `executor`, and (for attempts)
`attempt_id`, and a `DriveNeedsYou` event. The gate is what makes it visible
(`wait-gate pending`, API `needsYou`) and what stops drive: an unresolved gate
keeps the node out of `ready_nodes`, so there is no retry loop.

| reason | when | resolving the gate means |
| --- | --- | --- |
| `harness_refused` / `harness_unconfigured` | pre-pickup admission failed | acknowledged: drive leaves the node for manual pickup (re-drives it only if its executor changes) |
| `interrupted` | an attempt's harness started and neither finished nor left an exit code | restart it once on the same WIH |
| `attempt_failed` | spawn failed, or `wih close` was refused | spawn failed: retry the spawn on the same WIH; close refused: retry only the close from the captured output (the harness is not re-run) |
| `pickup_refused` | Gate 1 refused (e.g. a missing predecessor output) | try pickup again |
| `rounds_exhausted` | an `on_fail` node failed again after `max_rounds` route-backs | run the node once more (the flow stays recorded as degraded), or close the plan root yourself |

Gate lines show what the person must look at: a template gate with
`wait_gate.evidence` carries it in `params.evidence` and in its description
(`<description> (look at: <evidence>)`).

### Failure routes (`on_fail`, bounded)

A template step with `on_fail: <step>` mints a node labelled
`on_fail:<target node id>` and `max_rounds:<n>` (template `max_rounds`,
default 3). When that node closes failed (`FAILED` / `FAIL` from a harness
failure or a FAILED close, or `EXCEPTION` from the judge), each drive pass:

- **routes back** while the node's `on_fail_rounds` state is below
  `max_rounds`: one Gate refine reopens the target and every node on the
  `blocked_by` path between it and the failed node (`DagNodeStatusChanged` to
  `NEW`, or `READY` from `EXCEPTION` / `NEEDS_HUMAN`), appends "Round k of N:
  ... its output: <output path> (receipt ...)" to the target's description,
  and sets `on_fail_rounds = k` on the failed node. Then `DriveRouteBack`.
- **stops** when the rounds are used: the plan root's state `closure` is set
  to `degraded` (reason = the template's `closure_degraded` text), the failed
  node is reopened and gets a `rounds_exhausted` needs-you gate, then
  `DriveRoundsExhausted`. Drive never routes that node back again.

A route is skipped (and shown as waiting) while a node on the path is held
by a WIH or mid-run. Rounds come from the projected DAG, not a side file, so
`replay` reproduces them. `--dry-run` prints `would route back X → Y (round
k/N)` or `rounds exhausted → degraded`. `DriveReport` has `routed_back` and
`degraded`.

### Harmful-action idempotency

`DriveAttemptStarted` is written **before** the spawn: from then on the harness
may have acted. An interrupted attempt is restarted automatically only when the
node carries the label `retry:safe` (template step `retry: safe`, or a
`add_label` mutation); every other node goes to needs-you with the attempt id,
and only a human resolution for that attempt restarts it. Dead and timed-out
sessions are never retried: the node closes FAILED with the transcript as its
output receipt.

## Caps and capacity (S10)

| Cap | Default | Scope |
| --- | --- | --- |
| `max_concurrent` | 4 | per DAG (`--max-concurrent` may lower it) |
| `max_spawns_per_hour` | 20 | per DAG, rolling hour (`--max-spawns-per-hour` may lower it) |
| `global_max_concurrent` | 4 | every DAG, every drive process |
| `global_max_spawns_per_hour` | 20 | every DAG, every drive process |

Global state is `.allternit/drive/caps.json`, read-modified-written only under
an exclusive `flock` on `.allternit/drive/caps.lock`. A spawn reserves a running
slot and an hourly entry before pickup; a running slot is freed on finish or
when its tmux session is gone (reservations younger than 30 s are kept so a
concurrent prune cannot race a spawn). A deferred spawn is not picked up and is
logged once per node and reason (`DriveSpawnDeferred`); the loop keeps waiting
while anything is deferred.

Capacity admission: drive refuses to start (`DriveCapacityRefused`, non-zero
exit) and defers spawns while the 1-minute load average per CPU is above
`max_load_per_cpu` (default 2.0) or available memory is below
`min_free_mem_mb` (default 512; Linux `MemAvailable`, macOS free + inactive +
speculative + purgeable pages from `vm_stat`). Env overrides:
`ALLTERNIT_DRIVE_MAX_LOAD_PER_CPU`, `ALLTERNIT_DRIVE_MIN_FREE_MEM_MB`.

Only one drive process per DAG: `.allternit/drive/dags/<dag_id>.lock`.

## Config: `.allternit/drive/config.json`

All fields optional; unknown fields are rejected.

```json
{
  "max_concurrent": 4, "max_spawns_per_hour": 20,
  "global_max_concurrent": 4, "global_max_spawns_per_hour": 20,
  "min_free_mem_mb": 512, "max_load_per_cpu": 2.0,
  "timeout_seconds": 3600, "poll_interval_ms": 2000,
  "harnesses": { "claude": { "argv": ["claude", "-p", "--model", "haiku", "{prompt}"] } }
}
```

Argv placeholders: `{prompt}`, `{prompt_file}`, `{wih_id}`, `{dag_id}`, `{node_id}`.

## Files

`.allternit/drive/runs/<dag_id>/<node_id>/<attempt_id>/` holds `prompt.md`,
`stdout.txt`, `stderr.txt`, `exit_code`. Attempt ids are `<wih_id>-a<n>`;
sessions are `ao-drive-<dag_id>-<node_id>-<n>` and are killed after collection.

## Integration points (sibling branches)

`drive::hooks::DriveHooks` (default `NoHooks`) is where in-flight work plugs in
without changing the runner:

- **judge** (verifier-only close, S9): lands inside `wih close`, which drive
  already calls; `on_node_finished` carries outcome + receipt for a verdict surface.
- **lease heartbeat / stale reclaim** (S9): `on_attempt_heartbeat` fires every
  poll while a session is alive. A reclaimed node shows up to drive as an
  interrupted attempt and follows the idempotency rule above.
- **campaigns / wakes** (S4): `on_needs_you` and `on_node_finished` are wake sources.
- **observer / lessons**: `on_node_finished`.

Hook errors are logged and never change a node's recorded outcome.

## Dry run

`--dry-run` loads the ledger, runs the same decision function and prints what
it would do: the pickup and the exact argv after the spawn gate's rewrite, caps
counts (read without locking), capacity, refusals, bot mail, waits. It writes no
ledger events and no files, takes no locks, and builds no Gate or orchestrator.
