---
name: agent-orchestrator
description: Orchestrate external CLI agents (kimi, codex, agy, claude, grok, gizzi) through the Allternit Factory (gizzi agents / orchestration / workspace / workflows). Use when the user asks to delegate, offload, or farm out implementation work to another CLI agent / terminal session, or to run work outside this session to save tokens. Part of Allternit Agents / Bot Agents — not a separate product.
---

# CLI Agent Orchestrator (v4 — Allternit Factory)

You are the **orchestrator**: you own scoping, task specs, monitoring, review, and bug-fixing. The **executor** is an external CLI agent running as a **Terminal bot** in an Allternit Factory pane. You never do the bulk implementation yourself — but you always verify it.

The Factory is what Gizzi and Allternit Desktop both run. People and agents type `gizzi <part> <verb>`. The engine underneath (`allternit-factory`) is never typed. Brain page: `Allternit Brain/Products/Factory.md`. Dev docs: `allternit/surfaces/docs/factory/` (commands, determinism, migration). The older orchestration tools are merged into it, and their names are removed, not aliased.

**Contract you can rely on** (every `gizzi` Factory verb):
- `--json` prints one JSON document and nothing else. Mutations take `--dry-run` and print exactly what would change.
- Exit codes: `0` ok · `1` refused by the Gate · `2` not found (also "`<part> <verb>` is not built yet") · `3` transport broken or engine missing · `4` timeout · `5` needs a person · `64` usage · `70` internal.
- On error with `--json`: `{"error":{"code","fact","action"}}`. Read `fact` and `action` and act on them. Never guess past a refusal.

## Old names

The `ao-*` scripts and `allternit-rails` are gone (removed 2026-10-06, after the `gizzi` commands passed the end-to-end check on gizzi-code 2.2.1). If a note or task file still names one, use the Factory command:

| Old | Factory |
|---|---|
| `ao-doctor` | `gizzi doctor`, `gizzi agents doctor`, `gizzi agents harness list` |
| `ao-spawn [--worktree] <slug> <repo> "<cmd>"` | `git worktree add` + team file + `gizzi agents up <team> --workdir <wt>` |
| `ao-send <slug> "…"` | `gizzi orchestration send <bot@team> "…" [--queue]` |
| `ao-watch <slug> <sentinel>` | sentinel loop over `gizzi orchestration capture` + `gizzi agents ps --json` (Phase 4) |
| `ao-status [slug] [lines]` | `gizzi agents ps`, `gizzi orchestration capture <bot@team> [lines]` |
| `ao-kill <slug> [--rm-worktree]` | `gizzi agents down <team> [--rm-worktree]` |
| `ao recover` | `gizzi agents recover [--apply]` |
| `ao drain` | `gizzi orchestration drain <bot@team> [--all]` |
| `allternit-rails plan new` / `wih pickup` | `gizzi workspace plan new` / `gizzi workspace node claim` |
| `allternit-rails peer list` / `peer send` | `gizzi agents ps` / `gizzi orchestration send` |
| `~/.agent-orchestrator/state.json` | `~/.allternit/factory/registry.json` (migrated once by the engine) |

If a `gizzi` Factory verb exits `3` (engine missing), install it: it ships with Allternit Desktop and `brew install allternit/tap/gizzi-code`, or set `$ALLTERNIT_FACTORY_BIN` to a dev build. There is no script fallback.

## Gate 0 — Enter the DAG first

Orchestrated work is multi-step and cross-session by definition (DAG-as-default rule). Before spawning anything, at the workspace root:

```bash
gizzi workspace plan new "<scope>" --dry-run   # check, then run without --dry-run
gizzi workspace node add …                      # one node per phase, if the plan didn't make them
gizzi workspace node claim <node>               # the WIH pickup for the phase you're delegating
```

The executor works against that `dag` / node id. Handoffs, steering and sends reference it (`gizzi orchestration send … ` with the node in the task file). Skip only for provably ≤2-step work.

## Phase 0 — Detect agents

```bash
gizzi doctor                  # Gizzi, the engine, harnesses, transport, install
gizzi agents doctor           # panes, harnesses and the registry against reality
gizzi agents harness list     # which harnesses the engine can drive here
```

Exit `0` = usable. Exit `3` = the engine is missing or its transport is broken (follow the printed action). Trust these per-harness verdicts over any static table.

## Phase 1 — Scope and plan (you, in this session)

Do the analysis yourself, then write inside the executor's workdir:

- `docs/<TOPIC>_MAP.md` — the full gap analysis.
- `docs/<TOPIC>_PHASE_<N>_TASK.md` — one phase only, with constraints, file paths, and a deliverable sentinel/notes file.

Executors can't read outside their workspace, so inline what they need (`Allternit Brain/delegation-runbook.md`).

## Phase 2 — Spawn

A spawned executor is a Terminal bot on a team. For a one-off delegation, a team of one is fine.

```bash
# 1. Worktree (refuse if the git root is $HOME)
git -C <repo> worktree add ../<repo-name>-<slug> -b factory/<slug>

# 2. Team file at the workspace root: .allternit/teams/<team>/team.yaml
cat > .allternit/teams/<team>/team.yaml <<'YAML'
name: <team>
bots:
  - { bot: <slug>, role: build, binding: terminal, harness: claude }   # or codex / kimi / grok / agy / gizzi
YAML

# 3. Plan, then apply
gizzi agents up <team> --workdir ../<repo-name>-<slug> --dry-run --json   # TeamPlanStep[]: spawn / bind / skip / stop
gizzi agents up <team> --workdir ../<repo-name>-<slug>
gizzi agents ps --json                                                     # the bot, its pane, state
```

The address is `<slug>@<team>`. The pane exports `ALLTERNIT_FACTORY_BOT`, `ALLTERNIT_FACTORY_TEAM`, `ALLTERNIT_FACTORY_PANE_ID` (and `ALLTERNIT_FACTORY_BOT_ID` once bound), so the executor knows who it is (`gizzi agents whoami`). The engine delivers the bot's persona as guidance, its context pack and skills, the Allternit connector and the Gate hook at spawn. Add `model:` or `--preset` per `team.yaml` when the task needs a specific model.

A spawn the Gate refuses exits `1` and says why. Fix the cause. Don't retry blindly.

## Phase 3 — Send prompts

```bash
gizzi orchestration send <slug>@<team> "Read docs/<TASK_FILE> and execute it exactly." --json
```

Read the delivery in the reply:
- `via: pane`, `state: verified` — it landed.
- `via: pane_queue`, `state: queued` — the pane was busy or the paste couldn't be verified, so it's in the bot's mailbox. `detail` says why. Deliver it once the pane is idle with `gizzi orchestration drain <slug>@<team>`.
- `failed` — nothing landed. Read `detail` and fix it.

Use `--queue` to queue on purpose. Use `--dry-run` to see the records a send would write. A repeated idempotency key returns the first delivery instead of sending twice. Every send is recorded in mail and as a `factory.delivery` ledger event (`gizzi orchestration mail list`, `GET /api/factory/deliveries?agent=`).

## Phase 4 — Monitor and steer

Completion is the **sentinel file's existence only**, never TUI activity (kimi shows no spinner). Watch in the foreground, bounded:

```bash
deadline=$(( $(date +%s) + 3600 ))
while [ ! -f <workdir>/docs/<NOTES>.sentinel ]; do
  state=$(gizzi agents ps --json | jq -r '.agents[] | select(.address=="<slug>@<team>") | .state')
  case "$state" in offline|failed) echo "PANE-DEAD"; break;; esac        # was exit 3
  [ "$(date +%s)" -ge "$deadline" ] && { echo "TIMEOUT"; break; }       # was exit 4
  sleep 20
done
```

- Progress: `gizzi orchestration capture <slug>@<team> 40`. Full log: `gizzi orchestration transcript <slug>@<team>`.
- `needs_you` state, or `gizzi orchestration attention list`, means the executor is waiting on a person or an approval.
- To steer or answer questions, `gizzi orchestration send` into the same bot.
- A person can watch live with `gizzi agents wall [team]` or `gizzi agents attach <slug>@<team>` (interactive, so don't run it yourself).
- For work that's already a DAG from a template, `gizzi workflows drive <dag> --once --dry-run`, then `--once`, drives READY nodes through the Gate instead of hand-sending.

## Phase 5 — Review the work

Verify the true footprint (`git -C <worktree> diff --stat`), scope, claims, and a cheap syntax gate. Never accept the notes file at face value. Fix small bugs yourself. Send bad implementations back with `gizzi orchestration send`. Proof for DAG work lives in the node: `gizzi workspace proof show`, `gizzi workspace judge show`.

## Phase 6 — Iterate and clean up

When all phases pass: merge/apply the worktree branch, close the node (`gizzi workspace node close <node>`), then

```bash
gizzi agents down <team> --dry-run
gizzi agents down <team> --rm-worktree     # keeps the branch
```

If a session died mid-run: `gizzi agents recover` (dry run by default) shows a respawn plan with the harness's resume argv. `--apply` respawns.

## Cross-session messaging

```bash
gizzi agents ps                                  # every bot of every binding: state, node, proof
gizzi orchestration send <bot@team> "<message>"  # Terminal, Hosted or Vendor bot; the engine picks the delivery
gizzi orchestration mail list | read <thread>    # the recorded threads
gizzi orchestration feed --json                  # ledger events
```

From gizzi-code, the runtime's `allternit_list_agents` and `allternit_send_message` tools reach the same bots. Local pane sends never leave the machine.

## Platform integration

Desktop, ai.allternit.com and the phone read the same engine through allternit-api's `/api/factory` proxy: agents, capture, transcript, send, deliveries, the `/api/factory/events` SSE stream, templates, runs, campaigns, nodes, and approvals (owned by allternit-api). A delegated executor shows up in Code mode's terminal wall and on the team strip as a Terminal bot.

## Dispatch semantics

- **One registry:** `~/.allternit/factory/registry.json` (`$ALLTERNIT_FACTORY_HOME` moves it), migrated once from the old orchestrator registry. It's reconciled against live panes at engine start and on every `gizzi agents ps`: a gone pane reads `offline`, never running. Panes a person opened with an agent in them are adopted into the registry too.
- **Queue, never drop:** a send to a busy or unverifiable pane goes to the bot's mailbox (`pane_queue` / `queued`). `gizzi orchestration drain <bot@team> [--all]` delivers it, oldest first, through the same verified paste; a message is settled only once its delivery is verified.
- **No silent fallback:** without the pane engine every spawn and send fails with exit `3`. There's no tmux path.
- **Spawn logs** for engine spawns are in `~/.allternit/factory/logs/`.

## Pitfalls learned the hard way

- kimi `-p` refuses `--yolo`/`--auto` — run kimi as an interactive Terminal bot and send to it.
- C-c kills a kimi TUI outright. The verified send clears with C-u on a mismatch, so don't paste by hand.
- Busy-indicator polling gives false idles on kimi — sentinel files only.
- Use worktrees to attribute changes when multiple agents share a repo. Never attribute working-tree changes to your executor without a worktree or mtime evidence.
- Scope each phase so its review fits in a few file reads. A 2,000-line unreviewed diff is not "done".
- macOS screen-recording filenames contain U+202F — use globs for user-provided recordings.
- Don't drive Terminal.app with AppleScript (focus races injected prompts into the wrong window). If the user wants a visible window, have them run `gizzi agents attach <bot@team>` or `gizzi agents wall`.
- A `best_effort` delivery (vendor or channel) is not proof the bot read it.

## Provenance

Patterns adopted from: awslabs/cli-agent-orchestrator (named sessions, status semantics, attach-to-observe), kingbootoshi/codex-orchestrator (persistent transcript logs, completion notification chaining), primeline-ai/claude-tmux-orchestration (verify-idle-before-send handshake), claude-squad/Crystal (worktree-per-agent isolation). The pane engine is a fork of herdr (Apache-2.0).
