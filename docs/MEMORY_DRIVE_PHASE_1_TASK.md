# Phase 1 task — canonical Memory Drive

Read MEMORY_DRIVE_MAP.md, MEMORY_DRIVE_SPEC.md, MEMORY_DRIVE_BASELINE.md, MEMORY_DRIVE_UI_SKILL.md and MEMORY_DRIVE_VOICE.md first. DAG dag_176229 root n_6275. Eoj approved all four phases in this session; this task implements Phase 1 first for bounded review. Work ONLY in these owned worktrees: /Users/joe/Desktop/allternit-workspace/allternit-wt-memory-drive and /Users/joe/Desktop/allternit-workspace/allternit-ai-wt-memory-drive.

First expand full four-phase plan into docs/research-agent-memory-repo-dreaming-PLAN.md with dependency and call-site inventory; then implement Phase 1 from the spec. New standard is https://github.com/AgentMemoryRepo/agentmemoryrepo/blob/main/SPEC.md. Clone tokens ship now; old imported rows visible for 30 days then hidden. Use server canonical drive API, git commit CAS, secret scan, index rebuild, history/diff/tree/file/clone routes, import dry run, gizzi local offline checkout with rejected push retry, source-session links, shared responsive UI for all three surfaces, user/dev docs and nav.

Do NOT create a toy disconnected library or placeholder routes. Existing retain/adapter/manual-save/delete routes and gizzi sessions must use the drive end-to-end. No data loss in concurrent first writes. Do not break identity/team gates. Audit smart-HTTP pushes to ensure format and secret validation plus post-push reindex. Signed-out local identity must preserve account separation.

Authorization limits: no builds/typechecks/dev servers, commits/pushes/merges, production import, deploys or production data deletion. Write implementation and meaningful focused tests; run safe focused tests and docs/check_links.py. Report missing build/smoke/typecheck verification honestly as awaiting explicit authorization. Do not ask Eoj directly; route missing decisions to orchestrator. No other agents unless orchestrator specifically requests.

Completion: docs/MEMORY_DRIVE_PHASE_1_NOTES.md with exact evidence and frontmatter status/files_changed/deviations/remaining/brain_updates, then docs/MEMORY_DRIVE_PHASE_1.sentinel. Do not proceed to Phase 2 until review.

## Orchestrator read-through findings
- brain_routes token today grants all owner brains and receive-pack. Clone endpoint must mint repo-scoped read-only token and enforce scope for info/refs service and upload/receive-pack; retain a separate explicit write-scoped credential for gizzi sync. Do not embed token in clone URL; use separate one-time token display and credential-helper docs.
- Explicit retain calls kernel.retain_turn directly; queued user extraction calls extract_and_reconcile with only DbHandle. Cover both and the fallback persist_facts path, not just the HTTP route. Pass explicit drive location/config safely instead of relying on a global default that differs from Desktop data_dir.
- Session links: current ShellApp query deep links handle view/section/item for customize only. Verify actual session route and implement source-link handling consistently on desktop and PhoneShell if absent; do not invent a nonworking URL.
- Focused Rust tests compile code and remain prohibited pending the user's verification authorization. Existing script/pure JS tests and docs link checker are allowed. Do not treat no-build restrictions as license to claim untested Rust behavior.
