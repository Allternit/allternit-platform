# Memory Drive independent review checklist

Do not accept notes without checking the produced diff and real call sites.

## Phase 1
- Private repo stable per owner, lazy first-write race and no shared signed-out identities.
- File path/ref validation and byte limits, no symlink/path traversal.
- Every writer includes source/date on one line, secret scan before commit and before receive-pack acceptance.
- CAS uses expected head and first-write zero OID correctly; no global git identity changes.
- One git commit per turn/import/edit and no force push.
- Index keyed by drive/file/line identity, deletes remove only indexed rows; offline/reindex failure retry durable.
- Retain explicit, queued model path and heuristic fallback, adapter upsert/delete, manual fact deletes, gizzi save/delete/load/recall/TUI, external push all connected.
- Server users cannot read other's history/diffs/files; invalid refs rejected; scoped clone token cannot access another brain or receive-pack.
- Clone URL contains no credential, token returned once, revoke effective.
- Import dry run writes nothing; no partial marker on failure; rerun no new commit; archive visible until exactly migration+30d.
- Shared UI actually reachable on Desktop, ai phone and PWA, stale response cancellation, per-file history/diff selection, errors/offline/loading and source links.

## Phase 2
- Real completion call and transcripts source evidence, no lexical-only stand-in.
- Candidate bounded/schema checked and secrets blocked before apply.
- No transcript bodies, credentials or unverified factual changes committed.
- One Dream commit/report, idempotent nightly schedule/lease, retries visible.
- Undo reverts Dream changes while preserving later commits; conflict is surfaced.
- Old jobs no longer mutate drive index, twin changes only proposals.

## Phase 3
- Server membership/ownership checks cover every endpoint and token route.
- Mounted drive context isolation, selected write target and Factory board auth.
- Team/swarm concurrent questions/answers, stable IDs, removed-member denial.
- Twin owner-only/bot-only content never copied to broadly readable drive; owner approval cannot be bypassed by git push.

## Phase 4
- Inventory covers currently active beta store/cloud-agent/work callers; no blind drop.
- Runtime canonical writes retired, drive-backed compatibility endpoints and 30-day archives preserved.
- External skill artifact manifest/install/docs valid and MIT provenance retained.
- User/dev docs and navigation shipped; link checker zero.

## Release gates
Build/typecheck/scratch server/commit/push authorization pending user's async answer. Production import always dry run then specific human approval. Merging main triggers API/ai production deploy and needs concrete preview/go-ahead. PWA/docs deploy separate. No approved phase claimed complete before evidence exists.
