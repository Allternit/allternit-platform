# Vendor Compatibility Matrix

Status legend:
- **Supported**: Allternit provides equivalent semantics + CLI
- **Partial**: core semantics exist but CLI or extra ops are missing
- **Not supported**: explicitly out of scope for v1

## Upstream issue-tracking CLI — command families

Supported / Partial:
- create/update/show/list: **Partial** → `allternit plan new/refine/show`, `allternit wih list`
- dep/blocked/ready/graph: **Partial** → DAG edges + cycle detection + `wih list --ready`
- gate: **Partial** → `allternit gate status/check/rules/verify` (no waiter management)
- history/audit: **Partial** → `allternit ledger trace`
- close/reopen: **Partial** → `allternit wih close` (no reopen)

Not supported (v1):
- daemon/daemons, sync/federation, gitlab/jira/linear, worktree, admin/compact/repair,
  labels/comments/kv, templates/formulas, mol/epic/swarm/merge-slot, import/export, hooks.

## MCP Agent Mail CLI

Supported / Partial:
- thread/message/review semantics: **Supported** → `allternit mail ensure/send/request-review/decide`
- inbox status/search: **Not supported** (no server/FTS mail store)

Not supported (v1):
- serve-http/serve-stdio, guard, file_reservations, archive/restore, share/encrypt,
  products/docs/doctor tooling, project discovery utilities.

## Notes
- Allternit reuses vendor **semantics**, not their runtime.
- Any unsupported surface can be added later as an Allternit-native module.
