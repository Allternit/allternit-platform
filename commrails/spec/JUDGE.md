# Judge (fail-closed verdicts and tool decisions)

Code: `src/judge/` (backends, parser, prompts, policy, projections, hard floor,
heartbeats), `src/gate/gate_judge.rs` (Gate wiring), `src/cli/judge.rs` (CLI).
Sources: Raven's `report_verdict` judge (RAVEN-JEV-DEEPER-ANALYSIS §A1), audit specs
S2 (permission judge) and S9 (verifier-only close, lease heartbeats).

## The rule

A judge answer only counts when it is a **structured object that echoes a
per-call nonce**. Everything else fails closed:

| Call | Valid answer | Timeout / error / invalid / missing |
|---|---|---|
| Node verdict (Gate 4) | `{"report_verdict": {"verdict": "accomplished"\|"not_accomplished", "category": …, "reason": …, "nonce": …}}` | `needs_human` → node `NEEDS_HUMAN` |
| Tool decision (Gate 2) | `{"report_permission": {"decision": "allow"\|"ask"\|"deny", "reason": …, "nonce": …}}` | `ask` |

Raven fails open (a judge timeout counts as accomplished). This one never does.
Text that says "accomplished", a verdict without the wrapper, a wrong nonce, an
unknown verdict/category, a missing category on `not_accomplished`, or an empty
reason is invalid. The answer may arrive bare, inside Claude Code's
`--output-format json` envelope (`structured_output` or `result` text), or as one
```` ```json ```` fence; nothing is scanned out of prose.

Categories: `missing_user_input`, `missing_credential`, `tool_failure`,
`dependency_output_unusable`, `output_limit`, `other`.

## Self-attestation guard

The node output, evidence refs, receipt list, tool command line and paths are
worker-produced. The prompt places them between `<<<UNTRUSTED:<nonce> …>>>` /
`<<<END:<nonce>>>` fences (fence markers inside the data are defanged) and says
they are data. The nonce is minted after the output is fixed, so a verdict
forged inside the output cannot carry it and never parses. Verdict events are
emitted by the gate (`actor gate:judge`), never by the worker. Tool decisions
see only the tool, command line (≤4 KiB) and paths — never write contents.

## Policy (opt-in; default off)

`allternit judge policy set --dag <dag> [--node <node>] …` records
`JudgePolicySet`. Node level overrides plan level; later events override earlier
ones field by field.

| Field | Default | Effect |
|---|---|---|
| `verify` | `off` | `judge`: Gate 4 asks for a verdict on DONE/PASS closes |
| `close_by` | `any` | `verifier`: the worker cannot close its own node DONE/PASS |
| `tool_judge` | `false` | `true`: Gate 2 `pre_tool` runs the hard floor + judge after its own checks |
| `max_continuations` | `2` | re-open attempts after `not_accomplished` before `NEEDS_HUMAN` |

An agent that holds an open WIH in the dag cannot weaken the policy
(`policy_self_weaken`); the hard floor also denies a worker's command that runs
`judge policy|resolve|continue` or writes `.allternit/judge/`.

## Gate 4 verdict

With `verify: judge`, a DONE/PASS close records the output first, then asks the
judge with: title, description (the resolved prompt when the WIH has one),
acceptance, output (≤24 KiB), evidence refs, receipts. Outcome → node status:

- `accomplished` → requested status (DONE/PASS), normal close.
- `not_accomplished` → `EXCEPTION`; `judge continue` re-opens it (READY) up to
  `max_continuations`. When the cap is used, or the category is
  `missing_user_input` / `missing_credential`, → `NEEDS_HUMAN`.
- `needs_human` (judge failed) → `NEEDS_HUMAN`.

A user closer is the verifier (recorded as accomplished, `source: human`).
`judge resolve … accomplished|continue|abandon --actor user:<id>` lets a person
settle an `EXCEPTION`/`NEEDS_HUMAN` node (DONE / READY, uncounted / FAILED).
`NEEDS_HUMAN` nodes appear in needs-you (`judge pending`, API `needsYou`) with
reason `judge_failed` (the judge itself failed) or `judge_needs_human`.

## Verifier-only close (S9)

With `close_by: verifier` and no `verify: judge`, a DONE/PASS close by the worker
(closer unset, the gate, or the WIH's own agent) is refused with
`gate4.close`/`close_by_verifier` and a `WIHCloseDenied` event. A user or another
agent may close. With `verify: judge` too, the judge is the verifier. FAILED is
never blocked. Actor identity is declarative, as for wait-gate resolution.

## Tool judge (S2)

`allternit judge tool --wih <id> --tool <t> [--command …] [--paths …]` →
`allow | ask | deny`, in this order, each final:

1. Gate 2 checks (open-signed, allowed tools, lease coverage) → `deny`.
2. Hard floor (`judge::hard_rules`) → `deny`.
3. Judge → its decision; any failure → `ask`.

Inside `pre_tool` the same steps 2–3 run only when `tool_judge: true`; `ask` and
`deny` return `allowed: false` (reason `ask: …` / `deny: …`). Every decision is
recorded as `JudgeToolDecision` (command preview only).

## Backends (`.allternit/judge/config.json`)

```json
{
  "backend": "command",
  "timeout_secs": 180,
  "tool_timeout_secs": 30,
  "command": { "argv": ["claude", "-p", "--output-format", "json", "--model", "haiku",
                        "--tools", "", "--setting-sources", "project",
                        "--no-session-persistence", "--json-schema", "{json_schema}"] },
  "system_one": { "url": "http://127.0.0.1:7717", "model": "jev-latest",
                  "confidence_band": 0.85, "timeout_ms": 3000 }
}
```

- **command** (default, argv above when the file is absent): prompt on stdin,
  stdout parsed strictly; `{json_schema}` becomes the answer schema (Claude
  Code's `--json-schema` forces the structured object). Runs in a fresh temp
  cwd so repo hooks and CLAUDE.md are not loaded; `--setting-sources project`
  keeps user-level hooks (steering) out. Non-zero exit = error.
- **system_one** (optional first pass, PR #952's local server): one Choice
  question `complete | verify_more | incomplete`. Only a confident
  (`≥ confidence_band`) `incomplete` short-circuits to `not_accomplished`
  (category `other`); everything else — `complete`, `verify_more`, low
  confidence, server down — defers to the command backend. For tools, a
  confident `risky` short-circuits to `ask`. System One has no code path to
  `accomplished` or `allow`.
- **stub** (`{"backend": "stub", "stub": {"node": "accomplished", "tool": "ask"}}`):
  canned answers through the same parser, for tests and smoke runs. Shortcuts:
  `accomplished`, `not_accomplished[:<category>]`, `allow`, `ask`, `deny`,
  `invalid`, `error`, `hang`; anything else is raw text (`{nonce}` substituted).

An unreadable or invalid config file does not fall back to the default: the
judge becomes unavailable and every verdict is `needs_human`, every tool
decision `ask` (`allternit judge config` shows the error).

## Lease heartbeats and reclaim (S9)

`allternit leases heartbeat <wih> [--pid <pid>]` writes
`.allternit/leases/heartbeats/<wih>.json` (`LeaseHolderHeartbeat` only when the
holder changes). `allternit leases reclaim --stale-after 5m` treats a holder as
stale when its last beat is older than the window, its pid is on this host and
not running, or its WIH is already closed; it releases the leases
(`LeaseReleased` + `LeaseReclaimed`) and closes an open WIH as `RECLAIMED`
(`WIHReclaimed` + `WIHClosedSigned`) so the node can be picked up again.
Leases whose WIH never beat are skipped unless `--include-unbeaten`. The drive
runner is expected to beat every ~60s and sweep with `--stale-after 5m`.

## Not in this slice

- The S9 decision queue (answers merged at the next pickup).
- Wiring gizzi's `smart` permission mode to `judge tool` (S2 "Where" in gizzi).
- Automatic continuation: `EXCEPTION` waits for `judge continue` (orchestrator)
  or `judge resolve` (person), as in Raven's adjudication step.
