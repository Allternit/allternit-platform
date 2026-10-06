# Read-only Observer (v1)

A never-write advisor (Fable-class, Raven/Jev analysis §B4) on top of the
steering consult (`src/steer/`): the same "build context, ask an external
agent" loop, pointed at a WIH DAG, with a read-only tool profile, and with its
answer delivered only as mail.

Code: `src/observer/` (`mod.rs` observe + triggers, `profile.rs` read-only
command profiles, `context.rs` prompt, `config.rs`). CLI: `src/cli/observe.rs`.

## What it does

`allternit-factory internal core observe --dag <id> [--wih <id>] --trigger plan|repeat-failure|pre-close`

1. Builds a prompt from:
   - a DAG slice — the whole DAG (≤ 60 nodes) or, with `--wih`, the WIH's node,
     its ancestors, its blocked_by predecessors (transitive) and direct dependents
     (status, title, blockers, executor, description ≤ 600 chars);
   - recorded outputs of the slice's nodes (≤ 4 KiB each), each nonce-fenced
     (`source="node:<id>"`, see `FRESH_CONTEXT_ISOLATION.md`);
   - the last 25 ledger events of the DAG (including its WIHs and mail threads),
     fenced as `source="ledger"`;
   - the read-only rules and the fence rule, before any fenced content.
2. Runs the consult command through a read-only profile (below), with
   `ALLTERNIT_OBSERVER=1` / `ALLTERNIT_OBSERVER_READ_ONLY=1` in its env, killed
   after `timeout_secs` (default 300). Non-zero exit or empty output is an error
   and nothing is posted.
3. Posts the answer as one informational mail message: thread `wih:<id>` when a
   WIH is given, else `dag:<id>` (`MAIL_SCOPE.md`), `from_agent: "observer"`,
   importance low, no ack, subject
   `observer <trigger> dag:<id> [wih:<id>] [sig:<failure signature>]`.

## Write-freedom (enforced twice)

- **By profile.** Known CLIs get read-only flags appended and any other flag is
  refused (only `--model X` / `-m X` passes), so a config cannot switch writes back
  on:

  | consult_cmd | invocation |
  |---|---|
  | `claude` | `claude -p --permission-mode plan --tools Read,Grep,Glob --disallowedTools Bash,Edit,Write,NotebookEdit,MultiEdit --strict-mcp-config` (prompt on stdin) |
  | `kimi` | `kimi --plan -p <prompt>` |
  | `codex` | `codex exec --sandbox read-only --skip-git-repo-check --ephemeral -` (prompt on stdin) |
  | anything else | refused, unless it is exactly the configured `consult_cmd` and `read_only_attested: true` (operator attests it has no write tools — e.g. a stub or an API-only script); run as `bash -c`, prompt on stdin |

  The claude flags were checked against `claude --help` (2026-09-29): `plan` is a
  `--permission-mode` choice; `--tools` limits built-ins; `--disallowedTools`
  denies; `--strict-mcp-config` without `--mcp-config` loads no MCP servers.
- **By construction.** `observe()` holds only a `Ledger` (read) and a `Mail`
  handle. Its only writes are `Mail::ensure_thread` (ThreadCreated, if new) and
  `Mail::send_typed_message` (MessageSent + the body file under
  `.allternit/mail/messages/`). It never touches leases, receipts, the Gate or
  DAG state. Tests assert the ledger delta is exactly `[ThreadCreated,
  MessageSent]` with zero Lease*/ReceiptWritten events.

## Triggers (event-based, no timers)

All configured in `.allternit/rails/observer.json`:

```json
{
  "consult_cmd": "claude --model opus",
  "read_only_attested": false,
  "observe_on_plan": false,
  "observe_on_repeat_failure": true,
  "repeat_failure_threshold": 2,
  "observe_before_close": false,
  "timeout_secs": 300
}
```

Env overrides: `ALLTERNIT_OBSERVER_CMD`, `ALLTERNIT_OBSERVER_READ_ONLY_ATTESTED=1`.

- **Automatic triggers need an explicit observer command** (`consult_cmd` or
  `ALLTERNIT_OBSERVER_CMD`); without one every hook is a no-op. The explicit
  `observe` command additionally falls back to `STEER_CONSULT_CMD` (still forced
  through a profile; the attestation never covers it).
- `plan` — after `plan new` (CLI and `POST /v1/plan`), opt-in `observe_on_plan`.
  Posts on `dag:<id>`.
- `repeat-failure` — after `wih close` with a failure status (`FAIL`, `FAILED`,
  `ERROR`). Failure signature = sha256(node, status, recorded output sha256 — or,
  without output, the sorted non-receipt evidence refs), first 12 hex. When the
  node has failed with the same signature `repeat_failure_threshold` (≥ 2) times,
  the observer runs once for that signature (dedupe: an observer message with
  `sig:<signature>` in its subject already exists). A different failure is a new
  signature. On by default once an observer command is configured.
- `pre-close` — before `wih close` (CLI and `POST /v1/wihs/:id/close`), opt-in
  policy `observe_before_close`. Posts on `wih:<id>`.

Hooks are advisory: a missing/unreadable config, a refused profile, a consult
failure or a timeout prints a warning and the plan/close proceeds. The observer
never blocks or changes a close. In the service, the pre-close hook is awaited
(so the advice lands before the close) and the plan / repeat-failure hooks run
in a spawned task, off the request path.

## Out of scope (v1)

- Second-Brain lint (the same observer pointed at `Allternit Brain/` with
  `brain_audit`/`brain_search`) and the nightly audit campaign — parked per §B4.
- Acting on the advice. Advice is mail; agents and humans decide.
