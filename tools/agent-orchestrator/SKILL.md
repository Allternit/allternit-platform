---
name: agent-orchestrator
description: Orchestrate external CLI agents (kimi, codex, agy, claude, gizzi-code) through the Allternit Rails communication and coordination system. Use when the user asks to delegate, offload, or farm out implementation work to another CLI agent / terminal session, or to run work outside this session to save tokens.
---

# CLI Agent Orchestrator (v3 — Rails-native)

You are the **orchestrator**: you own scoping, task specs, monitoring, review, and bug-fixing. The **executor** is an external CLI agent in its own tmux session. You never do the bulk implementation yourself — but you always verify it.

The underlying engine is `allternit-commrails` from the `allternit-commrails` crate. The bundled `ao-*` scripts in this directory are thin shims over `allternit-commrails orchestrator` (and `allternit-commrails steer` for consults). The binary must be on PATH:

```bash
cargo build -p allternit-commrails
# symlink or copy target/debug/allternit-commrails onto PATH, e.g. ~/.local/bin
```

Every spawned session is registered as a Rails peer under `.allternit/peers/`, with a per-session UDS inbox. That lets other local agents discover it (`allternit-commrails peer list`, `/list-agents` in gizzi-code) and send plain-text messages to it (`allternit-commrails peer send`, `/send-message`).

## Gate 0 — Enter the DAG first

Per the ratified DAG-as-default rule (`commrails/spec/DAG_AS_DEFAULT_TASK_SYSTEM.md`), orchestrated work is multi-step and cross-session by definition. Before spawning anything: `allternit-commrails plan new "<scope>"` at the workspace root, then `wih pickup` on the node. The spawned executor works against that `dag:<dag_id>` / `wih:<wih_id>`; handoffs and steering reference those ids. Skip only for provably ≤2-step work.

## Phase 0 — Detect agents

```bash
ao-doctor   # probes tmux/script/git + every executor CLI
```

Exit codes: 0 = usable; 1 = no usable executors; 2 = transport broken. Trust its per-vendor verdicts over any static table.

## Phase 1 — Scope and plan (you, in this session)

Same as v2: do the analysis yourself, then write inside the executor's workdir:

- `docs/<TOPIC>_MAP.md` — the full gap analysis.
- `docs/<TOPIC>_PHASE_<N>_TASK.md` — one phase only, with constraints, file paths, and a deliverable sentinel/notes file.

## Phase 2 — Spawn

```bash
~/.claude/skills/agent-orchestrator/scripts/ao-spawn [--worktree] <slug> <repo-dir> "<agent-launch-cmd>"
# prints: ao-<slug> <workdir> <logfile> <peer_id> <inbox_socket>
```

Use `--worktree` whenever the repo allows it. The binary refuses when the git root is `$HOME`.

Headless one-shot pattern — chain the sentinel:

```bash
ao-spawn <slug> <repo> "claude -p '$(cat docs/X_TASK.md)' --permission-mode acceptEdits; touch docs/X_NOTES.sentinel"
```

**Spawn gate (audit S1).** Every launch line goes through `ao-spawn-gate` (bash) / `ao_gate.rs` (engine `ao spawn` and `ao recover --apply`), byte-for-byte identical:

- **claude / claude-code → hook.** Launched with `--permission-mode acceptEdits --settings ~/.agent-orchestrator/logs/ao-<slug>.claude-settings.json`; the settings file comes from `allternit-commrails hook claude-settings` and runs the PreToolUse hook (hard floor + Gate 2) on every tool call. `--dangerously-skip-permissions` (and `--permission-mode bypassPermissions`) is rewritten, with a one-line `spawn gate: rewrote ...` notice. No `allternit-commrails` binary (`$ALLTERNIT_COMMRAILS_BIN`, else PATH) → the spawn is refused, never run unhooked.
- **codex → sandbox.** `--dangerously-bypass-approvals-and-sandbox` becomes `-c 'sandbox_mode="workspace-write"' -c 'approval_policy="never"' -c sandbox_workspace_write.network_access=true`; `danger-full-access` becomes `workspace-write`.
- **everything else (kimi, agy, gemini, sh, ...) → ungated.** Unchanged, and labeled `gate=ungated` in `~/.agent-orchestrator/logs/spawn-gate.log` (every spawn is logged there with its class). With `ALLTERNIT_COMMRAILS_WIH` set, an ungated harness must pass `allternit-commrails hook spawn-check` or the spawn is refused.

The gate classifies by the first command word (after `VAR=value`/`env`/`exec`), so launch the harness directly — `bash -c 'claude ...'` is logged as ungated `bash`.

Each spawned session exports:

- `ALLTERNIT_RAILS_PEER_NAME=ao-<slug>`
- `ALLTERNIT_RAILS_INBOX=<socket-path>`
- `ALLTERNIT_RAILS_ROOT=<workdir>`

so the executor can participate in Rails cross-session messaging.

## Phase 3 — Send prompts

```bash
ao-send <slug> "Read docs/<TASK_FILE> and execute it exactly."
```

`ao-send` now delegates to `allternit-commrails orchestrator send`. It prefers UDS delivery if the peer inbox is listening, otherwise falls back to tmux key injection.

## Phase 4 — Monitor and steer

```bash
ao-watch <slug> <NOTES_FILE> [timeout=3600] [interval=20]
```

Exits 0 on DONE, 3 on PANE-DEAD, 4 on TIMEOUT. To steer or answer questions, `ao-send` into the same session.

To see progress: `ao-status <slug> 40`. To list all sessions: `ao-status`.

## Phase 5 — Review the work

Same as v2: verify the true footprint, scope, claims, and cheap syntax gate. Fix small bugs yourself; send bad implementations back via `ao-send`.

## Phase 6 — Iterate and clean up

When all phases pass: merge/apply the worktree branch, then `ao-kill <slug> [--rm-worktree]`.

## Cross-session messaging

Local peers are discoverable with:

```bash
allternit-commrails peer list
allternit-commrails peer send <name> "<message>"
```

From gizzi-code, the runtime exposes `allternit_list_agents` and `allternit_send_message` tools. Messages travel over the peer's UDS inbox and never leave the machine.

## Platform integration

Executor lifecycle events are still mirrored into CommRails mail thread `wih:executor-<slug>` via `runtime/server/rails-bridge.ts`, and artifacts can be shared with `POST /api/commrails/mail/share` (the old `/api/rails/mail/share` prefix remains as a one-release alias).

## Dispatch semantics (registry, mailbox, ownership, recovery)

The ao engine adds durable dispatch semantics on top of the tmux/rails flow. Additive: the `ao-*` bash scripts and `ao spawn/send/watch/status/kill/doctor` keep their exact behavior.

**Dispatch registry.** `~/.agent-orchestrator/state.json` — `{"sessions": {"ao-<slug>": {...}}}`. Legacy keys `cwd`/`log`/`dead` are unchanged (old readers ignore the rest). Added keys: `runner` (`logs/ao-<slug>.cmd.sh`), `worktree`, `branch`, `sentinel` (armed by `ao watch`), `lead`, `lifecycle` (running|dead|finished), `world` (engine|tmux), `queued`. Written by the engine spawn/kill paths and by the bash `ao-spawn`/`ao-kill` shims via `ao-registry-sync`.

**Bus mailbox — queue-not-drop.** Busy/unverifiable sends go to the durable Rails Bus (`.allternit/bus/queue.db`, server-free) addressed to `peer:ao-<slug>` with transport `mailbox`:

- `ao queue <slug> "msg"` — enqueue; `ao queue <slug>` — list pending.
- `ao send --queue <slug> "msg"` — try the verified immediate send; on busy/unverifiable, enqueue instead of dropping. Plain `ao send` is unchanged.
- `ao drain <slug> [--all]` — inject the oldest pending row through the verified ao-send paste path; settle (`mark_delivered`) only after verified delivery. Failed paste leaves the row `pending`.
- `ao watch` auto-drains the oldest pending row per tick once the pane reads idle.

Never use `GET /api/rails/peers/:name/inbox` for drain (it marks delivered on read). Exactly one drainer per recipient, owned by ao-engine. Mailbox root: `--root` > `AO_PEERS_ROOT` > cwd.

**Fail-closed ownership.** Spawn records the lead (`--lead` / `AO_LEAD` / user / `human`). `queue`/`drain`/`send --queue`/`recover` require that lead or `--as-human`. Unknown owner → refuse.

**Recovery pass.** `ao recover [slug]` reconciles registry vs live tmux + engine. Dead-but-unfinished sessions get a respawn plan from `.cmd.sh`, upgraded to harness resume argv when the runner line is resumable. Dry-run default; `--apply` respawns and re-arms the sentinel watcher.

**Validation boundary.** Unit tests + `infrastructure/executor/ao-engine/tests/dispatch_demo/run.sh`. Not claimed: multi-day autonomous runs, cross-machine leads, atomic claim under concurrent drainers.

## Pitfalls learned the hard way

- kimi `-p` refuses `--yolo`/`--auto` — TUI + `ao-send` is the only autonomous kimi path.
- C-c kills a kimi TUI outright; `ao-send` clears with C-u on mismatch.
- Busy-indicator polling gives false idles on kimi — sentinel files only.
- Use worktrees to attribute changes when multiple agents share a repo.
- Scope each phase so its review fits in a few file reads.
