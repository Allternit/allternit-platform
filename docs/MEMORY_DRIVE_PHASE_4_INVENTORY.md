# Memory Drive — Phase 4 store inventory (2026-10-06)

Every runtime memory store in `cmd/allternit-api`, what writes it, and what it
is now that the Memory Drive is canonical for personal memory. Nothing is
dropped or deleted; there is no production data migration in this change
beyond the opt-in, dry-run-first import.

| Store | Writers (call sites) | Status | Why |
| --- | --- | --- | --- |
| `memory_facts` (kernel facts) | `persist_facts(_typed)`, `apply_ops_typed`, `edit_fact`, `delete_fact`, `adapter_upsert/delete`, `memory_reconstruction_routes` | **Covered — index only.** With a drive root configured, every writer commits to the drive (`memory_drive_writer::commit_facts`, one commit per turn/edit) and rows are rebuilt by `memory_drive_service::reindex`. Remaining direct SQL: index projection (`index_snapshot`), soft supersession of drive-removed entries, retrieval counters/decay metadata (`note_retrieved`, `decay`), legacy-archive retirement after import. `apply_merge` skips drive-backed rows (Dream owns consolidation). The startup prune skips drive-backed rows. | Drive is canonical (Eoj, 2026-10-06). |
| `memory_observations` | `record_observation` | **Nonredundant: evidence log.** Turns the Dream and source links cite. Never copied into the drive. | Spec: no transcripts in the drive. |
| `memory_relationships` / `memory_edges` | `write_relation` | **Nonredundant: graph index metadata** built after drive commits. | Edges point at index fact ids. |
| `memory_adapter_links` | adapter routes | **Covered — mapping index** from external ids to drive-backed facts. | |
| `twin_memory` | twin routes, `tool_twin_propose`, Dream proposals | **Canonical for activation, mirrored.** Owner accept is the only activation path; `twin/active.md` and `twin/proposed.md` are a read-only projection (pushes and edits to `twin/` are refused). | Keeps the owner-approval gate. |
| `cowork_memory_entries` (bot/project memory) | cowork runtime crate (`allternit_cowork_runtime::sqlite_store`), `memory_curation` weekly job | **Compatibility facade, mirrored.** Principal/grant ACLs live here; each bot and project drive gets a read-only `cowork/memory.md` mirror, refreshed when the drive is opened. `cowork/` is refused for user writes and pushes. | The runtime's grant model has no drive equivalent yet; moving its writer is a separate change. |
| `memory_notes` | `memory_notes_routes`, `agent_gateway_routes` (gateway notes) | **Nonredundant: document store.** Multi-line titled notes (not one-line memories); indexed for search by `memory_index`. | Doesn't fit the one-line entry format. |
| `session_memory` | `session_memory_service` (`/memory/session`) | **Nonredundant: per-session scratch key/value** state, cleared with the session. | Not long-term memory. |
| `procedural_memory` | `procedural_memory_service` (`/memory/procedural`) | **Nonredundant: structured procedures** (trigger patterns, steps, use counts). | Structured records; Dream lessons cover prose procedures in `lessons.md`. |
| `beta_memory_stores` / `beta_memory_entries` | `beta_memory_store_routes`; read by `beta_work_routes`, `cloud_agents_routes` | **Nonredundant: developer API product** (namespaced key/value stores for API clients). | Customer-facing API with its own contract. |
| `memory_entities` | `/memory/entities`, extraction | **Nonredundant: graph index.** | |

## Retired runtime behavior

- `memory_consolidation::apply_merge` no longer edits drive-backed rows.
- `prune_turn_derived_facts` no longer touches drive-backed rows.
- Desktop gizzi `autoDream` stays off when the server Dream is enabled for the drive (gizzi checks `GET /memory/drive/settings`).

## External agents

`tools/memory-drive-plugin/` — Claude Code plugin manifest, `memory-drive` skill
(also usable by Codex and others), `scripts/memory-drive.sh` (clone with a
credential helper, load, remember with reread/retry and no force push), local
test `tests/test.sh`, MIT format NOTICE. Publishing it to a marketplace is a
separate step that needs Eoj's go-ahead.
