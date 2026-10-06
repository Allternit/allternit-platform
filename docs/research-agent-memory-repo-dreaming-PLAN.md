# Memory Drive and Dreaming — approved four-phase execution plan

Eoj approved all four phases on 2026-10-06 and reaffirmed continuation. Clone access ships with user-minted scoped tokens; imported rows are visible for 30 days then hidden. No production import/deploy without concrete preview and approval. Canonical git drive, automatically applied Dream commits with undo, twin owner approval preserved.

## Workspace and tracking

DAG dag_176229, root n_6275. Phase nodes: n_624, n_2090, n_6554, n_3509. Platform/gizzi/docs worktree: allternit-wt-memory-drive. Desktop/ai phone/PWA worktree: allternit-ai-wt-memory-drive. Branch factory/memory-drive in both. No shared-checkout edits. Older rq-20260908-008 and rq-20260908-011 superseded by rq-20261006-001.

## Grounded findings

- API hosted brains: brain_routes.rs uses bare repos and git http-backend. Existing git_tokens authenticate every owned brain with read/write scope; external Memory Drive clone requires narrower repo-scoped read-only token. Receive-pack must validate candidates before refs land and trigger reindex after successful push.
- API write path: memory_routes.rs explicit retain -> kernel.retain_turn, queued user retain -> memory_extraction.extract_and_reconcile, heuristic fallback -> kernel.persist_facts. Fact PATCH uses memory_relations.edit_fact. Delete uses kernel.delete_fact. Adapter paths live in memory_consolidation. All canonical mutations must converge, including bot/manual/project/thread callers.
- API index: memory_facts, memory_adapter_links, memory_entities/relations/embeddings, FTS/vector recall. Reindex must preserve provenance/type/retrieval metadata and delete only index rows owned by the edited drive. Repair after a failed post-commit reindex must be durable.
- Gizzi memory_write -> MemoryService frontmatter files, kernel-adapter mirrors into rows. Session instruction loads memdir index. Multiple memdir copies exist in main/shared/ink-app: change them consistently. Brain sync already does pull --rebase/push; never force.
- UI MemorySettingsPanel lists facts/entities; Phone SettingsScreen hosts the same SettingsView. Shared responsive drive component covers all three surfaces, with phone.css adaptations and signed-out/first-run/offline/error states. Phone notification source routing supports bot+session; Desktop query view handling only forwards customize context. Source links need a tested common session navigation contract.
- main.rs spawns memory_curation weekly and memory_consolidation daily. Both must stop mutating canonical drive-backed rows once Dream is on the path.
- Twin persona supports all/bot/owner visibility and proposed/active status. Only owner accept activates; a git edit must never bypass that gate or widen visibility.
- beta_memory stores are actively read by beta_work_routes and cloud_agents_routes. memory_notes is written by agent_gateway_routes. Phase 4 requires compatibility adapters/import; these tables are not safely droppable today.

## Phase 1 — Memory Drive (n_624)

1a. Core bare-git library: initialization, tree/file/history/diff/snapshot, one-line format, index, secret scan, explicit paths, atomic commit with expected-ref CAS. Scratch tests for first-write races, stale head, traversal/secrets, replay/idempotence. First independently reviewable diff.
1b. Drive registry/index: reserve per-user memory repo through brain registry with a uniqueness rule; new migration number checked against origin/main; configurable local/cloud storage. Drive routes under /memory/drive/*. Rebuild changed markdown into kernel facts with stable drive/file/entry identity. Keep existing recall API unchanged and durable dirty-index retry. All owner checks server-side.
1c. Writers and transport: explicit/queued/fallback retain, edit/delete, adapters and manual-save routes use drive batch commits. Secret scanner and format gate on smart-HTTP pushes; hook reindex on success. Scoped revocable git tokens separate read-only clone from explicit write credentials. Clone URL never contains secret.
1d. Reversible import: per-user dry-run plan with legacy count/topic/source mapping, one Import existing memory commit, durable idempotence marker and atomic rollback behavior. Existing data retained; 30-day archive visibility. Export/import memdir preserves unknown provenance honestly. No production import yet.
1e. Gizzi checkout: explicit owner-aware checkout, local offline signed-out drive, safe first-login merge, remote revision retry with no lost entries. MemoryService save/delete/list/read, recall and session-start MEMORY.md connected. TUI /memory presents files/history and sync errors. Every memdir duplicate reviewed.
1f. Shared UI: responsive file tree/markdown/source links/history/diff, token creation/revoke and copy URL. Shared SettingsView on Desktop/ai phone/PWA; source-session navigation works on each. Real request cancellation/error/loading/empty states, keyboard and reduced motion. No new effect package without justified real state.
1g. User/dev docs: guides/memory-drive.mdx, core/memory-kernel.mdx, api/brain-git.mdx, API reference/nav, link check zero. MIT notice for adopted published format.

Acceptance: first write creates standard repo and one commit; concurrent independent writers both land and same-entry conflicts reread/retry; recall changes within one push; deletes reflected; import no-op on rerun; secrets blocked on every path; scoped token clone works and denied for other brain/write; all surfaces browser and source links verified. Fresh scratch DB boot and focused API/gizzi/UI tests/typechecks after explicit verification authorization.

## Phase 2 — model-based nightly Dream (n_2090)

2a. Durable nightly schedule/lease per user using existing runner, immutable source revision, bounded recent transcript retrieval, completion via shared gizzi router. No transcript copies in drive and no direct new vendor.
2b. Source-checked constrained Dream plan: merge duplicates, resolve contradictions only from source evidence, synthesize lessons, prune unused stale entries with evidence. Missing source/model/error keeps memory intact; report failure honestly. Validate all changes and scan candidate secrets.
2c. One atomic Dream YYYY-MM-DD commit plus report, idempotent per-date retry tracking. Twin suggestions use proposal path only. No old cleanup job edits to drive-backed rows.
2d. Undo via revert commit preserving later writes; conflict is visible. Reports/history/undo across shared UI and Gizzi. Disable competing local autoDream when canonical scheduler owns the drive.
2e. Tests: source absence and malformed output, stale candidates, repeat nightly execution, undo after intervening edits, wrong-owner denial, twin no auto-activation, scheduler retry/error. Docs and link check.

## Phase 3 — mounted scoped drives and questions board (n_6554)

3a. Personal/project/team/bot/swarm registry and permission matrix; existing workspaces/workspace_members and bot/swarm ownership enforced on every read/write/token. Mounted repos stay separate, root-relative links never cross implicitly.
3b. Multi-drive session context and targeted writes on Gizzi/web/bots. Move bot/project/thread memory onto correct owner drive; no duplicate broader-scope copies. Preserve offline behavior and membership revocation.
3c. Twin drive folders preserve proposed/active provenance and all/bot/owner visibility. Owner-only and bot-private data must not leak through a broader cloned drive. Git writes cannot auto-activate proposals.
3d. questions.md board: stable source-linked IDs, author/date/status, ask/answer/resolve, atomic concurrent edits. Existing Factory tool/MCP surface checks same team/swarm membership and write role. UI/TUI reads same canonical board.
3e. Tests: cross-owner/team denial, revoked member, scoped mount context, concurrent questions/answers, twin approval/visibility. User/dev docs and nav.

## Phase 4 — retirement and external plugin (n_3509)

4a. Complete runtime inventory of memory_notes/session_memory/beta/cowork/procedural/document/frontmatter adapters. Mark each covered, compatibility facade, or nonredundant index/archive; no unexplained surviving canonical writer.
4b. Reversible idempotent import for covered stores and drive-backed facades for active endpoints. Keep read-only archive 30 days then hide. Retire obsolete job/route wiring only after callers covered. No production drop or destructive row cleanup.
4c. Installable Allternit external-agent plugin/skill with valid manifests, MIT published-format provenance, clone via credential helper, scoped MEMORY.md session load, safe source/date writing, reread/retry and no force push. Document scanner, Dream undo and twin proposal boundaries. Local artifact validated; external publication gets concrete preview/approval.
4d. End-to-end scratch flow from import to recall, concurrent writes, Dream/report/undo, scoped board and compatible legacy API. Update all user/dev guides, link checker zero, full review evidence.

## Review and release

Each bounded pass reports actual git footprint, tests run and tests awaiting authorization. Sentinel existence signals executor completion only; orchestrator reviews code and evidence before next phase. No builds/typechecks/dev servers/commits/pushes while explicit authorization question is unanswered. Production import: dry-run plan first, specific user approval, manual run per runbook. API/ai merge triggers production deployment and gets concrete preview/go-ahead. PWA and docs deploy are separate. Worktrees/artifacts cleaned only after reviewed work is safely saved; never remove sibling worktrees or live build output.
