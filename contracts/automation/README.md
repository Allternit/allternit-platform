# Automation contract (Primitives Wave A, PR2)

Shared by the gizzi runtime (`cmd/gizzi-code/src/runtime/primitives/heartbeat.ts`,
used by the runtime `/cron` fallback) and allternit-api's routine scheduler
(`routine_local_scheduler.rs`), which both run heartbeat automations.

- **`heartbeat-verdict.json`:** test vectors for the verdict parser. Each entry is
  `{note, input, expected}`; `expected` is `{kind: "notify"|"done", line}` or
  `{kind: "dont_notify"|"none"}`. Both parsers must pass every vector. Add a vector
  here first when the rule changes.
- **`heartbeat-instructions.txt`:** the text appended to a heartbeat run's prompt.
  It must equal `Heartbeat.heartbeatInstructions()` (a gizzi test checks this); Rust
  reads it with `include_str!`.

## The verdict rule

The verdict is the last non-empty line of the reply. Trailing code fences and
markdown rules are skipped, and markdown around the line (bold, inline code, a
quote marker or a bullet) is ignored. The keyword is case-insensitive.

| Last line | Verdict | Scheduler |
|---|---|---|
| `NOTIFY: <line>` (or `NOTIFY - <line>`) | `notify` | post `<line>` to the target thread and push it |
| `DONT_NOTIFY` (also `DONT NOTIFY`, `Don't notify`, `Do not notify`; text after it is ignored) | `dont_notify` | quiet `skipped` run |
| `DONE: <line>` or `DONE` | `done` | post and push `<line>` (or a default), then mark the automation complete |
| anything else, including `NOTIFY` with no message | `none` | the run is `failed`; the failure streak pauses the automation |
