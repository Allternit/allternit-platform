# MCP Agent Mail (Vendor Notes)

Source: `3-adapters/vendor/mcp_agent_mail`

Key reference points:
- Mail-like coordination layer for coding agents (FastMCP server, HTTP).
- Provides agent identities, inbox/outbox, searchable history, and threaded messages.
- Advisory file reservation leases to avoid conflicts.
- Git-backed mail artifacts + SQLite indexing for queries.
- Design notes and original prompt are in `project_idea_and_guide.md`.

How we use it:
- We mirror the core semantics (threads, messages, attachments, leases) in Allternit
  without depending on this runtime.
- We keep message flow as ledger events + derived thread views.
- File reservations stay authoritative in SQLite leases.

Why it matters:
- Provides a proven async coordination model (mail) and lease conventions.
- Useful for naming and event taxonomy parity.
