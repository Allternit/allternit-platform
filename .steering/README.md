# Steering checkpoints

Hook-based steering for agent sessions working in this repo — works across every
agent CLI used here, not just one. When a session ends a turn, a `Stop` hook
(`.steering/bin/steer-stop.sh`) consults a **separate steering agent** — but only
if `checkpoint.md` changed since the last review.

Flow:

1. The working agent updates `checkpoint.md` (Goal / Just did / Next / Open questions)
   at meaningful checkpoints. Its AGENTS.md instructions require this.
2. The hook sends `prompt.md` (the review rubric) + checkpoint + **evidence** —
   git status, diff stat, the actual diff (capped at 16KB), and test output when
   `.steering/test-command` exists (a shell script the hook runs, tail 4KB kept) —
   to the steering agent.
3. **Gap analysis**: `.steering/spec.md` is the source of truth for what "done"
   means. Write it when the feature is scoped, keep it current, and make each
   requirement checkable. Every consult, the steering agent maps each requirement
   to DONE / PARTIAL / MISSING with code evidence — a MISSING core requirement
   blocks approval.
4. Steering replies `APPROVE` (turn ends) or `STEER` + the gap analysis, answers,
   and severity-tagged findings (BLOCKER/MAJOR/MINOR), which is injected back
   into the working session as a `[steering]` message.

There is also a **hard commit gate** (`steer-pre-commit-gate.sh`, `PreToolUse` on
shell tools): `git commit` / `git push` only execute after the steering agent
APPROVES them. All other commands pass without a consult.

The steering agent is deliberately a **different model family** than the usual
worker — same-model reviewers share the worker's blind spots. The Factory's
steering coordinator picks the consult backend (see Controls below).

Operational note: a cold consult (the steering agent's first turn) includes its
repo exploration and can exceed five minutes; the hook's 600s timeout covers it.

## Per-CLI wiring

Run `.steering/bin/steer-install.sh` once per machine (idempotent; re-run after
`git pull`). It registers:

| CLI | Mechanism | Where |
|---|---|---|
| kimi | `[[hooks]]` Stop + PreToolUse entries | `~/.kimi-code/config.toml` |
| Claude Code | project Stop + PreToolUse hooks | `.claude/settings.json` — **committed**, active automatically (approve the trust prompt on first run) |
| codex | Claude-style Stop + PreToolUse hooks + `codex_hooks` feature flag | `~/.codex/hooks.json`, `~/.codex/config.toml` |
| gizzi-code | `allternit-steering` plugin (`session.stop` hook) | copied to `~/.gizzi/plugins/` from `tools/agent-orchestrator/gizzi-plugin/` |
| agy | no hook support | convention only (AGENTS.md checkpoint rule) |

The hook script speaks every CLI's block protocol at once: stderr + exit 2
(kimi, Claude Code) and `{"decision":"block","reason":...}` JSON on stdout
(codex, gizzi-code). It no-ops in any project without `.steering/checkpoint.md`,
so the global registrations are safe for other repos.

## Session worktree hooks

Two more hooks ride the same registrations (all four CLIs; gizzi-code also has
native `--worktree`, default-on tracked as phase W2):

- `session-worktree.sh` (UserPromptSubmit; also SessionStart for Claude Code) —
  injects the per-session worktree ritual once per session when the cwd is the
  shared main checkout.
- `guard-main-checkout.sh` (PreToolUse on shell tools) — blocks git mutations
  in the shared checkout; linked worktrees pass automatically. Escape hatch for
  human/orchestrator merges: `STEER_GUARD_OFF=1`.

Tests: `bash .steering/bin/worktree-guard-test.sh`.

## Git-discipline gate (Stop hook)

`git-discipline-gate.sh` runs at every turn end, BEFORE the steering consult
(it is registered first in Stop order). It runs `scripts/git-discipline-check.sh`
against the SHARED checkout — resolved via `git worktree list`, so it works when
a session stops from inside its own worktree — and BLOCKS the stop while the
checkout has drifted:

- detached HEAD, or not on `main`
- `main` behind or ahead of `origin/main`
- unmerged stale branches (beyond branches checked out in live worktrees and
  branches listed in `.steering/git-discipline-allowlist`)

A dirty working tree does NOT block — the shared checkout may legitimately hold
another session's in-flight uncommitted work (soft-warn instead). Escape hatch
for humans/orchestrators: `STEER_GUARD_OFF=1` (same as the other hooks). The
gate enforces AGENTS.md commandment 6; it is mechanical and never consults an
agent. Intentional long-lived unmerged branches are recorded one per line in
`.steering/git-discipline-allowlist` with a reason (see the checkpoint for the
current entries).

## Controls

- Kill switch: `touch .steering/off` (delete to re-enable).
- Status: `.steering/bin/steer-status.sh` prints `steering: ENABLED|DISABLED` and
  the last 5 consults (exit 0 enabled, 1 disabled).
- State and consult log: `.steering/state/` (gitignored).
- Consult backend: `gizzi orchestration steer consult` — the Allternit Factory's
  steering coordinator, which builds the steering context and consults the
  configured backend. Falls back to `kimi -p` (fresh session per consult) when
  `gizzi` is not on PATH. Override either with `STEER_CONSULT_CMD` (reads the
  prompt on stdin, writes the answer on stdout). The old consult scripts are
  gone; see the Factory migration page (`surfaces/docs/factory/migration.mdx`).
- Test evidence: put a runnable shell script at `.steering/test-command` and the
  Stop hook will run it (2 min cap where `timeout`/`gtimeout` exists) and attach
  the output tail to every consult.
