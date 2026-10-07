# Memory Drive (Agent Memory Repo + Dreaming) — Claude Code, 2026-10-06

**Spec:** Research/specs/agent-memory-repo-dreaming.md (rq-20261006-001, landed). Picked up from a stalled Codex run.

## Shipped
- platform #1348 + ai #445: per-user git-backed Memory Drive (Agent Memory Repo format, canonical; `memory_facts` is a rebuilt index), every kernel writer through `memory_drive_writer` (one commit per turn), repo-scoped read/write git tokens, pre-receive validation of every push, import of existing memory (dry run, one commit, 30-day archive), nightly Dream with evidence checks/report/undo, team/project/bot/swarm drives, questions board, twin mirror, bot/project memory canonical in their drives, Gizzi checkout + `gizzi memory`, external Claude Code/Codex plugin (`tools/memory-drive-plugin`), docs. Migration V242.
- platform #1362 + ai #448 (gap analysis closed): two-way sync between the owner's computers run by the Desktop app (no main copy, Eoj's decision), delete forever (history rewrite + refuse-back, Gizzi purge-safe replay), import from ChatGPT/Claude/pasted lists, Needs-you import reminder, Dream in the owner's time zone, storage usage + 80% warning, `memory_drive_events_total` metrics, questions board as bot MCP tools + Gizzi CLI, plain "no computer online" message. Migration V243.
- m.allternit.com PWA deployed from ai `22b38aaa`.

## Verified
Rust: 114 + 38 tests (memory drive, Dream, cowork, git, twin, inbox, vendor-bot MCP). Fresh-DB boot of the real binary, 14 live checks (remember/recall, read-token clone, read token can't push, secret push refused by hook, good push indexed). Gizzi drive tests 23/23. UI typecheck 0, vitest 53 + 18. Docs link check 0.

## Outstanding
- Live look on Desktop / ai.allternit.com / PWA after the next Desktop build (another agent owns the build).
- Eoj's own import of existing memories (preview → apply) after that build.
- Gizzi full suite on a clean install (3 non-drive memory test files couldn't load on a borrowed install).

## Optional (not started; need Eoj's go-ahead)
- Publish `tools/memory-drive-plugin` to a Claude Code plugin marketplace (public release).
- Release video / post explaining the Memory Drive.
