# Codex CLI app-server repair — 2026-09-28

Session: `session/codex-cli-debug-20260928` (Codex)
PR: #902, merge commit `fe9f70ada415f9d5138ce18540e275db5513479e`

## Change

Allternit's Codex CLI adapter expected a top-level `threadId`, but Codex 0.158.0 returns `result.thread.id`; no turn could start. The adapter now reads that field, forwards the selected model, streams agent-message deltas without repeating the final message, and reports failed turns. Model discovery now reads the installed CLI's catalog instead of advertising obsolete placeholder IDs.

## Evidence

- Live Codex app-server: `thread/start` accepted `model: gpt-6-astra` and returned `result.thread.id`; a read-only turn emitted an agent-message delta and final item.
- Focused Bun tests: 26 passed, 0 failed.
- Desktop release preflight: 52 passed, 0 failed.
- `git diff --check`: clean.

## Incident and follow-through

The installed Desktop was started with debug logging and both gateway and Gizzi health endpoints returned 200. Its bundled Gizzi binary predates the repair. A Desktop build and install from merged main is in progress; this note does not claim the installed binary is fixed yet. The published Gizzi package is 25 commits behind main, so a public Gizzi release is a separate channel decision.
