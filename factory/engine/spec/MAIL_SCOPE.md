# Mail Scope (v1)

This system intentionally scopes mail to **coordination notes + approvals**.

Included:
- Thread creation
- Message send
- Review request/decision events
- Derived thread views from ledger events

Thread identity (locked):
- `thread_id = dag:<dag_id>` for work-wide coordination
- `thread_id = wih:<wih_id>` for execution-specific threads
- no other thread_id formats are allowed

Excluded (not implemented in v1):
- Delivery retries / transport servers (serve-http/serve-stdio)
- Guard/pre-commit integration
- File reservation operations beyond core leases
- Archive/restore tooling
- Share/encryption workflows
- Products/Docs/Doctor operational tooling

Upgrade path:
- If ops-grade mail is required, implement a mail subsystem with durable delivery
  mechanics that still emits ledger events for auditability.
