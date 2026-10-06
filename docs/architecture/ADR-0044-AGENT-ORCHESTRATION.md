# ADR-0044: Vendor-Neutral Agent Orchestration (`@allternit/orchestrator`)

- **Status:** Accepted
- **Date:** 2026-07-12
- **Owners:** Platform API / Agents

## Decision

Allternit will ship cross-vendor agent orchestration as a native platform primitive: one agent (any vendor) acts as **orchestrator** — it scopes work, delegates execution to **executor** agents running in isolated sessions, monitors them, reviews the produced work, and iterates — through a single primitive set defined in `packages/@allternit/orchestrator`.

Every major harness now ships orchestration for its own agents only (Claude Code agent teams, Codex cloud tasks, Antigravity's agent manager). None will ever orchestrate a competitor's agent — the incentive structure forbids it. Allternit's entire positioning is the vendor-neutral layer: provider adapters, vendored kernel views (OpenClaw, Hermes, Kimi), the unified MCP connector sidecar (ADR-0043), the ACU engine. Cross-vendor agent orchestration is the same play one level up: the platform that coordinates everyone's agents, because no vendor can.

## The primitive set

| Primitive | Meaning |
|---|---|
| `ExecutorBackend` | Where executor sessions live. v1: `local-terminal` (tmux, transcript via `script(1)`). Planned: `kernel` (in-process OpenClaw/Hermes/Kimi views), `cloud`, `acu` (computer-use tasks via the ACU gateway). |
| `SessionRegistry` | Spawn / status / kill executor sessions; owns session state (`spawning → running → done | dead | killed`) and transcript refs. |
| Delegation verbs | `handoff` (synchronous — spawn, send, block until completion report), `assign` (asynchronous — returns a live session), `sendMessage` (steer or re-task a running session). Verb shape follows the pattern proven by awslabs/cli-agent-orchestrator. |
| Completion contract | The executor writes a notes file whose YAML frontmatter (`status: done|blocked`, `files_changed`, `deviations`, `remaining`) is the ONLY completion signal — never TUI busy-indicator inference. Parsed into a `CompletionReport`. |
| Review gate | Delegation is not done until the orchestrator has verified the diff against the declared scope. `handoff` returns the report *plus* the footprint; acceptance is the caller's explicit act. Phase graphs (delegate → watch → review → iterate) compose as `@allternit/workflow-engine` DAG nodes. |

## Existing component disposition

| Component | Decision |
|---|---|
| `packages/@allternit/orchestrator` | New. Canonical home of the primitive set (this ADR). |
| `packages/@allternit/executor-core` | Kept as-is: it models *parallel variant racing* (one goal, N model variants, verification profiles) — a sibling concern, not a substitute. `orchestrator` models *sequenced delegation with review*. A later ADR may unify their status/event vocabularies. |
| `packages/@allternit/parallel-run`, `cowork-engine` | Untouched consumers-to-be; cowork mode is a natural orchestrator caller. |
| `packages/@allternit/workflow-engine` | Integration target: orchestration phases as DAG nodes (phase 2). |
| `~/.claude/skills/agent-orchestrator` and its original shell scripts (dev machine) | Reference implementation and proving ground. The `local-terminal` backend encodes the same semantics (verified send, sentinel watch, `script(1)` transcripts, worktree isolation). Superseded by the Allternit Factory (`gizzi agents|orchestration`); the scripts are retired (see `surfaces/docs/factory/migration.mdx`). |

## Vendor launch matrix (v1 knowledge baked into the package)

| Vendor | Interactive (steerable) | Headless one-shot | Constraint |
|---|---|---|---|
| kimi | `kimi --yolo` | none | `-p` cannot combine with `--yolo`/`--auto` |
| codex | `codex --dangerously-bypass-approvals-and-sandbox` | `codex exec "…"` | Verify current flags with `codex --help`; `--yolo` is not exposed by the July 2026 CLI. |
| claude | `claude --dangerously-skip-permissions` | `claude -p "…" --dangerously-skip-permissions` | |
| agy | `agy --dangerously-skip-permissions` | TBD | |

## Hard rules (encoded, not documented-only)

- The verified-send protocol: paste → read pane back → submit only on byte-verified match (alnum comparison, immune to TUI wrap/borders). A mismatch clears with `C-u`; **`C-c` is never sent to a TUI executor** (kills kimi outright).
- Completion = sentinel/notes file existing. Pane-dead and timeout are distinct terminal states with distinct results.
- Worktree isolation (`<repo>-ao-<slug>`, branch `ao/<slug>`) whenever the repo allows; refused when the git root is a home directory.
- tmux exact-match pane targets require the `=NAME:` form (trailing colon) — bare `=NAME` fails on tmux ≥ 3.6 pane-target commands.
- Executors never make product decisions; task specs are written by the orchestrator into the executor's workdir before spawn.

## Surfaces (phase 2 — not in this ADR's implementation)

- MCP tools (`orchestrator_spawn`, `orchestrator_handoff`, `orchestrator_assign`, `orchestrator_send`, `orchestrator_status`, `orchestrator_kill`) registered through the unified MCP path so any kernel or external agent can delegate.
- `/v1/orchestrator/*` HTTP routes via `CanonicalApiRoutes()` in `cmd/allternit-api` for the platform/desktop/extension surfaces.
- Kernel views (OpenClaw/Hermes) invoke the package directly, NOT through allternit-api (consistent with the dedicated-views decision).

## Rollout gate

The TypeScript package lands now; wiring it into MCP/routes/kernels (phase 2) is gated on the completion contract surviving at least 3 real cross-vendor delegation runs on the dev-machine reference implementation without contract changes. If real runs force contract changes, amend this ADR before phase 2.

### Gate record

| Run | Executor | Path | Result |
|---|---|---|---|
| 1 | Claude | Headless `SessionRegistry.handoff` in an isolated scratch-repo worktree | Contract unchanged; review found and fixed untracked-directory footprint collapse by adding `git status --porcelain -uall` |
| 2 | Codex | Interactive `ao-*` delegation in the shared development worktree; added honest runtime/saved peer identity discovery across the Gizzi peer API and Code Mode UI | Contract unchanged; sentinel report matched the four-file footprint and the three changed source files passed the syntax gate |
| 3 | Kimi | Interactive `ao-*` delegation in the shared development worktree; added deterministic human-readable output for orchestrator runtime discovery | Contract unchanged; sentinel report matched the four-file footprint and both changed TypeScript files passed the syntax gate |

The three-run gate is satisfied. Phase 2 HTTP/MCP/kernel integration may proceed,
while retaining the same completion contract and mandatory caller-owned review gate.
The first phase-2 slice is owned by Gizzi's Node runtime at `/v1/orchestrator/*`:
it holds the live `SessionRegistry`, derives launch commands from the vetted vendor
matrix, and exposes doctor, assign/handoff, status/tail, verified send, watch/review,
and kill operations. The Rust API may proxy this surface later; it must not duplicate
the in-memory executor registry.

Gizzi's stdio MCP entrypoint runs as a separate process, so it must expose orchestration
tools as clients of `/v1/orchestrator/*`. It must not instantiate another local
`SessionRegistry`, which would split session ownership and make status, steering, and
kill operations disagree between HTTP, Code Mode, and MCP callers.

Both Gizzi stdio MCP entrypoints now register the shared HTTP-backed tool catalog:
`orchestrator_doctor`, `orchestrator_spawn`, `orchestrator_assign`,
`orchestrator_handoff`, `orchestrator_status`, `orchestrator_send`,
`orchestrator_watch`, and `orchestrator_kill`. The bridge forwards configured Gizzi
basic authentication and supports `GIZZI_ORCHESTRATOR_URL` for non-default runtime
placement.

`cmd/allternit-api` exposes a stateless protected proxy at
`/api/v1/orchestrator/*`. It preserves method, query, status, content type, and body,
and targets Gizzi's `/v1/orchestrator/*`. Code Mode prefers the gateway and falls
back to direct Gizzi only when the gateway route is absent (404); authentication or
upstream errors are never bypassed.

## Accepted follow-up design

### Runtime discovery and fallback

The static launch matrix remains the source of known command shapes, but the package
must probe installed CLIs and verify required flags before selecting an executor.
`doctor()`, `probeVendor()`, and ordered `selectVendor()` preferences implement this
without weakening the explicit guards in `vendors.ts`. Discovery is advisory: an
unknown or incompatible CLI fails closed instead of silently changing execution mode.

### Terminal-control backend candidate

`anomalyco/terminal-control` is a candidate `local-pty` backend, not an orchestration
replacement. Its rendered-screen waits and capture/recording evidence could replace
timing heuristics under `ExecutorBackend` while leaving delegation verbs, completion
contract, footprint, and review gate unchanged. The existing tmux backend stays the
default until terminal-control proves installation maturity and live-observation
tradeoffs are acceptable.

The optional `TerminalControlBackend` is now implemented as backend kind `local-pty`
and selectable explicitly as `terminal-control` through HTTP, MCP, and Code Mode.
It was initially unavailable and correctly failed closed. Terminal-control 0.3.1 was
subsequently installed with a side-by-side Rust 1.97 toolchain. A real Codex
delegation exposed and fixed two transport issues (input-box startup readiness and
wrapped verification markers), then completed with matching notes plus PNG, text, and
`.termctrl` evidence. tmux remains the conservative default; `local-pty` is validated
and explicitly selectable.

### Durable lifecycle and remaining backends

Gizzi persists redacted session/spec metadata with owner-only permissions, restores it
on restart, and publishes lifecycle/review events over SSE. Review is explicit:
completed work remains `pending` until accepted or rejected, and the decision is an
event plus persisted session state. Code Mode consumes SSE rather than polling.

Kernel, cloud, and ACU are injection-owned backends through
`ExecutorBackendDriver`. They fail closed until the owning runtime supplies a driver,
and a driver without footprint support cannot silently satisfy the review contract.

### Process-supervisor patterns

Runtime discovery, doctor diagnostics, and ordered vendor fallback are adopted as
independently implemented patterns. No source from `backnotprop/orchestrator` may be
copied or vendored because its BSL 1.1 restrictions can conflict with Allternit's
commercial hosted orchestration use.

### Peer collaboration boundary

Peer discovery, mailbox messaging, broadcasts, and read state are collaboration
primitives over Gizzi's existing team registry. They do not spawn or supervise
executors and therefore do not bypass the phase-2 orchestration gate. The Code Mode
peer surface may ship before orchestrator HTTP/MCP routes.
