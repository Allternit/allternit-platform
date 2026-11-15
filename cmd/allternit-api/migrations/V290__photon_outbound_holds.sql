-- Photon (Beta) outbound texts held for the owner's approval (2026-11-11).
-- A bot's message to a Photon iMessage/SMS space is written here first (state 'pending'),
-- sent only after the owner approves it ('approved' -> 'sent'), and never sent to anyone
-- but the owner's own handles. vendor_message_id / vendor_status come from Photon's answer.
CREATE TABLE IF NOT EXISTS photon_outbound_holds (
    id                TEXT PRIMARY KEY,
    agent_id          TEXT NOT NULL,
    user_id           TEXT NOT NULL,
    to_handle         TEXT NOT NULL,
    space_id          TEXT NOT NULL,
    sender_id         TEXT NOT NULL,
    content           TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'pending',
    vendor_message_id TEXT,
    vendor_status     TEXT,
    error             TEXT,
    created_at        DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    decided_at        DATETIME
);
CREATE INDEX IF NOT EXISTS idx_photon_outbound_holds_agent ON photon_outbound_holds(agent_id, state);
