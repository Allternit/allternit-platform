# Agency Kernel configuration gaps — Codex — 2026-10-11

User authorized gap completion, commit and merge against the supplied Agency Kernel dashboard reference.

## Landed changes

The shared settings source now includes a responsive five-area kernel overview, complete frozen schema inspection/copy with failure feedback and current version/primitive counts, decision policy/verifier/completion/override navigation, and unified skills/plugins/templates access. Network/environment and privacy placeholders were replaced with existing backend-enforced approval/fencing and scoped local-only model routing. Canonical tiers remain S0 Commands / S1 Decisions / S2 Generate / S3 Solve. Immutable safety targets remain locked.

Project requests now send workspace/org parent chains. Empty custom rules remain local drafts until their text and matching patterns are complete. Rule writes are serialized through disabled controls, and stale scope loads/writes do not replace another scope's view. Contract lists scroll within a bounded phone area; inspector headers/actions wrap.

User and developer docs, navigation and the dependency map were updated. CI exposed a pre-existing docs-linter failure on provider-specific bot-isolation instructions; that page was added to the existing provider-integration exception list. Full local docs lint and Mintlify validation then passed. The obsolete agent-rules backend header was corrected; no Rust runtime behavior or frozen ABI validation changed.

## Evidence

- UI PR: https://github.com/Allternit/allternit-ai/pull/515
- UI merge: 6cf9c89f0c5cde64639cabb9fd928b14ee1ea403
- Platform/docs PR: https://github.com/Allternit/allternit-platform/pull/1502
- Platform/docs merge: b8519dcd06f4a9d71f86e660d94130b76556de75
- 39 targeted kernel/settings/phone tests passed. Contract tests passed again after phone layout fixes.
- TypeScript check passed. Two dependencies already pinned in the lockfile but absent from the borrowed node_modules were unpacked only into the owned worktree for verification; manifests/lockfiles were unchanged.
- Platform PR CI: all six checks pass (Desktop unit tests, Desktop typecheck/build, docs lint/build, service-worker guard, secrets scan and typography).
- Docs link check: 527 navigation entries, 522 pages, 0 problems.
- Dependency map: valid against merged UI source.
- Frozen schema projection: 65 contracts, 21 schema files and 191 primitives match platform source.
- Playwright inspected actual component markup at desktop/phone widths, using unavailable/empty test services. No production data/settings were changed in that preview. Catalogue overflow and action clipping caught by the preview were corrected. This was component layout QA, not a signed-in production end-to-end test.

## Limits and incidents

The configured steering consult returned a transport error: no steering backend installed. No approval rejection occurred. Repository hook documents fail-open behavior on a consult failure. Local verification and CI supplied the merge evidence.

The UI source ships to all three surfaces. Desktop packaging and the separate PWA release deployment were not performed: user requested source changes, commit and merge, with an explicit no-build/no-dev-server rule. Privacy restricts Agency model candidates, not arbitrary tools/connectors. Retrieval preference remains trace-only and is labelled accordingly. No domain firewall, sensitivity-label enforcement or general retention policy was invented.

Existing unrelated edits in the shared checkouts were preserved. Session-owned worktrees/branches and browser were cleaned up after merge.
