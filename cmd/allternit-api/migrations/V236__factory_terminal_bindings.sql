-- V236: Factory bots (SPEC §9 "Bots in the Factory").
--
-- 1. Terminal bindings. `bot_execution_bindings.type` gains the value
--    'terminal': a CLI harness (claude, codex, kimi, grok, agy, gizzi ...) in an
--    engine pane. A terminal binding has no vendor, account or lane; it names
--    its harness, and optionally the machine and pane it runs in. Validation
--    lives in allternit-api (agent_gateway_routes.rs), as for the other types.
ALTER TABLE bot_execution_bindings ADD COLUMN harness TEXT;
ALTER TABLE bot_execution_bindings ADD COLUMN machine TEXT;
ALTER TABLE bot_execution_bindings ADD COLUMN pane_id TEXT;

-- 2. Factory bot slugs: `allternit-factory agents bot add <slug>` is
--    idempotent on (owner, slug). The bot itself is an ordinary `agents` row.
CREATE TABLE IF NOT EXISTS factory_bot_slugs (
    owner      TEXT NOT NULL,
    slug       TEXT NOT NULL,
    bot_id     TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (owner, slug)
);
CREATE INDEX IF NOT EXISTS idx_factory_bot_slugs_bot ON factory_bot_slugs(bot_id);

-- 3. Vendor tickets as node deliveries. A ticket linked to a Factory node
--    (dag, node, WIH, workspace root) is how that node reaches a vendor bot;
--    its post_result closes the WIH through the Gate with the result as the
--    node output. node_close_state: pending | closing | closed | failed.
ALTER TABLE vendor_tickets ADD COLUMN dag_id TEXT;
ALTER TABLE vendor_tickets ADD COLUMN node_id TEXT;
ALTER TABLE vendor_tickets ADD COLUMN wih_id TEXT;
ALTER TABLE vendor_tickets ADD COLUMN workspace_root TEXT;
ALTER TABLE vendor_tickets ADD COLUMN node_close_state TEXT;
ALTER TABLE vendor_tickets ADD COLUMN node_close_error TEXT;
ALTER TABLE vendor_tickets ADD COLUMN node_close_json TEXT;
ALTER TABLE vendor_tickets ADD COLUMN node_close_at TEXT;
CREATE INDEX IF NOT EXISTS idx_vendor_tickets_node ON vendor_tickets(dag_id, node_id);
-- One delivery per (owner, dag, node, WIH): a repeat create returns the same ticket.
CREATE UNIQUE INDEX IF NOT EXISTS idx_vendor_tickets_node_wih
    ON vendor_tickets(owner, dag_id, node_id, wih_id) WHERE dag_id IS NOT NULL;
