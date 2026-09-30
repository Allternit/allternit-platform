---
name: agent-orchestrator
description: Orchestrate external CLI agents (kimi, codex, agy, claude) in their own tmux sessions — you write the scope/plan, delegate execution, monitor or steer the session, then review the produced work, fix bugs, and iterate to the next phase. Use when the user asks to delegate, offload, or farm out implementation work to another CLI agent / terminal session ("spawn a kimi/codex/agy agent to do X", "take over a terminal"), or to run work outside this session to save tokens.
---

# CLI Agent Orchestrator (v2 — tmux-native)

You are the **orchestrator**: you own scoping, task specs, monitoring, review, and bug-fixing. The **executor** is an external CLI agent in its own tmux session. You never do the bulk implementation yourself — but you always verify it.

**Transport is tmux, driven by the bundled scripts** (`ao-doctor`, `ao-spawn`, `ao-send`, `ao-watch`, `ao-status`, `ao-kill`) — they live in `~/.claude/skills/agent-orchestrator/scripts/` and are symlinked onto PATH (`~/.local/bin`), so call them by bare name. This workflow is mirrored agent-agnostically at `~/.agent-orchestrator/ORCHESTRATOR.md` (loaded by kimi/codex/agy via their skills and AGENTS.md files) — keep the two in sync when editing. tmux targets sessions by name, so the wrong-window keystroke bug of the old AppleScript flow is structurally impossible. The user watches any executor live with `tmux attach -t ao-<slug>` (detach: `ctrl-b d`); every pane is also transcript-logged to `~/.agent-orchestrator/logs/` and survives session death.

## Phase 0 — Detect agents

```bash
ao-doctor   # probes tmux/script/git + every executor CLI: installed, version, launch flags verified against --help
```

Exit 0 = at least one executor usable; 1 = none; 2 = transport broken. Trust its per-vendor `interactive=`/`headless=` verdicts over the static table below — CLIs change flags between versions.

| Agent | Interactive TUI | Headless one-shot | Notes |
|---|---|---|---|
| kimi | `kimi --yolo` | ❌ `-p` CANNOT combine with `--yolo`/`--auto` | resume: `kimi -S <session-id>` |
| codex | `codex` (ao-spawn adds the workspace-write sandbox flags) | `codex exec "..."` | `--dangerously-bypass-approvals-and-sandbox` is rewritten to the sandbox by the spawn gate |
| claude | `claude --permission-mode acceptEdits` (ao-spawn adds `--settings <hook>`) | `claude -p "..." --permission-mode acceptEdits` | `--dangerously-skip-permissions` is rewritten by the spawn gate; no commrails binary = refused |
| agy | `agy --dangerously-skip-permissions` | check `agy --help` | **ungated** (logged in `spawn-gate.log`) | **Pick the cheapest mode that fits the phase**: headless one-shots for mechanical/bulk phases (deterministic completion, no TUI steering needed); interactive TUI for phases that may need steering or span multiple prompts (session context persists across phases — cheaper than cold restarts).

## Phase 1 — Scope and plan (you, in this session)

1. Do the analysis yourself (read code, compare against the reference/spec, produce the gap map). The executor must never have to make product decisions.
2. Write two files **inside the executor's workdir** (it may not read outside its workspace):
   - A **spec/map doc** (`docs/<TOPIC>_MAP.md`) — the full analysis.
   - A **task spec** (`docs/<TOPIC>_PHASE_<N>_TASK.md`) — one phase only:
     - Exact scope with file paths; explicit "do NOT start phase N+1".
     - **Constraints**: no builds/typechecks/dev servers, no git operations, match repo idiom (name the conventions: styling system, import patterns, icon library, export style).
     - **A structured deliverable sentinel**: "When finished, write `docs/<TOPIC>_PHASE_<N>_NOTES.md` starting with YAML frontmatter — `status: done|blocked`, `files_changed: [paths]`, `deviations: [what + why]`, `remaining: [items]` — then prose notes. That file existing = done." The frontmatter lets the scope check in review be scripted.
3. Phase the work so each review is tractable: shell/foundation first, primitives second, bulk migration third, polish last.

## Phase 2 — Spawn

```bash
~/.claude/skills/agent-orchestrator/scripts/ao-spawn [--worktree] <slug> <repo-dir> "<agent-launch-cmd>"
# prints: ao-<slug> <workdir> <logfile>
```

- **Use `--worktree` whenever the repo allows it.** It creates `<repo>-ao-<slug>` on branch `ao/<slug>`: the executor cannot collide with other agents, and review becomes `git -C <worktree> diff` instead of mtime forensics. The script refuses when the git root is `$HOME` (Eoj's home is a git root — a worktree would checkout everything); in that case spawn without it and fall back to mtime attribution in review.
- Headless one-shot pattern — chain the sentinel so completion is the command exiting, no polling ambiguity:
  ```bash
  ao-spawn <slug> <repo> "claude -p \"\$(cat docs/X_TASK.md)\" --permission-mode acceptEdits; touch docs/X_NOTES.sentinel"
  ```
  (Still require the real NOTES file in the task spec; the `.sentinel` guards against the agent forgetting it.)
- **Spawn gate (audit S1).** Every launch line goes through `ao-spawn-gate` (bash) / `ao_gate.rs` (engine `ao spawn` and `ao recover --apply`), byte-for-byte identical:
  - **claude / claude-code → hook.** Launched with `--permission-mode acceptEdits --settings ~/.agent-orchestrator/logs/ao-<slug>.claude-settings.json`; the settings file comes from `allternit-commrails hook claude-settings` and runs the PreToolUse hook (hard floor + Gate 2) on every tool call. `--dangerously-skip-permissions` (and `--permission-mode bypassPermissions`) is rewritten, with a one-line `spawn gate: rewrote ...` notice. No `allternit-commrails` binary (`$ALLTERNIT_COMMRAILS_BIN`, else PATH) → the spawn is refused, never run unhooked.
  - **codex → sandbox.** `--dangerously-bypass-approvals-and-sandbox` becomes `-c 'sandbox_mode="workspace-write"' -c 'approval_policy="never"' -c sandbox_workspace_write.network_access=true`; `danger-full-access` becomes `workspace-write`.
  - **everything else (kimi, agy, gemini, sh, ...) → ungated.** Unchanged, and labeled `gate=ungated` in `~/.agent-orchestrator/logs/spawn-gate.log` (every spawn is logged there with its class). With `ALLTERNIT_COMMRAILS_WIH` set, an ungated harness must pass `allternit-commrails hook spawn-check` or the spawn is refused.
  The gate classifies by the first command word (after `VAR=value`/`env`/`exec`), so launch the harness directly — `bash -c 'claude ...'` is logged as ungated `bash`.

## Phase 3 — Send prompts (verified protocol, scripted)

```bash
~/.claude/skills/agent-orchestrator/scripts/ao-send <slug> "Read docs/<TASK_FILE> and execute it exactly. It is your full task spec."
```

`ao-send` bracketed-pastes the prompt, reads the pane back, and **only presses Enter if every character verifiably landed in that session's input box** (alnum-only comparison, immune to TUI wrapping and box borders). On mismatch it clears with `C-u` and exits 1 — inspect with `ao-status <slug>` and retry. Keep prompts to one line pointing at the task file. After ~5s, `ao-status <slug>` to confirm the agent actually started (activity/todo output, empty input box).

## Phase 4 — Monitor and steer

```bash
~/.claude/skills/agent-orchestrator/scripts/ao-watch <slug> <NOTES_FILE> [timeout=3600] [interval=20]
```

Run it in the background (or under Monitor): exits 0 on DONE, 3 on PANE-DEAD (agent crashed/exited — read the transcript log), 4 on TIMEOUT. The sentinel file is the completion signal — **never** infer completion from busy-indicators; kimi shows no spinner while composing between tool calls. To check progress or stalls, `ao-status <slug> 40`. To steer, answer questions, or send the next phase: `ao-send` into the same session — context persists; say "Phase N is reviewed and approved" and reference the next task file.

**Never send C-c to a TUI agent** — it kills the process, not the input line. `ao-send` already uses `C-u` for clearing.

## Phase 5 — Review the work (NON-NEGOTIABLE)

Never accept the notes file at face value:

1. **True footprint** — worktree spawn: `git -C <worktree> status --porcelain && git -C <worktree> diff --stat`. Non-worktree: `find <src> -newer docs/<TASK_FILE> -type f -not -path "*/node_modules/*"` cross-checked with `git status`; other agents may share the repo — attribute by mtime + content, flag (don't revert) unexplained changes.
2. **Scope check**: diff the footprint against `files_changed` in the notes frontmatter — anything outside declared scope is a finding.
3. **Verify claims in code**: read the key diffs, especially (a) anything positioned/scrolling, (b) data-shape assumptions (read the hook's actual source), (c) deleted symbols (grep for lingering references), (d) new fetch/effect wiring (dependency arrays, refetch loops, error paths), (e) claimed imports/hooks actually existing.
4. **Cheap syntax gate** (not a build): `node -e "require('esbuild').transformSync(require('fs').readFileSync('<f>','utf8'),{loader:'tsx'})"` per changed file.
5. **Fix small bugs yourself**; note each fix in the next phase's task file ("a reviewer changed X — preserve it").
6. **Send bad implementations back** via `ao-send` with the findings; the executor redoes it in-session.
7. Report to the user: what passed, what you fixed, what you flagged.

## Phase 6 — Iterate, then clean up

Write the next phase's task spec, `ao-send` it to the SAME session, re-arm `ao-watch`. When all phases pass review: merge/apply the worktree branch, then `ao-kill <slug> [--rm-worktree]`. Report actual phase durations, not guesses.

## Platform integration (canvas + commrails)

`ao-*` sessions are discovered by the allternit app and shown as executor tiles on the code canvas (lifecycle → commrails mail thread `wih:executor-<slug>`; see ORCHESTRATOR.md "Platform integration"). In task specs, tell executors: append milestone notes to `.allternit/shared-context.md` when present (append-only, `### <slug> <ISO ts>`), and drop artifacts in `~/.agent-orchestrator/evidence/<slug>/`, announcing each via `curl -X POST http://127.0.0.1:8013/api/commrails/mail/share` with `{"thread":"wih:executor-<slug>","asset_ref":"<path>"}` — that's what makes progress and artifacts show up for humans in the app. (The old `/api/rails/mail/share` prefix remains as a one-release alias.)

## Dispatch semantics (registry, mailbox, ownership, recovery)

The ao engine adds durable dispatch semantics on top of the tmux flow. Additive: the `ao-*` bash scripts and `ao spawn/send/watch/status/kill/doctor` keep their exact behavior.

**Dispatch registry.** `~/.agent-orchestrator/state.json`. Legacy keys `cwd`/`log`/`dead` stay. Added: `runner`, `worktree`, `branch`, `sentinel`, `lead`, `lifecycle`, `world`, `queued`. Bash `ao-spawn`/`ao-kill` update the same file via `ao-registry-sync`.

**Bus mailbox.** `ao queue` / `ao send --queue` / `ao drain` / `ao watch` auto-drain. Settle only after verified paste. Never the HTTP inbox endpoint. Single drainer owned by ao-engine.

**Ownership.** Fail-closed lead→runner. `--as-human` override.

**Recovery.** `ao recover [slug]` (dry-run default; `--apply` respawns from `.cmd.sh` with harness resume argv).

**Validation boundary.** `infrastructure/executor/ao-engine/tests/dispatch_demo/run.sh`. Not claimed: multi-day autonomous runs, cross-machine leads, concurrent drainers.

## Pitfalls learned the hard way

- kimi `-p` refuses `--yolo`/`--auto` — TUI + `ao-send` is the only autonomous kimi path.
- C-c kills a kimi TUI outright; a pasted-but-unsubmitted line is already safe — just don't press Enter, or clear with C-u.
- Busy-indicator polling gives false idles on kimi — sentinel files only.
- Multiple yolo agents in one repo: never attribute working-tree changes to your executor without a worktree or mtime evidence.
- macOS screen-recording filenames contain U+202F — use globs for user-provided recordings.
- Scope each phase so its review fits in a few file reads. A 2,000-line unreviewed diff is not "done".
- AppleScript/Terminal.app is deprecated as transport (focus races caused wrong-window prompt injection); if the user insists on a visible window, open Terminal running `tmux attach -t ao-<slug>` instead.

## Provenance

Patterns adopted from: awslabs/cli-agent-orchestrator (named sessions, status semantics, attach-to-observe), kingbootoshi/codex-orchestrator (persistent transcript logs, completion notification chaining), primeline-ai/claude-tmux-orchestration (verify-idle-before-send handshake), claude-squad/Crystal (worktree-per-agent isolation).
