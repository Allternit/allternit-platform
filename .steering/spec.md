# Cloud Computer Phase 2 plan and verification
Source of truth: docs/learnings/CLOUD_COMPUTER_PHASE_2_TASK.md; reference docs/learnings/CLOUD_COMPUTER_PHASE_2_DESIGN.md.
- [x] Resize: validated PATCH, Incus live limits/stopped root disk, unsupported 501, default false capabilities.
- [x] Clone: credit gate, stateful snapshot/create/start, transactional owner/resource/billing mirror, retained restore snapshot, isolated native instance.
- [x] Idle: nullable timeout PATCH, activity touches, shutdown-aware sweeper, human-controls skip/recheck, shared stop and bot usage cleanup.
- [x] Groups: owner-scoped CRUD/counts, single-group attach/move/detach, foreign-key null-on-delete, visible-group list filter.
- [x] TypeScript client methods, exported types, groupId normalization/filter and focused tsconfig.
- [x] Required cargo check and tests; scoped tsc; SQL and HTTP regression tests; unchanged ACI/bot handlers verified.
- [x] Live availability probe captured. Actual VM smoke deferred: no Phase 2 API or configured Incus; no dev server started.
- [x] Prepare NOTES sentinel with files, deviations, remaining items and verification.
- [ ] Gated commit and push ao/cloud-computer-orgo-p2; record final SHA in external evidence and final response. No merge or Phase 3.

Necessary migration deviation: V133/V134 already exist in HEAD for provider routing policies/user credentials. Using them caused refinery UNIQUE(version) failures. Phase 2 migrations are V135/V136; original migrations untouched. Exact required SQL retained.
