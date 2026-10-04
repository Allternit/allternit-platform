-- V232: owner-level "Needs you" inbox (digital twin layer 2, src/inbox_needs.rs).
--
-- The inbox itself is derived live from the tables that already hold the work
-- (gateway approvals, held emails, threads waiting on the owner, missed calls,
-- unanswered messages, failed deliveries, vendor ticket results). This table
-- only remembers what the owner did with an item: dismissed it, or snoozed it.
-- `item_at` is the item's own timestamp when it was handled, so an item that
-- later changes (a thread asks for you again, a newer message arrives) comes
-- back instead of staying hidden under an old dismissal.
CREATE TABLE IF NOT EXISTS inbox_state (
    owner      TEXT NOT NULL,
    item_id    TEXT NOT NULL,
    state      TEXT NOT NULL,          -- dismissed | snoozed
    until      TEXT,                   -- RFC 3339, snoozed only
    item_at    TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (owner, item_id)
);
