# Campaigns, keyed wakes, and the attention gate

Status: implemented (`src/campaign/`, `src/wake/`, `src/attention/`, CLI in
`src/cli/campaign.rs`). Source ideas: Raven Oncall (`ops_check_later`,
`budget.py`), Raven's proactive engine (keyed wakes, sentinel nudge policy).
These are the guards only. Raven's shell freedom (no allowlist, sandbox `none`)
and its in-memory event queue are deliberately not copied.

## 1. Campaign

A campaign is a declared objective that owns its budget and its next check.
Cron is a fixed schedule that knows nothing about the goal. A campaign's check
is re-armed by whoever works the campaign, or automatically by `rearm`.

| Field | Meaning |
|---|---|
| `id` | `[A-Za-z0-9_.-]`, unique |
| `objective`, `owner` | required |
| `status` | `active` \| `paused` \| `finished` \| `killed` (the last two are terminal) |
| `executor` | `bot:<slug>` \| `ao:<harness>` \| `command` (+ `command` string) |
| `budget` | optional `{unit, limit, mode: shared\|additive, per_wake?}`; `spent` is projected |
| `dag_id` | optional binding to a WIH DAG (passed to command executors as `ALLTERNIT_DAG_ID`) |
| `rearm` | optional `{every_secs}` or `{weekdays: [mon..sun], at: "HH:MM", timezone}` |
| pending check | at most **one**: the wake keyed `campaign:<id>` |

### One pending check

`campaign check-later <id> <delay_secs>` schedules the wake `campaign:<id>`.
If one is already pending it is **replaced**, and the CLI says so ("replaced
pending check … one check is pending"). The delay is clamped to
`[60s, campaign.check_ceiling_secs]` (default 7 days) and the clamp is
reported. `finish` and `kill` cancel the pending check, so a concluded
campaign never comes back. A terminal campaign cannot be re-armed.

### Budgets

- The **unit is declared and never interpreted**: it is carried, printed and
  compared.
- **additive**: each spend entry pays for itself.
- **shared**: entries with a `start`/`end` span on the same `resource` overlap.
  An overlapping stretch costs what the most expensive single entry costs for
  that stretch. Entries without a span or resource add flat.
- `per_wake`: amount recorded automatically each time a check fires (the
  "looks" meter).
- When recorded spend reaches `limit` on an active campaign, the campaign is
  **paused** (`reason: budget_exhausted`) and a needs-you item
  `campaign:<id>:budget` goes through the attention gate. `resume` refuses
  until `--limit` raises the limit above what has been spent.
- **Budgets do not cap provider bills.** They bound what the campaign records
  as spent. Nothing here meters a model provider, a cloud account or a card.
  An executor that never reports spend never exhausts its budget. Put hard
  spend caps on the provider side.

## 2. Keyed wake queue

A wake is "look at this again at T". It is keyed, and keys coalesce: a new
wake for a key replaces the pending one.

| Key | Target | Fired by a sweep |
|---|---|---|
| `campaign:<id>` | campaign check | dispatch the campaign's executor (see below) |
| `node:<dag_id>/<node_id>` | timer wait-gates on a node | resolve elapsed timers in the DAG; re-arm for the node's next future timer |

Storage is the ledger (`WakeScheduled` / `WakeCancelled` / `WakeFired` /
`WakeCompleted`). `wake::project_pending` rebuilds the queue, and nothing is
held only in memory.

**Timer wait-gates register a wake.** When the Gate emits a
`DagNodeWaitGateAdded` with `kind: timer` (from CLI `wait-gate add`, `plan
refine` or a template expansion), it also appends a `WakeScheduled` for
`node:<dag>/<node>` due at `params.until`. A sweep flips readiness without a
polling loop. Lazy resolution on readiness checks still works as before.

### Sweeps (`wake run-due`)

1. Take the **sweep lock** (`.allternit/rails/wakes/sweep.lock`). This is an OS
   advisory lock (`std::fs::File::try_lock`: flock on unix, LockFileEx on
   Windows), so a crashed holder releases it automatically. A second
   concurrent sweep prints `locked` and exits 0.
2. Release attention items whose deferral has passed.
3. Re-read the ledger under the lock and, for each due wake, append
   `WakeFired` (the claim) → dispatch → `WakeCompleted {outcome, detail}`.
   Delivery is **at-most-once**. A sweep that dies between claim and
   completion leaves the wake listed as `unfinished` (`wake list`,
   `run-due` report), and it is not re-run, so a replay never repeats an
   executor run.

Campaign dispatch. Only an **active** campaign is dispatched. Paused and
terminal campaigns complete with `skipped_<status>`.

| Executor | Behaviour |
|---|---|
| `command` | Runs `/bin/sh -c <command>` in the Factory root **only if** `automation.yaml` has `wake.enabled_executors: [command]` **and** the exact string is in `wake.command_allowlist`. Timeout `wake.command_timeout_secs` (default 900). Env: `ALLTERNIT_CAMPAIGN_ID`, `ALLTERNIT_WAKE_ID`, `ALLTERNIT_WAKE_MESSAGE`, `ALLTERNIT_FACTORY_ROOT`, `ALLTERNIT_DAG_ID`. Non-zero exit or timeout raises needs-you `campaign:<id>:failed`. |
| `command` (not enabled/allowlisted) | No run. Needs-you `campaign:<id>:check`. |
| `bot:<slug>`, `ao:<harness>` | **Never spawned by a sweep**, even if listed in `enabled_executors`. Needs-you `campaign:<id>:check`. Spawn gating (concurrency and hourly caps, S10) lands with the drive runner. |

After dispatch: `per_wake` spend is recorded (this can pause the campaign),
then, if the campaign is still active, has `rearm` and has no pending check,
the next check is armed.

Nothing in this change runs `wake run-due` on a schedule. Whoever installs a
tick (launchd, cron) decides that explicitly.

## 3. Attention gate

Every agent→human notification goes through `AttentionGate::submit`:
needs-you items raised by campaigns and sweeps, plus `attention submit` for
scripts (e.g. a sweep report). The decision is the pure function
`attention::policy::decide(policy, candidate, history, now)`:

1. **Dedupe.** The same `key` with the same content hash (sha256 of title and
   body) is **coalesced** into the existing item if that item is still queued
   or was delivered inside `dedupe_window_secs` (default 24h).
2. **Quiet hours.** `{start, end}` local `HH:MM` in `timezone` (IANA). The
   window may span midnight. The item is **queued** until the window ends.
   DST: an ambiguous end takes the earlier instant, and a nonexistent end
   (spring-forward gap) moves to the first valid minute after the gap.
3. **Hourly cap.** `per_hour_cap` deliveries in any rolling 60 minutes
   (default 6; 0 = no cap). Over the cap, the item is **queued** until a slot
   frees. If that moment is inside quiet hours, it waits for the quiet end.

Nothing is dropped. Queued items are released by `attention release` and by
every `wake run-due`, oldest first, re-checking quiet hours and the cap.
Coalesced items stay in the ledger and point at the item they merged into.
Money and deploy approvals are queued like everything else, never discarded.

Delivered items are **open** until acked (`attention ack`). Open items appear in
`attention list --open` and in the API visibility `needsYou` list
(`reason: "attention"`, id `attention:<item_id>`). `mail` channel items also
send a typed mail message on `mail:attention` to `attention.recipient`
(default `joe`).

## 4. Config: `.allternit/rails/automation.yaml`

Optional. Every default is the safe one: no executor runs, 7-day check
ceiling, UTC, no quiet hours, 24h dedupe, 6/hour cap. See the header of
`src/wake/config.rs` for the full shape.

## 5. Example definitions

`docs/examples/campaigns/` (both ship `status: paused`):

- `research-pipeline-sweep.yaml`: the mechanical research sweep (weekdays
  09:05 America/Chicago, `command` executor, 25-run budget, `per_wake: 1`).
  The launchd job `com.allternit.research-pipeline-sweep` stays the live
  schedule. This file does not change or unload it.
- `nightly-audit.yaml`: the parked Nightly Audit (`bot:nightly-audit-engineer`).
  **Eoj must enable it explicitly.**

## 6. Not yet

- Spawning bot/ao executors from a sweep (drive runner + S10 caps).
- Declared action tables with receipt-keyed idempotency for harmful actions
  (S10). Today's at-most-once wake claim is the replay guard.
- DND file edited by the human (`.allternit/attention.md`), topic quotas,
  daily caps.
