---
name: steer-parallel-agent
description: Steer an already-running parallel agent session (another Kimi Code, Claude Code, Codex, Agy, etc.) that is working on the same project. Use whenever the user mentions another agent, a parallel session, a different CLI agent, a second agent, a background agent, "the other agent", "agent working on", "steer that session", "don't mess up its work", "coordinate with", "parallel agent", "another session", "same project", "already running", "existing agent", "target session", "redirect the agent", "augment its work", "merge plans", or "cross-agent steering". Covers discovery, non-invasive steering, activation, and verification — even when the target agent is not in tmux, not registered as a Factory peer, and cannot be injected into directly.
---

# Steer a Parallel Agent Session

You are the **steering agent**. The **target agent** is an already-running CLI agent session (Kimi Code, Claude Code, Codex, Agy, etc.) that is actively modifying the same project. Your job is to redirect or augment its work **without breaking its state**.

**Never do this:**
- Edit the target agent's session files (`wire.jsonl`, `state.json`, `agents/*/context/`) directly.
- Kill or restart the target session.
- Overwrite files the target agent is actively editing.
- Assume the target agent will magically discover your files.

**Core insight:** Most CLI agents write session state to disk in a predictable location. You can read that state to understand what the agent is doing, then write steering documents to a shared location the agent can discover, and finally activate the steering by having the user paste a pointer prompt into the target session.

## Scripted toolkit (deterministic path — prefer this over manual steps)

Bundled scripts live in `tools/agent-orchestrator/scripts/` in the Allternit platform repo (canonical source) and are symlinked onto PATH at `~/.local/bin` by the installer. Call them by bare name:

| Script | What it does |
|---|---|
| `steer-discover [--project <substr>] [--self <session-id>]` | Scan all local agent sessions, newest first. Shows session ID, age, subagent assignments, last user message. `--self` excludes your own session. |
| `steer-context <session-id>` | Dump the target's `state.json` summary, active plan files, and last 25 meaningful wire events. Read-only. |
| `steer-checkpoint <repo> <from-name> [-f file]` | Write `<repo>/.steering/checkpoint.md` from stdin or a file, and post a Factory steering checkpoint event if the API is up. |
| `steer-prompt <file1> [file2...]` | Generate the pointer prompt and copy it to the macOS clipboard. |
| `steer-verify <session-id> <path>` | Exit 0 if the target's `wire.jsonl` shows it Read the given file; exit 1 otherwise. |
| `steer send <session-id> <file1> [file2...]` | Full flow: generate prompt, copy to clipboard, then poll `steer-verify` up to 10 min until the target reads the first file. |

Typical deterministic flow:

```bash
steer discover --project <repo-name>          # 1. find the target session
steer context session_<uuid>                  # 2. read its context
steer checkpoint <repo> "$(whoami)" -f my-steering.md   # 3. write checkpoint
steer send session_<uuid> <repo>/.steering/checkpoint.md docs/coordination/<topic>-handoff.md
```

The manual phases below document what the scripts do — read them when you need to adapt the pattern to a non-Kimi target or an obstacle the scripts don't cover.

---

## Phase 0 — Discover the target session

The user will usually give you a session ID or a directory. If not, scan for active sessions.

### Kimi Code sessions

```bash
ls -lt ~/.kimi-code/sessions/ | head -20
```

Each session lives in `~/.kimi-code/sessions/wd_<workspace_hash>/session_<uuid>/`.

Key files:

| File | Purpose |
|---|---|
| `state.json` | Session metadata, agent tree (main + subagents), CWD |
| `agents/main/wire.jsonl` | Full event log: user messages, tool calls, agent thoughts, tool results |
| `agents/main/plans/` | Active plan files |
| `agents/main/tasks/` | Background task outputs |

### Claude Code sessions

```bash
ls -lt ~/.claude/projects/ | head -20
```

Claude stores sessions by project path. Look for `*.jsonl` files.

### Codex sessions

```bash
ls -lt ~/.codex/sessions/ | head -20
```

### Finding the right session

If the user gives you a session ID, verify it exists and is active:

```bash
cat ~/.kimi-code/sessions/wd_<hash>/session_<uuid>/state.json | python3 -m json.tool
```

If the user says "the agent working on the website", search for sessions whose `state.json` or `wire.jsonl` mentions the project name:

```bash
grep -r "Allternit-websites" ~/.kimi-code/sessions/*/session_*/state.json 2>/dev/null
```

---

## Phase 1 — Read the target's context (read-only)

Before steering, understand what the target agent knows and is doing.

1. **Read `state.json`** — see the agent tree (main agent, subagents, their labels/assignments).
2. **Read `agents/main/plans/`** — see what plan the agent has written or is executing.
3. **Tail `agents/main/wire.jsonl`** — see the most recent turns, tool calls, and thoughts.

```bash
tail -n 100 ~/.kimi-code/sessions/wd_<hash>/session_<uuid>/agents/main/wire.jsonl | \
  grep -E '"type":"context.append_message"|"type":"context.append_loop_event".*"type":"(think|text|tool.call)"' | \
  tail -30
```

Look for:
- What the agent believes its current task is.
- What files it has already changed.
- What subagents it has spawned and their assignments.
- Whether it is in plan mode, executing, or waiting for user input.

---

## Phase 2 — Write steering documents (non-invasive)

Create steering artifacts in a **shared location the target agent can read**. Do not write to the target's session directory.

### 2.1 Steering checkpoint (highest priority)

If the target project has a `.steering/` convention (common in repos with steering hooks), write:

```bash
<repo>/.steering/checkpoint.md
```

Structure:
- `From:` / `To:` / `Date:` header
- `Goal` — one sentence
- `What I did` — your analysis or prior work
- `What I see in your work` — acknowledge the target's existing work
- `Reconciled direction` — merge your plan with theirs
- `Immediate next steps` — concrete actions
- `Files to read` — paths to your full analysis

### 2.2 Coordination handoff (detailed)

```bash
<repo>/docs/coordination/<topic>-handoff.md
```

Structure:
- What each agent owns (avoid file conflicts)
- Reconciled plan (merged from both agents)
- Conflict-avoidance rules (do-not-touch lists)
- Phase sequencing
- Append-your-status section

### 2.3 Factory checkpoint event (optional, if allternit-api is running)

```bash
curl -s -X POST http://127.0.0.1:8013/api/factory/steer/checkpoint \
  -H "Content-Type: application/json" \
  -d '{"cwd":"<repo-path>","notes":"<brief summary>"}'
```

This only helps if the target agent has the Factory steering hooks configured
(or use `gizzi orchestration steer checkpoint`).

---

## Phase 3 — Activate the steering

The target agent will not automatically read your files. You must provide a **pointer prompt** for the user to paste into the target session.

### Pointer prompt template

```text
Read these files and follow them exactly:
1. <repo>/.steering/checkpoint.md
2. <repo>/docs/coordination/<topic>-handoff.md

They contain <brief description>. Do not continue implementing your current plan until you have read both. After reading, update your plan to include <key additions> and then proceed.
```

### If the target is in plan mode

The pointer prompt will cause it to read the files and update its plan. This is safe.

### If the target is executing

The pointer prompt will interrupt its current turn. The agent should stop, read the files, and reconcile. This is acceptable but slightly more disruptive.

### If the target is waiting for user input

Perfect timing — the user can paste the prompt immediately.

### If you cannot ask the user to paste

Try these fallbacks in order:

1. **Factory peer message** — if the target is registered as a Factory peer
   (`gizzi orchestration send <bot@team> "…"` for a bot on a team):
   ```bash
   curl -X POST http://127.0.0.1:8013/api/factory/peers/<name>/send \
     -H "Content-Type: application/json" \
     -d '{"message":"Read .steering/checkpoint.md"}'
   ```

2. **tmux send-keys** — if the target is in a tmux session:
   ```bash
   tmux send-keys -t <session> "Read .steering/checkpoint.md" Enter
   ```

3. **File watcher trigger** — if the project has a hook that watches `.steering/checkpoint.md`, writing the file may trigger a steering consultation automatically.

4. **Direct wire injection** — **only as a last resort** and only if you understand the session format. Appending a `context.append_message` event with `origin.kind="user"` can inject a message, but this risks corrupting the session if the format is wrong. Do not do this unless you have verified the format and the target agent is idle.

---

## Phase 4 — Verify the steering landed

After the user pastes the prompt, monitor the target's `wire.jsonl` to confirm it read your files and updated its plan.

```bash
tail -n 50 ~/.kimi-code/sessions/wd_<hash>/session_<uuid>/agents/main/wire.jsonl | \
  grep -E '"type":"context.append_message"|"type":"context.append_loop_event".*"type":"(think|text|tool.call)"' | \
  tail -20
```

Look for:
- A `tool.call` to `Read` on your steering files.
- A `think` part acknowledging the new direction.
- A `TodoList` update reflecting the merged plan.
- Text output stating the new plan.

If the target does not respond within a reasonable time, the prompt may not have been pasted, or the session may be stuck. Ask the user to confirm.

---

## Phase 5 — Iterate and avoid conflicts

Once the target is steering, continue to monitor it periodically. If you need to send further updates:

1. Update the steering checkpoint or handoff doc.
2. Provide a new pointer prompt: `"Re-read .steering/checkpoint.md — it has been updated."`
3. Verify the update landed by tailing the wire log.

### Conflict rules

| Situation | Rule |
|---|---|
| Target agent is editing `src/components/Header.tsx` | You do not edit that file. You write steering docs only. |
| Target agent has subagents assigned to specific files | Your steering docs should reference those assignments and suggest changes, not override them. |
| Target agent's plan conflicts with yours | Write a "Reconciled direction" section that merges both. Do not tell the target to discard its work. |
| Target agent is in plan mode | Steering docs will be read before it exits plan mode. Ideal. |
| Target agent is executing | Steering docs may cause a mid-task pivot. Be explicit about what to keep and what to change. |

---

## Obstacle handling

| Obstacle | Solution |
|---|---|
| Target session not in tmux | Read state from disk; use file-based steering. |
| Target not registered as a Factory peer | Use file-based steering; skip the Factory checkpoint. |
| Target agent is a different CLI (Claude, Codex) | Same pattern: find its session directory, read its state/log, write steering docs, activate via user paste. |
| Target agent has no `.steering/` convention | Create `.steering/` and write `checkpoint.md`; also write to `docs/coordination/`. |
| User refuses to paste prompt | Explain that direct injection is risky and ask them to paste. If absolutely necessary, use the Factory peer / tmux fallback. |
| Target session is archived or dead | Inform the user; the session cannot be steered. |
| Multiple agents on same project | Create a single `.steering/checkpoint.md` that addresses all agents; use `docs/coordination/` for per-agent notes. |

---

## Minimal viable steering (copy-paste checklist)

1. `cat ~/.kimi-code/sessions/.../state.json` — confirm target session.
2. `tail -n 50 ~/.kimi-code/sessions/.../agents/main/wire.jsonl` — understand current context.
3. `cat ~/.kimi-code/sessions/.../agents/main/plans/*.md` — read target's plan.
4. Write `.steering/checkpoint.md` and `docs/coordination/<topic>-handoff.md`.
5. Give the user a pointer prompt to paste.
6. `tail -n 50 ~/.kimi-code/sessions/.../agents/main/wire.jsonl` — verify it worked.

---

## Example from practice

**Scenario:** Allternit websites. One agent (session_1fa2fb78) was rebuilding the design system; a second agent (session_66874509) had completed a competitor audit. The user wanted the second agent to steer the first.

**What worked:**
1. Discovered the target session by listing `~/.kimi-code/sessions/`.
2. Read its `state.json` and saw subagents assigned to `www`, `compute`, `robotics`, `spaces`, `manufacturing`.
3. Tailed its `wire.jsonl` and saw it had built fonts and entered plan mode.
4. Wrote `.steering/checkpoint.md` and `docs/coordination/website-unification-handoff.md` in the shared repo.
5. Posted a Factory checkpoint event.
6. Gave the user a pointer prompt to paste.
7. Verified by tailing the wire log: the target read the steering files, updated its todo list to the merged plan, and began Phase 0.

**What did not work:**
- Factory peer messaging (target was not registered).
- tmux send-keys (target was not in tmux).
- Direct wire injection (too risky).
