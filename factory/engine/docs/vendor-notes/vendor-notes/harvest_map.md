# Vendor Harvest Map

This lists the vendor behaviors we mirrored in Allternit and where they live.

## Upstream issue-tracking CLI

- Dependency model (blocked_by edges) → `work/types.rs`, `work/projection.rs`
- Cycle detection → `work/graph.rs::would_create_cycle`
- Readiness computation → `work/graph.rs::ready_nodes`
- Event-sourced state derived from append-only log → `ledger/ledger.rs`, `work/projection.rs`
- WIH lifecycle anchor (analogous to issue state) → `wih/projection.rs`, `gate/gate.rs`
- Gate checks for readiness + exclusivity → `gate/gate.rs::wih_pickup_with`

Dropped (v1):
- Daemon + sync + federation + SCM integration
- Templates/formulas/molecules/epics
- Labels/comments/kv

## MCP Agent Mail

- Thread + message semantics → `mail/mail.rs`
- Review request/decision events → `mail/mail.rs`
- Derived thread views → `mail/projection.rs`

Dropped (v1):
- Transport servers, delivery retries
- Guard/pre-commit enforcement
- Share/encryption workflows
- Archive/restore tooling
