---
doc: project
updated: 2026-10-06
status: active
---

# Baseline: agent-memory

Mapped from `origin/main` on 2026-10-06 (platform `ab44cbf37`, workspace `eba9bc4b`), while researching Cognition's Agent Memory Repo and Dreaming (rq-20261006-001).

## What we already have

- **Account memory kernel (canonical today).** SQLite in allternit-api: `memory_observations`, `memory_facts`, `memory_entities`, `memory_relationships`, `memory_embeddings` (`V86`), plus decay and retrieval counts (`V209`), FTS5 and vector search (`V206`), and typed relations (`V208`). Reads go through `/memory/v2/recall`; auto-extraction goes through `/memory/v2/retain` (`memory_extraction.rs`).
- **Other stores.** Bot, project and thread memory in `cowork_memory_entries`. Twin memory in `twin_memory`, which has full provenance and proposed/active states, and only the owner can accept a fact (`twin_persona.rs`). Also `memory_notes`, `procedural_memory`, `session_memory`, `beta_memory_*` and `memory_documents`.
- **gizzi memdir.** `~/.gizzi/projects/<root>/memory/` holds a `MEMORY.md` index plus one frontmatter `.md` per memory (`cmd/gizzi-code/src/memdir/paths.ts`). Every write is mirrored to the kernel by `runtime/memory/kernel-adapter.ts`. `memory_write` is the tool; the index is injected at session start (`runtime/session/instruction.ts`).
- **Consolidation.**
  - `memory_consolidation.rs` runs every 24h. It merges by lexical match or Jaccard similarity and soft-decays old facts. It never calls a model.
  - `memory_curation.rs` runs weekly. It asks the bot's own model to KEEP, MERGE or DROP entries, and it hard-deletes.
  - gizzi `autoDream` (`services/autoDream/`) runs a model consolidation pass over memdir and transcripts. It is off by default.
- **Git hosting.** Hosted brains are per-user bare repos served over smart-HTTP with `allternit_git_` tokens (`brain_routes.rs`, `V33`). The `gizzi brain init/sync` CLI exists but is not connected to memory.
- **Schedulers.** `routine_local_scheduler.rs`, `cron_lite.rs`, `deployment_scheduler.rs`, the gizzi cron daemon, and the cloud-api `scheduler_service.rs`.
- **UI.**
  - Settings → Memory (`MemorySettingsPanel.tsx`) and the Memory Kernel view.
  - Bot Home Memory tab, Thread Inspector memory, the project memory modal, and Twin.
  - Phone reuses the bot Memory tab only.
  - TUI: `/memory`, `/memory-search`, and a Dream dialog.
- **Docs.** `surfaces/docs/core/memory-kernel.mdx`, `api/memory*.mdx`, `api/brain-git.mdx`, `guides/your-twin.mdx`.

## Gaps

- No memory is versioned. The canonical store is SQLite rows, and memdir is an unversioned mirror.
- The format is one row or one file per memory. There are no `[source: …; added: …]` bullets and no `[[path]]` links.
- Three cleanup jobs overlap. None of them checks sources to settle contradictions, and none finds lessons across sessions.
- You can't mount personal, team and swarm repos together. Team sync has no backend.
- There is no message board between agents.
- Hosted brains are not connected to memory.

## Related

- Specs: `specs/add-scoped-memory-briefings-gizzi-code.md` (rq-20260908-008), `specs/add-sourced-fact-history-agent-memory.md` (rq-20260908-011), `specs/agent-memory-repo-dreaming.md` (rq-20261006-001).
